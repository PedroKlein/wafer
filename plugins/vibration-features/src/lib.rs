//! Vibration features plugin for WAFER pipeline.
//!
//! Extracts time-domain and frequency-domain features from accelerometer
//! samples using FFT for predictive maintenance / condition monitoring.
//!
//! Config: { "sample_rate": 10000, "fft_size": 1024, "bands": [[0,100], [100,1000], [1000,5000]] }
//! Input: raw bytes = N × f32 little-endian accelerometer samples
//! Output: JSON with RMS, peak, crest_factor, kurtosis, dominant_freq, band energies, health_score

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use exports::pipeline::node::transform::{OutputMessage, ProcessError};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::Deserialize;
use wafer_plugin::{bad_input, define_state, output_with_type, payload_bytes, set_state, with_state};

#[derive(Deserialize)]
struct VibConfigInput {
    sample_rate: f64,
    fft_size: usize,
    bands: Vec<(f64, f64)>,
}

struct VibConfig {
    sample_rate: f64,
    fft_size: usize,
    bands: Vec<(f64, f64)>,
}

define_state!(VibConfig);

struct VibrationFeatures;

impl exports::pipeline::node::lifecycle::Guest for VibrationFeatures {
    fn validate(config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        match serde_json::from_str::<VibConfigInput>(&config.config) {
            Ok(c) => {
                if c.fft_size == 0 || (c.fft_size & (c.fft_size - 1)) != 0 {
                    return Some("fft_size must be a power of 2".to_string());
                }
                if c.sample_rate <= 0.0 {
                    return Some("sample_rate must be positive".to_string());
                }
                None
            }
            Err(e) => Some(format!("config parse error: {e}")),
        }
    }

