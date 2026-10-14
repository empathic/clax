//! pkt-line framing: a 4-hex-digit length (itself included) then the
//! payload; `0000` flush, `0001` delimiter and `0002` response end carry
//! none.

use crate::error::{GitError, Result};

/// The flush packet.
pub const FLUSH: &[u8] = b"0000";
/// The largest payload one packet carries (65520 bytes less the length).
pub const MAX_PAYLOAD: usize = 65516;

/// One packet of a pkt-line stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet<'a> {
    Data(&'a [u8]),
    Flush,
    Delim,
    ResponseEnd,
}

impl<'a> Packet<'a> {
    /// The payload as text without its trailing newline (lossy), or `None`
    /// for a special packet.
    pub fn text(&self) -> Option<String> {
        match self {
            Packet::Data(d) => {
                let d = d.strip_suffix(b"\n").unwrap_or(d);
                Some(String::from_utf8_lossy(d).into_owned())
            }
            _ => None,
        }
    }
}

/// Appends a packet holding `payload` to `out`.
pub fn write(out: &mut Vec<u8>, payload: &[u8]) {
    debug_assert!(payload.len() <= MAX_PAYLOAD);
    out.extend_from_slice(format!("{:04x}", payload.len() + 4).as_bytes());
    out.extend_from_slice(payload);
}

/// A packet holding `payload`.
pub fn line(payload: impl AsRef<[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, payload.as_ref());
    out
}

/// Splits `data` into packets, and answers them with the byte offset just
/// past the last one read. Reading stops at the end of `data` or, when
/// `stop_at_flush`, after the first flush (what follows, such as a pack, is
/// left to the caller).
pub fn read(data: &[u8], stop_at_flush: bool) -> Result<(Vec<Packet<'_>>, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= data.len() {
        let len = std::str::from_utf8(&data[at..at + 4])
            .ok()
            .and_then(|h| usize::from_str_radix(h, 16).ok())
            .ok_or_else(|| GitError::unavailable("Malformed pkt-line"))?;
        match len {
            0 => {
                out.push(Packet::Flush);
                at += 4;
                if stop_at_flush {
                    return Ok((out, at));
                }
                continue;
            }
            1 => out.push(Packet::Delim),
            2 => out.push(Packet::ResponseEnd),
            3 => return Err(GitError::unavailable("Malformed pkt-line length 3")),
            _ => {
                if at + len > data.len() {
                    return Err(GitError::unavailable("Truncated pkt-line"));
                }
                out.push(Packet::Data(&data[at + 4..at + len]));
                at += len;
                continue;
            }
        }
        at += 4;
    }
    Ok((out, at))
}

/// Every packet of `data`, which must be whole packets.
pub fn read_all(data: &[u8]) -> Result<Vec<Packet<'_>>> {
    let (packets, end) = read(data, false)?;
    if end != data.len() {
        return Err(GitError::unavailable("Truncated pkt-line"));
    }
    Ok(packets)
}

/// `data` on side-band channel `band` (1 data, 2 progress, 3 error), in
/// packets of at most `max_payload` bytes (band byte included).
pub fn write_sideband(out: &mut Vec<u8>, band: u8, data: &[u8], max_payload: usize) {
    for chunk in data.chunks(max_payload - 1) {
        let mut p = Vec::with_capacity(chunk.len() + 1);
        p.push(band);
        p.extend_from_slice(chunk);
        write(out, &p);
    }
}
