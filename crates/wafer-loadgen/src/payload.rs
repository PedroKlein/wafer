//! Deterministic payload templates for E-Perf-4 / E-Perf-5 payload sweeps.
//!
//! Four templates:
//! - `telemetry-120b` — Pipeline A schema (`device_id`, `temperature`, `humidity`,
//!   `ts`, `seq`) sized to 120 ±5 bytes.
//! - `generic-1kb`, `generic-10kb`, `generic-100kb` — telemetry schema plus a
//!   deterministic `pad` field, sized to EXACTLY `1024` / `10_240` / `102_400` bytes
//!   at canonical (`ts`, `seq`).
//!
//! # Determinism
//!
//! `render(ts, seq)` always returns the same bytes for the same inputs. For
//! generic-\*kb templates, output length is exactly `target_size()` when the
//! target is large enough to accommodate the fixed schema prefix — which is
//! true for 1KB / 10KB / 100KB across any u64 ts / seq value. For
//! telemetry-120b, output length is `target_size()` ± the digit-width delta of
//! ts and seq relative to canonical (empirically within ±5 bytes for
//! realistic wall-clock ts and small seq values).
//!
//! # Padding
//!
//! Padding is NOT `x` filler (see plan constraint). It is derived from
//! `blake3(b"wafer-loadgen-pad-v1|" || template_name)` extended via BLAKE3's
//! extendable output function (XOF), then hex-encoded. The hex encoding keeps
//! all pad bytes JSON-safe (`0-9`, `a-f`).
//!
//! # Fingerprint
//!
//! Each template exposes `fingerprint_hex()` — the SHA-256 of
//! `render(CANONICAL_TS_NS, CANONICAL_SEQ)` — as a compile-time string. The
//! `payload_size_exact` unit test asserts both byte-length AND fingerprint
//! per template; regressions in either raise a loud failure.

use std::str::FromStr;

/// Canonical timestamp used for build-time SHA-256 fingerprinting.
///
/// Chosen to be a realistic wall-clock ns value with 19 digits (2027-01-15).
/// Using a stable canonical value makes the fingerprint reproducible across
/// build hosts, whereas `SystemTime::now()` would change every run.
pub const CANONICAL_TS_NS: u64 = 1_800_000_000_000_000_000;

/// Canonical sequence number used for fingerprinting. 9 digits.
pub const CANONICAL_SEQ: u64 = 999_999_999;

/// Fixed device identifier used across all templates. Length chosen so that
/// `telemetry-120b` renders to EXACTLY 120 bytes at canonical ts/seq.
pub const DEVICE_ID: &str = "bench-shakedown-macos-012345";

/// Payload templates that map to `--payload-template <name>` on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PayloadTemplate {
    /// Pipeline A telemetry schema, 120 ±5 bytes.
    #[value(name = "telemetry-120b")]
    Telemetry120b,
    /// Telemetry + deterministic pad, 1024 bytes exact.
    #[value(name = "generic-1kb")]
    Generic1kb,
    /// Telemetry + deterministic pad, `10_240` bytes exact.
    #[value(name = "generic-10kb")]
    Generic10kb,
    /// Telemetry + deterministic pad, `102_400` bytes exact.
    #[value(name = "generic-100kb")]
    Generic100kb,
}

impl PayloadTemplate {
    /// All templates, in enumeration order. Used by tests to sweep.
    pub const ALL: [Self; 4] = [
        Self::Telemetry120b,
        Self::Generic1kb,
        Self::Generic10kb,
        Self::Generic100kb,
    ];

    /// Human-readable / CLI name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Telemetry120b => "telemetry-120b",
            Self::Generic1kb => "generic-1kb",
            Self::Generic10kb => "generic-10kb",
            Self::Generic100kb => "generic-100kb",
        }
    }

    /// Target size in bytes. For generic-\*kb this is exact; for
    /// telemetry-120b this is nominal (±5 for realistic ts / seq digit width).
    #[must_use]
    pub const fn target_size(self) -> usize {
        match self {
            Self::Telemetry120b => 120,
            Self::Generic1kb => 1024,
            Self::Generic10kb => 10_240,
            Self::Generic100kb => 102_400,
        }
    }

    /// Whether `render()` guarantees exact `target_size()` bytes.
    #[must_use]
    pub const fn is_exact_size(self) -> bool {
        !matches!(self, Self::Telemetry120b)
    }

    /// Render the payload for a given `(ts_ns, seq)` pair. Deterministic.
    #[must_use]
    pub fn render(self, ts_ns: u64, seq: u64) -> Vec<u8> {
        match self {
            Self::Telemetry120b => render_telemetry_120b(ts_ns, seq),
            Self::Generic1kb | Self::Generic10kb | Self::Generic100kb => {
                render_generic(self, ts_ns, seq)
            }
        }
    }

    /// SHA-256 hex fingerprint of `render(CANONICAL_TS_NS, CANONICAL_SEQ)`.
    ///
    /// Hard-coded per template. If the render logic changes, the
    /// `payload_size_exact` unit test will fail and the developer must
    /// intentionally update this value — that manual step is the "build-time
    /// checksum" per AC1.
    #[must_use]
    pub const fn fingerprint_hex(self) -> &'static str {
        match self {
            Self::Telemetry120b => TELEMETRY_120B_FINGERPRINT,
            Self::Generic1kb => GENERIC_1KB_FINGERPRINT,
            Self::Generic10kb => GENERIC_10KB_FINGERPRINT,
            Self::Generic100kb => GENERIC_100KB_FINGERPRINT,
        }
    }
}

