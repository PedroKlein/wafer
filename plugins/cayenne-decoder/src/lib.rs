//! CayenneLPP binary decoder plugin for WAFER pipeline.
//!
//! Decodes LoRaWAN CayenneLPP binary payloads (TLV format) into structured JSON.
//!
//! Config: { "extra_types": { "200": { "name": "custom", "size": 4, "scale": 0.01 } } }
//! Output: { "sensors": [{ "channel": 1, "type": "temperature", "value": 25.5, "unit": "°C" }, ...] }

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use exports::pipeline::node::transform::{OutputMessage, ProcessError};
use serde::Deserialize;
use std::collections::HashMap;
use wafer_plugin::{bad_input, define_state, output_with_type, payload_bytes, set_state, with_state};

#[derive(Deserialize, Default)]
struct CayenneConfigInput {
    #[serde(default)]
    extra_types: HashMap<String, CustomTypeInput>,
}

#[derive(Deserialize)]
struct CustomTypeInput {
    name: String,
    size: usize,
    scale: f64,
}

struct CustomType {
    name: String,
    size: usize,
    scale: f64,
}

struct CayenneConfig {
    extra_types: HashMap<u8, CustomType>,
}

define_state!(CayenneConfig);

struct CayenneDecoder;

impl exports::pipeline::node::lifecycle::Guest for CayenneDecoder {
    fn validate(config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        if config.config.trim().is_empty() || config.config.trim() == "{}" {
            return None; // Empty config is valid
        }
        match serde_json::from_str::<CayenneConfigInput>(&config.config) {
            Ok(_) => None,
            Err(e) => Some(format!("config parse error: {e}")),
        }
    }

