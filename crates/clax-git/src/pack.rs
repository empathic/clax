//! Packfiles, version 2. Writing stores each object whole (undeltified):
//! the header, each object as its type and size varint followed by its
//! zlib-deflated content, and a trailing SHA-1 of everything before it.
//! Reading takes what git sends: whole objects, `OFS_DELTA` and
//! `REF_DELTA` entries, and thin packs whose deltas name bases outside the
//! pack, which the caller supplies.

use std::collections::HashMap;

use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};

use crate::error::{GitError, Result};
use crate::id::ObjectId;
use crate::object::{Object, ObjectKind};

/// The zlib compression level packs are written with (git's default).
const LEVEL: u8 = 6;

/// zlib (RFC 1950) compression of `bytes`.
pub fn deflate_zlib(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(bytes, LEVEL)
}

fn entry_header(kind: ObjectKind, size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut byte = (kind.pack_code() << 4) | (size & 0x0f) as u8;
    let mut rest = size >> 4;
    while rest > 0 {
        out.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    out.push(byte);
    out
}

/// A packfile holding `objects`, each stored whole.
pub fn write_pack<'a>(objects: impl IntoIterator<Item = &'a Object>) -> Vec<u8> {
    let objects: Vec<&Object> = objects.into_iter().collect();
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for o in objects {
        out.extend_from_slice(&entry_header(o.kind, o.data.len()));
        out.extend_from_slice(&deflate_zlib(&o.data));
    }
    let sum = ObjectId::sha1(&out);
    out.extend_from_slice(sum.as_bytes());
    out
}

// ── Reading ─────────────────────────────────────────────────────────────────

/// The most bytes one pack entry may inflate to, by default.
pub const DEFAULT_MAX_OBJECT_BYTES: usize = 256 * 1024 * 1024;

/// The most bytes a whole pack may unpack to, by default: its entries
/// inflated plus the objects its deltas make.
pub const DEFAULT_MAX_UNPACKED_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
enum Entry {
    Whole(Object),
    /// A delta on the entry at that byte offset.
    OfsDelta {
        base_offset: usize,
        delta: Vec<u8>,
    },
    /// A delta on the object with that ID, in the pack or not.
    RefDelta {
        base: ObjectId,
        delta: Vec<u8>,
    },
}

/// A pack's entries as read, before deltas are resolved.
#[derive(Clone, Debug)]
pub struct ParsedPack {
    /// By byte offset of each entry, in pack order.
    entries: Vec<(usize, Entry)>,
    /// The bytes the entries inflated to.
    inflated: usize,
    /// The most bytes the entries and the objects deltas make may total.
    max_total_bytes: usize,
}

fn bad(what: &str) -> GitError {
    GitError::invalid(format!("The pack is malformed: {what}"))
}

/// Inflates the zlib stream at the start of `input` into exactly `size`
/// bytes; answers them and the compressed length. The output grows as the
/// stream inflates, so a declared size the stream does not hold is never
/// allocated.
fn inflate_exact(input: &[u8], size: usize) -> Result<(Vec<u8>, usize)> {
    const FIRST: usize = 64 * 1024;
    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut out = Vec::new();
    let mut written = 0;
    let mut consumed = 0;
    // One spare byte so a stream longer than declared is caught.
    let mut spare = [0u8; 1];
    loop {
        if written < size && written == out.len() {
            let grow = (size - written).min(written.max(FIRST));
            out.resize(written + grow, 0);
        }
        let target: &mut [u8] = if written < size {
            &mut out[written..]
        } else {
            &mut spare
        };
        let r = inflate(&mut state, &input[consumed..], target, MZFlush::None);
        consumed += r.bytes_consumed;
        if written >= size && r.bytes_written > 0 {
            return Err(bad("an entry inflates past its declared size"));
        }
        written += r.bytes_written;
        match r.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) if r.bytes_consumed == 0 && r.bytes_written == 0 => {
                return Err(bad("an entry is truncated"));
            }
            Ok(_) => continue,
            Err(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => continue,
            Err(_) => return Err(bad("an entry does not inflate")),
        }
    }
    if written != size {
        return Err(bad("an entry inflates short of its declared size"));
    }
    Ok((out, consumed))
}

fn over_total(max_total_bytes: usize) -> GitError {
    GitError::invalid(format!(
        "A pushed pack unpacks to over {max_total_bytes} bytes"
    ))
}

impl ParsedPack {
    /// Reads a whole pack: checks its header and trailing SHA-1 and inflates
    /// every entry. `max_object_bytes` bounds each entry's inflated size
    /// (and each delta's result when resolved); the whole pack may unpack
    /// to [`DEFAULT_MAX_UNPACKED_BYTES`].
    pub fn parse(pack: &[u8], max_object_bytes: usize) -> Result<ParsedPack> {
        ParsedPack::parse_bounded(pack, max_object_bytes, DEFAULT_MAX_UNPACKED_BYTES)
    }