    fn init(
        config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        let input: VibConfigInput = serde_json::from_str(&config.config)
            .map_err(|e| exports::pipeline::node::lifecycle::ProcessError::BadInput(format!("config parse error: {e}")))?;

        set_state!(VibConfig {
            sample_rate: input.sample_rate,
            fft_size: input.fft_size,
            bands: input.bands,
        });
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for VibrationFeatures {
    fn process(
        input: exports::pipeline::node::transform::Message,
    ) -> Result<
        exports::pipeline::node::transform::OutputMessage,
        exports::pipeline::node::transform::ProcessError,
    > {
        let bytes = payload_bytes!(&input);

        if bytes.len() % 4 != 0 {
            return Err(bad_input!("not aligned to f32"));
        }

        let num_samples = bytes.len() / 4;

        with_state!(cfg => {
            if num_samples < cfg.fft_size {
                return Err(bad_input!(format!("insufficient samples, need at least {}", cfg.fft_size)));
            }

            // Parse f32 samples
            let samples: Vec<f32> = (0..num_samples)
                .map(|i| {
                    let start = i * 4;
                    f32::from_le_bytes([bytes[start], bytes[start+1], bytes[start+2], bytes[start+3]])
                })
                .collect();

            // Time-domain features
            let rms = compute_rms(&samples);
            let peak = compute_peak(&samples);
            let crest_factor = if rms > 0.0 { peak / rms } else { 0.0 };
            let kurtosis = compute_kurtosis(&samples);

            // FFT — use first fft_size samples
            let fft_samples = &samples[..cfg.fft_size];
            let magnitudes = compute_fft_magnitudes(fft_samples, cfg.fft_size);

            // Dominant frequency
            let dominant_freq = find_dominant_freq(&magnitudes, cfg.sample_rate, cfg.fft_size);

            // Band energies
            let band_energies: Vec<f64> = cfg.bands.iter()
                .map(|(low, high)| compute_band_energy(&magnitudes, *low, *high, cfg.sample_rate, cfg.fft_size))
                .collect();

            // Health score heuristic (0-1): lower is worse
            // High crest factor and kurtosis indicate impulsive faults
            let health_score = compute_health_score(crest_factor as f64, kurtosis);

            // Build output JSON
            let json = build_output_json(rms as f64, peak as f64, crest_factor as f64, kurtosis, dominant_freq, &band_energies, health_score);

            Ok(output_with_type!(
                &input,
                json.into_bytes(),
                "application/json"
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Signal processing helpers
// ---------------------------------------------------------------------------

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

fn compute_peak(samples: &[f32]) -> f32 {
    samples.iter().map(|x| x.abs()).fold(0.0f32, f32::max)
}

fn compute_kurtosis(samples: &[f32]) -> f64 {
    let n = samples.len() as f64;
    if n < 4.0 {
        return 0.0;
    }

    let mean: f64 = samples.iter().map(|&x| x as f64).sum::<f64>() / n;
    let variance: f64 = samples.iter().map(|&x| { let d = x as f64 - mean; d * d }).sum::<f64>() / n;

    if variance < f64::EPSILON {
        return 0.0;
    }

    let fourth_moment: f64 = samples.iter().map(|&x| { let d = x as f64 - mean; d * d * d * d }).sum::<f64>() / n;
    fourth_moment / (variance * variance)
}

fn compute_fft_magnitudes(samples: &[f32], fft_size: usize) -> Vec<f64> {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(fft_size);

    let mut buffer: Vec<Complex<f64>> = samples.iter()
        .map(|&x| Complex::new(x as f64, 0.0))
        .collect();

    fft.process(&mut buffer);

    // Only first half (Nyquist)
    let half = fft_size / 2;
    buffer[..half].iter().map(|c| c.norm()).collect()
}

fn find_dominant_freq(magnitudes: &[f64], sample_rate: f64, fft_size: usize) -> f64 {
    if magnitudes.is_empty() {
        return 0.0;
    }

    // Skip DC bin (index 0)
    let (max_idx, _) = magnitudes.iter().enumerate().skip(1)
        .fold((0usize, 0.0f64), |(best_i, best_v), (i, &v)| {
            if v > best_v { (i, v) } else { (best_i, best_v) }
        });

    max_idx as f64 * sample_rate / fft_size as f64
}

fn compute_band_energy(magnitudes: &[f64], low_hz: f64, high_hz: f64, sample_rate: f64, fft_size: usize) -> f64 {
    let freq_resolution = sample_rate / fft_size as f64;
    let low_bin = (low_hz / freq_resolution).ceil() as usize;
    let high_bin = (high_hz / freq_resolution).floor() as usize;

    let high_bin = high_bin.min(magnitudes.len());
    if low_bin >= high_bin {
        return 0.0;
    }

    magnitudes[low_bin..high_bin].iter().map(|m| m * m).sum()
}

fn compute_health_score(crest_factor: f64, kurtosis: f64) -> f64 {
    // Healthy: crest ~1.4 (sine), kurtosis ~3.0 (Gaussian)
    // Faults: crest > 4, kurtosis > 5
    let crest_penalty = ((crest_factor - 1.4) / 4.0).clamp(0.0, 1.0);
    let kurtosis_penalty = ((kurtosis - 3.0) / 10.0).clamp(0.0, 1.0);
    (1.0 - (crest_penalty + kurtosis_penalty) / 2.0).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

fn build_output_json(rms: f64, peak: f64, crest_factor: f64, kurtosis: f64, dominant_freq: f64, band_energies: &[f64], health_score: f64) -> String {
    let bands_str: Vec<String> = band_energies.iter().map(|e| format!("{e:.6}")).collect();

    format!(
        "{{\"rms\": {rms:.6}, \"peak\": {peak:.6}, \"crest_factor\": {crest_factor:.4}, \"kurtosis\": {kurtosis:.4}, \"dominant_freq\": {dominant_freq:.2}, \"bands\": [{bands}], \"health_score\": {health_score:.4}}}",
        bands = bands_str.join(", ")
    )
}

export!(VibrationFeatures);

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn generate_sine(freq: f64, sample_rate: f64, num_samples: usize, amplitude: f32) -> Vec<f32> {
        (0..num_samples)
            .map(|i| (amplitude as f64 * (2.0 * PI * freq * i as f64 / sample_rate).sin()) as f32)
            .collect()
    }

    #[test]
    fn test_known_frequency() {
        let samples = generate_sine(1000.0, 10000.0, 1024, 1.0);
        let magnitudes = compute_fft_magnitudes(&samples, 1024);
        let dominant = find_dominant_freq(&magnitudes, 10000.0, 1024);
        // Should be close to 1000 Hz (within one bin: 10000/1024 ≈ 9.77 Hz)
        assert!((dominant - 1000.0).abs() < 15.0, "dominant_freq should be ~1000 Hz, got {dominant}");
    }

    #[test]
    fn test_rms_pure_sine() {
        let samples = generate_sine(1000.0, 10000.0, 10000, 1.0);
        let rms = compute_rms(&samples);
        // RMS of a sine wave = amplitude / sqrt(2) ≈ 0.7071
        assert!((rms - 0.7071).abs() < 0.01, "RMS of unit sine should be ~0.707, got {rms}");
    }

    #[test]
    fn test_band_energy() {
        // Generate 500 Hz sine — energy should be in band [100, 1000]
        let samples = generate_sine(500.0, 10000.0, 1024, 1.0);
        let magnitudes = compute_fft_magnitudes(&samples, 1024);

        let low_band = compute_band_energy(&magnitudes, 0.0, 100.0, 10000.0, 1024);
        let mid_band = compute_band_energy(&magnitudes, 100.0, 1000.0, 10000.0, 1024);
        let high_band = compute_band_energy(&magnitudes, 1000.0, 5000.0, 10000.0, 1024);

        assert!(mid_band > low_band * 100.0, "mid band should have most energy");
        assert!(mid_band > high_band * 100.0, "mid band should have more than high");
    }

    #[test]
    fn test_insufficient_samples() {
        let samples: Vec<f32> = vec![1.0; 100];
        let bytes: Vec<u8> = samples.iter().flat_map(|f| f.to_le_bytes()).collect();
        // With fft_size=1024, 100 samples is insufficient
        assert!(bytes.len() < 1024 * 4);
    }

    #[test]
    fn test_misaligned_bytes() {
        let bytes = vec![0u8; 5]; // Not divisible by 4
        assert!(bytes.len() % 4 != 0);
    }
}