    fn init(
        config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        let input: CayenneConfigInput = if config.config.trim().is_empty() || config.config.trim() == "{}" {
            CayenneConfigInput::default()
        } else {
            serde_json::from_str(&config.config)
                .map_err(|e| exports::pipeline::node::lifecycle::ProcessError::BadInput(format!("config parse error: {e}")))?
        };

        let extra_types = input
            .extra_types
            .into_iter()
            .filter_map(|(k, v)| {
                k.parse::<u8>().ok().map(|type_id| {
                    (type_id, CustomType { name: v.name, size: v.size, scale: v.scale })
                })
            })
            .collect();

        set_state!(CayenneConfig { extra_types });
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for CayenneDecoder {
    fn process(
        input: exports::pipeline::node::transform::Message,
    ) -> Result<
        exports::pipeline::node::transform::OutputMessage,
        exports::pipeline::node::transform::ProcessError,
    > {
        let bytes = payload_bytes!(&input);

        if bytes.is_empty() {
            return Err(bad_input!("empty payload"));
        }

        with_state!(cfg => {
            let sensors = decode_cayenne_lpp(&bytes, &cfg.extra_types)?;
            let json = build_sensors_json(&sensors);

            Ok(output_with_type!(
                &input,
                json.into_bytes(),
                "application/json"
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// CayenneLPP decoder
// ---------------------------------------------------------------------------

struct SensorReading {
    channel: u8,
    type_name: String,
    value: SensorValue,
    unit: String,
}

enum SensorValue {
    Single(f64),
    Triple(f64, f64, f64),
}

fn decode_cayenne_lpp(data: &[u8], extra_types: &HashMap<u8, CustomType>) -> Result<Vec<SensorReading>, ProcessError> {
    let mut sensors = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        if pos + 2 > data.len() {
            return Err(bad_input!("truncated"));
        }

        let channel = data[pos];
        let type_id = data[pos + 1];
        pos += 2;

        match decode_standard_type(type_id, channel, data, &mut pos)? {
            Some(reading) => sensors.push(reading),
            None => {
                // Check extra_types
                if let Some(custom) = extra_types.get(&type_id) {
                    if pos + custom.size > data.len() {
                        return Err(bad_input!("truncated"));
                    }
                    let raw = read_signed_bytes(&data[pos..pos + custom.size]);
                    let value = raw as f64 * custom.scale;
                    pos += custom.size;
                    sensors.push(SensorReading {
                        channel,
                        type_name: custom.name.clone(),
                        value: SensorValue::Single(value),
                        unit: String::new(),
                    });
                } else {
                    // Unknown type: skip with "unknown" marker
                    // We can't know the size, so we stop parsing
                    sensors.push(SensorReading {
                        channel,
                        type_name: "unknown".to_string(),
                        value: SensorValue::Single(type_id as f64),
                        unit: String::new(),
                    });
                    break;
                }
            }
        }
    }

    Ok(sensors)
}

fn decode_standard_type(type_id: u8, channel: u8, data: &[u8], pos: &mut usize) -> Result<Option<SensorReading>, ProcessError> {
    let (name, unit, size) = match type_id {
        0x00 => ("digital_input", "", 1),
        0x01 => ("digital_output", "", 1),
        0x02 => ("analog_input", "V", 2),
        0x03 => ("analog_output", "V", 2),
        0x65 => ("luminosity", "lux", 2),
        0x66 => ("presence", "", 1),
        0x67 => ("temperature", "\u{00B0}C", 2),
        0x68 => ("humidity", "%", 1),
        0x71 => ("accelerometer", "g", 6),
        0x73 => ("barometer", "hPa", 2),
        0x86 => ("gyroscope", "\u{00B0}/s", 6),
        0x88 => ("gps", "", 9),
        _ => return Ok(None),
    };

    if *pos + size > data.len() {
        return Err(bad_input!("truncated"));
    }

    let value = match type_id {
        0x00 | 0x01 | 0x66 => SensorValue::Single(data[*pos] as f64),
        0x02 | 0x03 => {
            let raw = i16::from_be_bytes([data[*pos], data[*pos + 1]]);
            SensorValue::Single(raw as f64 / 100.0)
        }
        0x65 => {
            let raw = u16::from_be_bytes([data[*pos], data[*pos + 1]]);
            SensorValue::Single(raw as f64)
        }
        0x67 => {
            let raw = i16::from_be_bytes([data[*pos], data[*pos + 1]]);
            SensorValue::Single(raw as f64 / 10.0)
        }
        0x68 => SensorValue::Single(data[*pos] as f64 / 2.0),
        0x71 | 0x86 => {
            let x = i16::from_be_bytes([data[*pos], data[*pos + 1]]);
            let y = i16::from_be_bytes([data[*pos + 2], data[*pos + 3]]);
            let z = i16::from_be_bytes([data[*pos + 4], data[*pos + 5]]);
            let scale = if type_id == 0x71 { 1000.0 } else { 100.0 };
            SensorValue::Triple(x as f64 / scale, y as f64 / scale, z as f64 / scale)
        }
        0x73 => {
            let raw = u16::from_be_bytes([data[*pos], data[*pos + 1]]);
            SensorValue::Single(raw as f64 / 10.0)
        }
        0x88 => {
            let lat = read_i24_be(&data[*pos..*pos + 3]) as f64 / 10000.0;
            let lon = read_i24_be(&data[*pos + 3..*pos + 6]) as f64 / 10000.0;
            let alt = read_i24_be(&data[*pos + 6..*pos + 9]) as f64 / 100.0;
            SensorValue::Triple(lat, lon, alt)
        }
        _ => SensorValue::Single(0.0),
    };

    *pos += size;
    Ok(Some(SensorReading {
        channel,
        type_name: name.to_string(),
        value,
        unit: unit.to_string(),
    }))
}

fn read_i24_be(bytes: &[u8]) -> i32 {
    let raw = ((bytes[0] as i32) << 16) | ((bytes[1] as i32) << 8) | (bytes[2] as i32);
    // Sign extend from 24-bit
    if raw & 0x800000 != 0 {
        raw | !0xFFFFFF_i32
    } else {
        raw
    }
}

fn read_signed_bytes(bytes: &[u8]) -> i64 {
    match bytes.len() {
        1 => bytes[0] as i8 as i64,
        2 => i16::from_be_bytes([bytes[0], bytes[1]]) as i64,
        4 => i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64,
        _ => {
            let mut val: i64 = 0;
            for &b in bytes {
                val = (val << 8) | (b as i64);
            }
            val
        }
    }
}

// ---------------------------------------------------------------------------
// JSON output construction
// ---------------------------------------------------------------------------

fn build_sensors_json(sensors: &[SensorReading]) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("{\"sensors\": [");

    for (i, s) in sensors.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str("{\"channel\": ");
        push_u64(&mut out, s.channel as u64);
        out.push_str(", \"type\": \"");
        out.push_str(&s.type_name);
        out.push_str("\", \"value\": ");

        match &s.value {
            SensorValue::Single(v) => push_f64(&mut out, *v),
            SensorValue::Triple(x, y, z) => {
                out.push('[');
                push_f64(&mut out, *x);
                out.push_str(", ");
                push_f64(&mut out, *y);
                out.push_str(", ");
                push_f64(&mut out, *z);
                out.push(']');
            }
        }

        if !s.unit.is_empty() {
            out.push_str(", \"unit\": \"");
            out.push_str(&s.unit);
            out.push('"');
        }

        out.push('}');
    }

    out.push_str("]}");
    out
}

fn push_u64(out: &mut String, n: u64) {
    let s = format!("{n}");
    out.push_str(&s);
}

fn push_f64(out: &mut String, v: f64) {
    if v == v.floor() && v.abs() < 1e15 {
        let s = format!("{:.1}", v);
        out.push_str(&s);
    } else {
        let s = format!("{v}");
        out.push_str(&s);
    }
}

export!(CayenneDecoder);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_temperature_decode() {
        // Channel 1, Temperature (0x67), 25.5°C = 255 = 0x00FF
        let data = vec![0x01, 0x67, 0x00, 0xFF];
        let sensors = decode_cayenne_lpp(&data, &HashMap::new()).unwrap();
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].channel, 1);
        assert_eq!(sensors[0].type_name, "temperature");
        match sensors[0].value {
            SensorValue::Single(v) => assert!((v - 25.5).abs() < 0.01),
            _ => panic!("expected single value"),
        }
    }

    #[test]
    fn test_humidity_decode() {
        // Channel 2, Humidity (0x68), 64% = 128 / 2
        let data = vec![0x02, 0x68, 0x80];
        let sensors = decode_cayenne_lpp(&data, &HashMap::new()).unwrap();
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].channel, 2);
        assert_eq!(sensors[0].type_name, "humidity");
        match sensors[0].value {
            SensorValue::Single(v) => assert!((v - 64.0).abs() < 0.01),
            _ => panic!("expected single value"),
        }
    }