    /// [`ParsedPack::parse`], with the entries inflated plus the objects
    /// [`ParsedPack::resolve`] makes from deltas bounded by
    /// `max_total_bytes`: past it, either refuses the pack. (Resolving also
    /// copies each whole entry once, so memory peaks near twice the bound.)
    pub fn parse_bounded(
        pack: &[u8],
        max_object_bytes: usize,
        max_total_bytes: usize,
    ) -> Result<ParsedPack> {
        if pack.len() < 32 || &pack[..4] != b"PACK" {
            return Err(bad("no PACK header"));
        }
        let version = u32::from_be_bytes(pack[4..8].try_into().expect("4 bytes"));
        if version != 2 && version != 3 {
            return Err(bad("unsupported version"));
        }
        let count = u32::from_be_bytes(pack[8..12].try_into().expect("4 bytes")) as usize;
        let body_end = pack.len() - 20;
        if ObjectId::sha1(&pack[..body_end]).as_bytes() != &pack[body_end..] {
            return Err(bad("its checksum does not match"));
        }
        let mut entries = Vec::with_capacity(count.min(1 << 16));
        let mut inflated = 0usize;
        let mut at = 12;
        for _ in 0..count {
            let start = at;
            let byte = |i: usize| {
                pack.get(i)
                    .copied()
                    .filter(|_| i < body_end)
                    .ok_or_else(|| bad("truncated"))
            };
            let mut c = byte(at)?;
            at += 1;
            let code = (c >> 4) & 7;
            let mut size = (c & 0x0f) as usize;
            let mut shift = 4;
            while c & 0x80 != 0 {
                c = byte(at)?;
                at += 1;
                if shift > 56 {
                    return Err(bad("an entry size overflows"));
                }
                size |= ((c & 0x7f) as usize) << shift;
                shift += 7;
            }
            if size > max_object_bytes {
                return Err(GitError::invalid(format!(
                    "A pushed object is over {max_object_bytes} bytes"
                )));
            }
            inflated = inflated
                .checked_add(size)
                .filter(|&t| t <= max_total_bytes)
                .ok_or_else(|| over_total(max_total_bytes))?;
            let entry = match code {
                1..=4 => {
                    let kind = ObjectKind::from_pack_code(code).expect("1..=4");
                    let (data, used) = inflate_exact(&pack[at..body_end], size)?;
                    at += used;
                    Entry::Whole(Object::new(kind, data))
                }
                6 => {
                    let mut c = byte(at)?;
                    at += 1;
                    let mut back = (c & 0x7f) as usize;
                    while c & 0x80 != 0 {
                        c = byte(at)?;
                        at += 1;
                        back = back
                            .checked_add(1)
                            .and_then(|b| b.checked_mul(128))
                            .ok_or_else(|| bad("offset overflows"))?
                            | (c & 0x7f) as usize;
                    }
                    let base_offset = start
                        .checked_sub(back)
                        .filter(|_| back > 0)
                        .ok_or_else(|| bad("a delta's base offset is out of range"))?;
                    let (delta, used) = inflate_exact(&pack[at..body_end], size)?;
                    at += used;
                    Entry::OfsDelta { base_offset, delta }
                }
                7 => {
                    if at + 20 > body_end {
                        return Err(bad("truncated"));
                    }
                    let base = ObjectId::from_bytes(&pack[at..at + 20]).expect("20 bytes");
                    at += 20;
                    let (delta, used) = inflate_exact(&pack[at..body_end], size)?;
                    at += used;
                    Entry::RefDelta { base, delta }
                }
                _ => return Err(bad("an entry has an unknown type")),
            };
            entries.push((start, entry));
        }
        if at != body_end {
            return Err(bad("bytes follow the last entry"));
        }
        Ok(ParsedPack {
            entries,
            inflated,
            max_total_bytes,
        })
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The objects of the pack with every delta applied. A `REF_DELTA` base
    /// that is not in the pack is looked up in `external`; the bases missing
    /// from both are answered as `Resolved::Missing` so the caller can fetch them
    /// and call again. Each delta is applied once its base is known, bases
    /// first, so the work is linear in the pack whatever order its deltas
    /// come in.
    pub fn resolve(
        &self,
        external: &HashMap<ObjectId, Object>,
        max_object_bytes: usize,
    ) -> Result<Resolved> {
        let n = self.entries.len();
        let index: HashMap<usize, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, (off, _))| (*off, i))
            .collect();
        // The deltas waiting on each base: an entry (by index) or an ID.
        let mut on_entry: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut on_id: HashMap<ObjectId, Vec<usize>> = HashMap::new();
        let mut resolved: Vec<Option<Object>> = vec![None; n];
        let mut ready: Vec<usize> = Vec::new();
        let mut budget = self.max_total_bytes.saturating_sub(self.inflated);
        let mut make = |base: &Object, delta: &[u8]| -> Result<Object> {
            let size = delta_result_size(delta)?;
            if size > max_object_bytes {
                return Err(GitError::invalid(format!(
                    "A pushed object is over {max_object_bytes} bytes"
                )));
            }
            if size > budget {
                return Err(over_total(self.max_total_bytes));
            }
            let data = apply_delta(&base.data, delta, size)?;
            budget -= data.len();
            Ok(Object::new(base.kind, data))
        };
        for (i, (_, e)) in self.entries.iter().enumerate() {
            match e {
                Entry::Whole(o) => {
                    resolved[i] = Some(o.clone());
                    ready.push(i);
                }
                Entry::OfsDelta { base_offset, .. } => {
                    let base = index
                        .get(base_offset)
                        .ok_or_else(|| bad("a delta's base offset names no entry"))?;
                    on_entry.entry(*base).or_default().push(i);
                }
                Entry::RefDelta { base, delta } => match external.get(base) {
                    Some(b) => {
                        resolved[i] = Some(make(b, delta)?);
                        ready.push(i);
                    }
                    None => on_id.entry(*base).or_default().push(i),
                },
            }
        }
        while let Some(i) = ready.pop() {
            let base = resolved[i].as_ref().expect("ready entries are resolved");
            let id = base.id();
            let mut children = on_entry.remove(&i).unwrap_or_default();
            children.extend(on_id.remove(&id).unwrap_or_default());
            let mut made = Vec::with_capacity(children.len());
            for c in children {
                if resolved[c].is_some() {
                    continue;
                }
                let delta = match &self.entries[c].1 {
                    Entry::OfsDelta { delta, .. } | Entry::RefDelta { delta, .. } => delta,
                    Entry::Whole(_) => unreachable!("only deltas wait on a base"),
                };
                made.push((c, make(base, delta)?));
            }
            for (c, o) in made {
                resolved[c] = Some(o);
                ready.push(c);
            }
        }
        if resolved.iter().any(Option::is_none) {
            // What still waits on an ID waits on an object neither in the
            // pack nor in `external`; on an entry, on one of those.
            let mut missing: Vec<ObjectId> = on_id.into_keys().collect();
            if missing.is_empty() {
                return Err(bad("a delta chain does not resolve"));
            }
            missing.sort();
            return Ok(Resolved::Missing(missing));
        }
        Ok(Resolved::Objects(
            resolved.into_iter().map(|o| o.expect("resolved")).collect(),
        ))
    }
}