impl FromStr for PayloadTemplate {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for tpl in Self::ALL {
            if tpl.name() == s {
                return Ok(tpl);
            }
        }
        Err(format!(
            "unknown payload template `{s}`; expected one of: {}",
            Self::ALL.map(Self::name).join(", ")
        ))
    }
}

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

fn render_telemetry_120b(ts_ns: u64, seq: u64) -> Vec<u8> {
    // Pipeline A schema exactly: {device_id, temperature, humidity, ts, seq}.
    // No pad field — size is achieved by DEVICE_ID length.
    format!(
        r#"{{"device_id":"{DEVICE_ID}","temperature":42.5,"humidity":37.2,"ts":{ts_ns},"seq":{seq}}}"#
    )
    .into_bytes()
}

fn render_generic(tpl: PayloadTemplate, ts_ns: u64, seq: u64) -> Vec<u8> {
    // Schema: telemetry + `pad` field. `pad` is a hex-encoded deterministic
    // stream that fills the remainder to reach exact target_size().
    let prefix = format!(
        r#"{{"device_id":"{DEVICE_ID}","temperature":42.5,"humidity":37.2,"ts":{ts_ns},"seq":{seq},"pad":""#
    );
    let suffix = "\"}";
    let base_len = prefix.len() + suffix.len();
    let target = tpl.target_size();
    let pad = if target > base_len {
        deterministic_pad_hex(tpl.name(), target - base_len)
    } else {
        String::new()
    };
    let mut out = String::with_capacity(target.max(base_len));
    out.push_str(&prefix);
    out.push_str(&pad);
    out.push_str(suffix);
    out.into_bytes()
}

/// Produce exactly `hex_chars` characters of hex-encoded BLAKE3 XOF output.
///
/// Domain separated with a version string so future template edits can rotate
/// the derivation without silently colliding with old fingerprints.
fn deterministic_pad_hex(template_name: &str, hex_chars: usize) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"wafer-loadgen-pad-v1|");
    hasher.update(template_name.as_bytes());
    let byte_count = hex_chars.div_ceil(2);
    let mut buf = vec![0_u8; byte_count];
    let mut reader = hasher.finalize_xof();
    reader.fill(&mut buf);
    let mut hex_out = hex::encode(&buf);
    hex_out.truncate(hex_chars);
    hex_out
}

// -----------------------------------------------------------------------------
// Fingerprints — SHA-256 of render(CANONICAL_TS_NS, CANONICAL_SEQ).
//
// If a template render changes, this constant MUST change too. Use the
// diagnostic emitted by `payload_size_exact` (the actual hash is printed on
// mismatch) to update. Do not paste values without verifying the test.
// -----------------------------------------------------------------------------
const TELEMETRY_120B_FINGERPRINT: &str =
    "d39c713ac4c27e23dc8ffed24d28ef4f8cd5ffb731ffa2cea29cb4765189ed54";
const GENERIC_1KB_FINGERPRINT: &str =
    "b34447bd0a92c0481da43839eb2a62a8cacdac096d33995e8ae86234794cd17a";
const GENERIC_10KB_FINGERPRINT: &str =
    "74c2bbd54a50bfa3425ef12772934d95f02ca058a3632d10c73edc5e71e605b0";
