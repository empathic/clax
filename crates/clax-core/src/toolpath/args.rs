//! The canonical argument hash (spec 2026-10-06-toolpath-audit-design
//! §12.2): a tool call's arguments in their JCS form (RFC 8785), hashed with
//! SHA-256. Clax computes it when a call is made, and a reader computes it
//! later from a transcript; both must agree byte for byte, so the vectors in
//! `tests/toolpath/args-hash-vectors.json` pin it.
//!
//! The canonical form:
//!
//! - object members sorted by their keys' UTF-16 code units (not code
//!   points: U+1F600 sorts before U+E000);
//! - no whitespace;
//! - numbers as IEEE 754 doubles in ECMAScript `Number.prototype.toString`
//!   form (`1.0` is `1`, `1e21` is `1e+21`, `-0` is `0`);
//! - strings escaped minimally: `\"`, `\\`, `\b`, `\f`, `\n`, `\r`, `\t`,
//!   other control characters as lowercase `\u00xx`, everything else as
//!   literal UTF-8.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// `"sha256:"` and the lowercase hex SHA-256 of `args`' canonical form. The
/// arguments are hashed as passed: nothing is dropped, defaulted or
/// normalized beyond the canonical form. Missing arguments are `{}`.
pub fn args_sha256(args: &Value) -> String {
    let mut h = Hasher::new();
    write(args, &mut h);
    h.finish()
}

/// [`args_sha256`] of the object with `members`, without copying them.
pub fn object_sha256(members: &Map<String, Value>) -> String {
    let mut h = Hasher::new();
    object(members, &mut h);
    h.finish()
}

/// `value` in its JCS canonical form (RFC 8785).
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// Where the canonical form goes: a string, or straight into a hash.
trait Sink {
    fn put(&mut self, s: &str) {
        self.put_bytes(s.as_bytes());
    }
    /// Bytes that are UTF-8: a whole canonical string.
    fn put_bytes(&mut self, b: &[u8]);
}

impl Sink for String {
    fn put_bytes(&mut self, b: &[u8]) {
        self.push_str(std::str::from_utf8(b).expect("canonical pieces are UTF-8"));
    }
}

/// A SHA-256 fed through a small buffer, so the many short pieces of a
/// canonical form cost one hash update per buffer.
struct Hasher(Sha256, Vec<u8>);

const HASH_BUF: usize = 16 << 10;

impl Hasher {
    fn new() -> Hasher {
        Hasher(Sha256::new(), Vec::with_capacity(HASH_BUF))
    }

    fn finish(mut self) -> String {
        self.0.update(&self.1);
        let digest = self.0.finalize();
        let mut out = String::with_capacity(7 + 64);
        out.push_str("sha256:");
        for b in digest {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }
}

impl Sink for Hasher {
    fn put_bytes(&mut self, b: &[u8]) {
        if self.1.len() + b.len() > HASH_BUF {
            self.0.update(&self.1);
            self.1.clear();
            if b.len() > HASH_BUF {
                self.0.update(b);
                return;
            }
        }
        self.1.extend_from_slice(b);
    }
}

fn write(value: &Value, out: &mut impl Sink) {
    match value {
        Value::Null => out.put("null"),
        Value::Bool(b) => out.put(if *b { "true" } else { "false" }),
        Value::Number(n) => number(n, out),
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.put("[");
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.put(",");
                }
                write(v, out);
            }
            out.put("]");
        }
        Value::Object(members) => object(members, out),
    }
}

fn object(members: &Map<String, Value>, out: &mut impl Sink) {
    let mut keys: Vec<&String> = members.keys().collect();
    keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    out.put("{");
    for (i, k) in keys.into_iter().enumerate() {
        if i > 0 {
            out.put(",");
        }
        string(k, out);
        out.put(":");
        write(&members[k], out);
    }
    out.put("}");
}

/// A JSON number as the double it denotes, in ECMAScript form. JSON holds
/// no NaN or infinity, so every value is finite.
fn number(n: &serde_json::Number, out: &mut impl Sink) {
    let f = n.as_f64().unwrap_or(0.0);
    if f == 0.0 {
        // Both zeros print as `0`.
        out.put("0");
        return;
    }
    let mut buf = ryu_js::Buffer::new();
    out.put(buf.format_finite(f));
}