/// What [`ParsedPack::resolve`] answers.
#[derive(Debug)]
pub enum Resolved {
    /// Every object, in pack order.
    Objects(Vec<Object>),
    /// Delta bases neither in the pack nor in `external`.
    Missing(Vec<ObjectId>),
}

fn varint(delta: &[u8], at: &mut usize) -> Result<usize> {
    let mut out = 0usize;
    let mut shift = 0;
    loop {
        let c = *delta.get(*at).ok_or_else(|| bad("a delta is truncated"))?;
        *at += 1;
        if shift > 56 {
            return Err(bad("a delta size overflows"));
        }
        out |= ((c & 0x7f) as usize) << shift;
        shift += 7;
        if c & 0x80 == 0 {
            return Ok(out);
        }
    }
}

/// The size of the object a delta makes, from its header.
fn delta_result_size(delta: &[u8]) -> Result<usize> {
    let mut at = 0;
    varint(delta, &mut at)?;
    varint(delta, &mut at)
}

/// Applies a git delta to `base`.
pub fn apply_delta(base: &[u8], delta: &[u8], max_bytes: usize) -> Result<Vec<u8>> {
    let mut at = 0;
    let src = varint(delta, &mut at)?;
    if src != base.len() {
        return Err(bad("a delta's base size does not match its base"));
    }
    let dst = varint(delta, &mut at)?;
    if dst > max_bytes {
        return Err(GitError::invalid(format!(
            "A pushed object is over {max_bytes} bytes"
        )));
    }
    let mut out = Vec::with_capacity(dst);
    while at < delta.len() {
        let op = delta[at];
        at += 1;
        if op & 0x80 != 0 {
            let mut offset = 0usize;
            let mut size = 0usize;
            for i in 0..4 {
                if op & (1 << i) != 0 {
                    offset |= (*delta.get(at).ok_or_else(|| bad("a delta is truncated"))? as usize)
                        << (8 * i);
                    at += 1;
                }
            }
            for i in 0..3 {
                if op & (0x10 << i) != 0 {
                    size |= (*delta.get(at).ok_or_else(|| bad("a delta is truncated"))? as usize)
                        << (8 * i);
                    at += 1;
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            let end = offset
                .checked_add(size)
                .filter(|&e| e <= base.len())
                .ok_or_else(|| bad("a delta copies past its base"))?;
            out.extend_from_slice(&base[offset..end]);
        } else if op != 0 {
            let n = op as usize;
            let chunk = delta
                .get(at..at + n)
                .ok_or_else(|| bad("a delta is truncated"))?;
            out.extend_from_slice(chunk);
            at += n;
        } else {
            return Err(bad("a delta holds the reserved instruction 0"));
        }
        if out.len() > dst {
            return Err(bad("a delta writes past its declared size"));
        }
    }
    if out.len() != dst {
        return Err(bad("a delta's result is not its declared size"));
    }
    Ok(out)
}