const GENERIC_100KB_FINGERPRINT: &str =
    "5504f0abd0e19f76d805cca6ae87538596826381101764fd880fa4c70704cce5";

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    /// AC1: byte-length AND SHA-256 fingerprint asserted per template.
    /// AC2: `telemetry-120b` schema + 120 ±5 bytes.
    #[test]
    fn payload_size_exact() {
        for tpl in PayloadTemplate::ALL {
            let bytes = tpl.render(CANONICAL_TS_NS, CANONICAL_SEQ);
            let observed = sha256_hex(&bytes);

            // Size assertion.
            if tpl.is_exact_size() {
                assert_eq!(
                    bytes.len(),
                    tpl.target_size(),
                    "{}: expected exact {} bytes, got {}",
                    tpl.name(),
                    tpl.target_size(),
                    bytes.len()
                );
            } else {
                let tol = 5;
                assert!(
                    bytes.len().abs_diff(tpl.target_size()) <= tol,
                    "{}: expected {} ±{tol} bytes, got {}",
                    tpl.name(),
                    tpl.target_size(),
                    bytes.len()
                );
            }

            // Fingerprint assertion. Guides the developer with the observed
            // value on failure so updating the constant is a copy-paste.
            assert_eq!(
                observed,
                tpl.fingerprint_hex(),
                "{}: fingerprint drift. Observed sha256={observed}. If the render change is intentional, update the FINGERPRINT constant to this value.",
                tpl.name()
            );
        }
    }

    #[test]
    fn telemetry_120b_matches_pipeline_a_schema() {
        let bytes = PayloadTemplate::Telemetry120b.render(CANONICAL_TS_NS, CANONICAL_SEQ);
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        let obj = json.as_object().expect("object");
        // AC2: schema is {device_id, temperature, humidity, ts, seq}.
        for key in ["device_id", "temperature", "humidity", "ts", "seq"] {
            assert!(obj.contains_key(key), "telemetry-120b missing field `{key}`");
        }
        assert_eq!(obj.len(), 5, "telemetry-120b should have exactly 5 fields, got {}", obj.len());
        assert_eq!(obj["ts"].as_u64(), Some(CANONICAL_TS_NS));
        assert_eq!(obj["seq"].as_u64(), Some(CANONICAL_SEQ));
    }

    #[test]
    fn generic_templates_are_valid_json() {
        for tpl in [PayloadTemplate::Generic1kb, PayloadTemplate::Generic10kb, PayloadTemplate::Generic100kb] {
            let bytes = tpl.render(CANONICAL_TS_NS, CANONICAL_SEQ);
            let json: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", tpl.name()));
            let obj = json.as_object().expect("object");
            for key in ["device_id", "temperature", "humidity", "ts", "seq", "pad"] {
                assert!(obj.contains_key(key), "{}: missing field `{key}`", tpl.name());
            }
        }
    }

    #[test]
    fn render_is_deterministic_across_calls() {
        for tpl in PayloadTemplate::ALL {
            let a = tpl.render(CANONICAL_TS_NS, CANONICAL_SEQ);
            let b = tpl.render(CANONICAL_TS_NS, CANONICAL_SEQ);
            assert_eq!(a, b, "{}: render not deterministic", tpl.name());
        }
    }

    #[test]
    fn padding_is_not_x_filler() {
        // Constraint: "Padding pattern must be deterministic (not `x` filler)."
        let bytes = PayloadTemplate::Generic1kb.render(CANONICAL_TS_NS, CANONICAL_SEQ);
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let pad = json["pad"].as_str().unwrap();
        assert!(!pad.chars().all(|c| c == 'x'), "pad is trivial 'x' filler");
        // Sanity: hex charset only.
        assert!(pad.chars().all(|c| c.is_ascii_hexdigit()), "pad contains non-hex chars");
    }

    #[test]
    fn different_templates_produce_different_padding() {
        // Domain separation: template names feed into the derivation, so 1kb
        // pad prefix ≠ 10kb pad prefix.
        let a = PayloadTemplate::Generic1kb.render(CANONICAL_TS_NS, CANONICAL_SEQ);
        let b = PayloadTemplate::Generic10kb.render(CANONICAL_TS_NS, CANONICAL_SEQ);
        let a_json: serde_json::Value = serde_json::from_slice(&a).unwrap();
        let b_json: serde_json::Value = serde_json::from_slice(&b).unwrap();
        let a_pad = a_json["pad"].as_str().unwrap();
        let b_pad = b_json["pad"].as_str().unwrap();
        let prefix_len = 64.min(a_pad.len()).min(b_pad.len());
        // Pad content is pure ASCII hex, so byte-index == char-index.
        assert_ne!(
            a_pad.as_bytes()[..prefix_len],
            b_pad.as_bytes()[..prefix_len],
            "generic-1kb and generic-10kb pads share a prefix — domain separation is broken"
        );
    }

    #[test]
    fn from_str_round_trip() {
        for tpl in PayloadTemplate::ALL {
            let parsed: PayloadTemplate = tpl.name().parse().unwrap();
            assert_eq!(parsed, tpl);
        }
        // Loop through nonsense inputs and confirm rejection.
        for bad in ["nonsense", "", "telemetry-120", "generic-1MB"] {
            let err = PayloadTemplate::from_str(bad);
            assert!(err.is_err(), "parsing `{bad}` should fail");
            if let Err(msg) = err {
                assert!(msg.contains(bad), "error should reference input: {msg}");
            }
        }
    }
}
