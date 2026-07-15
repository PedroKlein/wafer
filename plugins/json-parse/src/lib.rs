//! JSON parse transform plugin for WAFER pipeline.
//!
//! Validates JSON structure and pretty-prints with 2-space indentation.
//! No serde dependency — uses a manual parser/formatter to keep binary small.

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use exports::pipeline::node::transform::{OutputMessage, ProcessError};
use wafer_plugin::{bad_input, output_with_type, payload_as_str};

struct JsonParse;

impl exports::pipeline::node::lifecycle::Guest for JsonParse {
    fn validate(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(
        _config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for JsonParse {
    fn process(
        input: exports::pipeline::node::transform::Message,
    ) -> Result<
        exports::pipeline::node::transform::OutputMessage,
        exports::pipeline::node::transform::ProcessError,
    > {
        let text = payload_as_str!(&input)?;

        if text.is_empty() {
            return Err(bad_input!("empty payload"));
        }

        // Validate and pretty-print JSON without serde
        let formatted = match json_pretty_print(&text) {
            Ok(s) => s,
            Err(_) => return Err(bad_input!("payload is not valid JSON")),
        };

        Ok(output_with_type!(
            &input,
            formatted.into_bytes(),
            "application/json"
        ))
    }
}

// ---------------------------------------------------------------------------
// Manual JSON validator + pretty-printer
// ---------------------------------------------------------------------------

/// Validates JSON structure and produces a pretty-printed version with 2-space
/// indentation. Returns Err(()) if the input is not valid JSON.
fn json_pretty_print(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut out = String::with_capacity(len * 2);
    let mut indent: usize = 0;

    pos = skip_ws(bytes, pos);
    if pos >= len {
        return Err(());
    }

    pos = format_value(bytes, pos, &mut out, &mut indent)?;
    pos = skip_ws(bytes, pos);

    // Trailing content after root value is invalid
    if pos < len {
        return Err(());
    }

    Ok(out)
}

fn format_value(bytes: &[u8], mut pos: usize, out: &mut String, indent: &mut usize) -> Result<usize, ()> {
    pos = skip_ws(bytes, pos);
    if pos >= bytes.len() {
        return Err(());
    }

    match bytes[pos] {
        b'{' => format_object(bytes, pos, out, indent),
        b'[' => format_array(bytes, pos, out, indent),
        b'"' => format_string(bytes, pos, out),
        b't' => format_literal(bytes, pos, out, b"true"),
        b'f' => format_literal(bytes, pos, out, b"false"),
        b'n' => format_literal(bytes, pos, out, b"null"),
        b'-' | b'0'..=b'9' => format_number(bytes, pos, out),
        _ => Err(()),
    }
}

fn format_object(bytes: &[u8], mut pos: usize, out: &mut String, indent: &mut usize) -> Result<usize, ()> {
    out.push('{');
    pos += 1; // skip '{'
    pos = skip_ws(bytes, pos);

    if pos >= bytes.len() {
        return Err(());
    }

    if bytes[pos] == b'}' {
        out.push('}');
        return Ok(pos + 1);
    }

    *indent += 2;
    let mut first = true;

    loop {
        if !first {
            out.push(',');
        }
        first = false;

        out.push('\n');
        push_indent(out, *indent);

        // Key must be a string
        pos = skip_ws(bytes, pos);
        if pos >= bytes.len() || bytes[pos] != b'"' {
            return Err(());
        }
        pos = format_string(bytes, pos, out)?;

        pos = skip_ws(bytes, pos);
        if pos >= bytes.len() || bytes[pos] != b':' {
            return Err(());
        }
        pos += 1; // skip ':'
        out.push(':');
        out.push(' ');

        pos = format_value(bytes, pos, out, indent)?;
        pos = skip_ws(bytes, pos);

        if pos >= bytes.len() {
            return Err(());
        }

        match bytes[pos] {
            b',' => pos += 1,
            b'}' => break,
            _ => return Err(()),
        }
    }

    *indent -= 2;
    out.push('\n');
    push_indent(out, *indent);
    out.push('}');
    Ok(pos + 1)
}

fn format_array(bytes: &[u8], mut pos: usize, out: &mut String, indent: &mut usize) -> Result<usize, ()> {
    out.push('[');
    pos += 1; // skip '['
    pos = skip_ws(bytes, pos);

    if pos >= bytes.len() {
        return Err(());
    }

    if bytes[pos] == b']' {
        out.push(']');
        return Ok(pos + 1);
    }

    *indent += 2;
    let mut first = true;

    loop {
        if !first {
            out.push(',');
        }
        first = false;

        out.push('\n');
        push_indent(out, *indent);

        pos = format_value(bytes, pos, out, indent)?;
        pos = skip_ws(bytes, pos);

        if pos >= bytes.len() {
            return Err(());
        }

        match bytes[pos] {
            b',' => pos += 1,
            b']' => break,
            _ => return Err(()),
        }
    }

    *indent -= 2;
    out.push('\n');
    push_indent(out, *indent);
    out.push(']');
    Ok(pos + 1)
}

fn format_string(bytes: &[u8], mut pos: usize, out: &mut String) -> Result<usize, ()> {
    if pos >= bytes.len() || bytes[pos] != b'"' {
        return Err(());
    }

    out.push('"');
    pos += 1; // skip opening quote

    loop {
        if pos >= bytes.len() {
            return Err(()); // unterminated string
        }

        match bytes[pos] {
            b'"' => {
                out.push('"');
                return Ok(pos + 1);
            }
            b'\\' => {
                out.push('\\');
                pos += 1;
                if pos >= bytes.len() {
                    return Err(());
                }
                // Pass through the escaped character
                out.push(bytes[pos] as char);
                pos += 1;
            }
            ch => {
                out.push(ch as char);
                pos += 1;
            }
        }
    }
}

fn format_number(bytes: &[u8], mut pos: usize, out: &mut String) -> Result<usize, ()> {
    let start = pos;

    // Optional minus
    if pos < bytes.len() && bytes[pos] == b'-' {
        pos += 1;
    }

    // Integer part
    if pos >= bytes.len() {
        return Err(());
    }

    if bytes[pos] == b'0' {
        pos += 1;
    } else if bytes[pos].is_ascii_digit() {
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
    } else {
        return Err(());
    }

    // Fraction
    if pos < bytes.len() && bytes[pos] == b'.' {
        pos += 1;
        if pos >= bytes.len() || !bytes[pos].is_ascii_digit() {
            return Err(());
        }
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
    }

    // Exponent
    if pos < bytes.len() && (bytes[pos] == b'e' || bytes[pos] == b'E') {
        pos += 1;
        if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
            pos += 1;
        }
        if pos >= bytes.len() || !bytes[pos].is_ascii_digit() {
            return Err(());
        }
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
    }

    // We must have consumed at least one character beyond optional minus
    if pos == start || (pos == start + 1 && bytes[start] == b'-') {
        return Err(());
    }

    // Safety: we only accepted ASCII digits, '.', 'e', 'E', '+', '-'
    let num_str = core::str::from_utf8(&bytes[start..pos]).map_err(|_| ())?;
    out.push_str(num_str);
    Ok(pos)
}

fn format_literal(bytes: &[u8], pos: usize, out: &mut String, expected: &[u8]) -> Result<usize, ()> {
    let end = pos + expected.len();
    if end > bytes.len() || &bytes[pos..end] != expected {
        return Err(());
    }
    // Safety: expected is always valid ASCII
    let s = core::str::from_utf8(expected).map_err(|_| ())?;
    out.push_str(s);
    Ok(end)
}

fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

export!(JsonParse);