/// A string with JCS's minimal escapes.
fn string(s: &str, out: &mut impl Sink) {
    let mut buf = Vec::with_capacity(s.len() + 2);
    escape_into(s, &mut buf);
    out.put_bytes(&buf);
}

/// Appends `s` quoted, with JCS's minimal escapes, to `buf`. Escaped bytes
/// are all ASCII, so the other bytes copy through as UTF-8.
fn escape_into(s: &str, buf: &mut Vec<u8>) {
    buf.push(b'"');
    let mut rest = s.as_bytes();
    while let Some(i) = rest
        .iter()
        .position(|&b| b < 0x20 || b == b'"' || b == b'\\')
    {
        buf.extend_from_slice(&rest[..i]);
        let b = rest[i];
        rest = &rest[i + 1..];
        match b {
            b'"' => buf.extend_from_slice(b"\\\""),
            b'\\' => buf.extend_from_slice(b"\\\\"),
            0x08 => buf.extend_from_slice(b"\\b"),
            0x0C => buf.extend_from_slice(b"\\f"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            b'\r' => buf.extend_from_slice(b"\\r"),
            b'\t' => buf.extend_from_slice(b"\\t"),
            0x00..=0x1F => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                buf.extend_from_slice(b"\\u00");
                buf.push(HEX[usize::from(b >> 4)]);
                buf.push(HEX[usize::from(b & 0xF)]);
            }
            _ => buf.push(b),
        }
    }
    buf.extend_from_slice(rest);
    buf.push(b'"');
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// The §12.2 vectors, shared with the Pi extension's tests.
    pub(crate) fn vectors() -> Vec<Value> {
        serde_json::from_str(include_str!("../../tests/toolpath/args-hash-vectors.json"))
            .expect("vectors file is JSON")
    }

    #[test]
    fn args_hash_vectors() {
        let vectors = vectors();
        assert_eq!(vectors.len(), 7);
        for v in &vectors {
            let n = &v["n"];
            let args: Value =
                serde_json::from_str(v["arguments"].as_str().unwrap()).expect("arguments parse");
            assert_eq!(
                canonical(&args),
                v["canonical"].as_str().unwrap(),
                "vector {n}"
            );
            assert_eq!(
                args_sha256(&args),
                v["args_sha256"].as_str().unwrap(),
                "vector {n}"
            );
            // The canonical form is a fixed point.
            let again: Value = serde_json::from_str(&canonical(&args)).unwrap();
            assert_eq!(canonical(&again), canonical(&args), "vector {n}");
        }
    }

    #[test]
    fn numbers_take_their_ecmascript_form() {
        for (input, want) in [
            ("0", "0"),
            ("-0", "0"),
            ("-0.0", "0"),
            ("1.5", "1.5"),
            ("1e21", "1e+21"),
            ("1e-7", "1e-7"),
            ("0.000001", "0.000001"),
            ("123456789012345680000", "123456789012345680000"),
            ("9007199254740993", "9007199254740992"),
            ("-9223372036854775808", "-9223372036854776000"),
            ("18446744073709551615", "18446744073709552000"),
            ("5e-324", "5e-324"),
            ("1.7976931348623157e308", "1.7976931348623157e+308"),
        ] {
            let v: Value = serde_json::from_str(input).unwrap();
            assert_eq!(canonical(&v), want, "{input}");
        }
    }

    #[test]
    fn strings_escape_minimally() {
        let v = json!("\u{7f}\u{2028}é\u{0}\u{1b}/<>");
        assert_eq!(canonical(&v), "\"\u{7f}\u{2028}é\\u0000\\u001b/<>\"");
        assert_eq!(canonical(&json!("\u{8}\u{c}")), r#""\b\f""#);
    }

    #[test]
    fn nested_objects_sort_at_every_level() {
        let v = json!({"b": {"y": 1, "x": [{"d": 1, "c": 2}]}, "a": null});
        assert_eq!(
            canonical(&v),
            r#"{"a":null,"b":{"x":[{"c":2,"d":1}],"y":1}}"#
        );
    }
}