    #[test]
    fn test_multi_sensor() {
        // Temperature + Humidity in one payload
        let data = vec![
            0x01, 0x67, 0x00, 0xFF, // Ch1, temp, 25.5
            0x02, 0x68, 0x80,       // Ch2, humidity, 64%
        ];
        let sensors = decode_cayenne_lpp(&data, &HashMap::new()).unwrap();
        assert_eq!(sensors.len(), 2);
        assert_eq!(sensors[0].type_name, "temperature");
        assert_eq!(sensors[1].type_name, "humidity");
    }

    #[test]
    fn test_truncated_payload() {
        // Temperature type expects 2 bytes but only 1 available
        let data = vec![0x01, 0x67, 0x00]; // Missing second byte
        let result = decode_cayenne_lpp(&data, &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_type() {
        // Unknown type 0xFE
        let data = vec![0x01, 0xFE, 0x00];
        let sensors = decode_cayenne_lpp(&data, &HashMap::new()).unwrap();
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].type_name, "unknown");
    }

    #[test]
    fn test_custom_type() {
        let mut extra = HashMap::new();
        extra.insert(0xC8, CustomType {
            name: "custom_sensor".to_string(),
            size: 2,
            scale: 0.1,
        });

        // Channel 3, custom type 0xC8, value = 0x01F4 (500) → 50.0
        let data = vec![0x03, 0xC8, 0x01, 0xF4];
        let sensors = decode_cayenne_lpp(&data, &extra).unwrap();
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].type_name, "custom_sensor");
        match sensors[0].value {
            SensorValue::Single(v) => assert!((v - 50.0).abs() < 0.01),
            _ => panic!("expected single value"),
        }
    }
}
