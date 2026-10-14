//! SHA-1 object IDs.

use std::fmt;
use std::str::FromStr;

use sha1::{Digest, Sha1};

use crate::error::GitError;

/// A git object ID: the SHA-1 of the object's loose form.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ObjectId(pub [u8; 20]);

impl ObjectId {
    /// The all-zero ID git uses for "no object" in ref updates.
    pub const ZERO: ObjectId = ObjectId([0; 20]);

    /// The SHA-1 of `bytes`.
    pub fn sha1(bytes: &[u8]) -> ObjectId {
        ObjectId(Sha1::digest(bytes).into())
    }

    /// Parses 40 lowercase hex digits.
    pub fn from_hex(hex: &str) -> Option<ObjectId> {
        let b = hex.as_bytes();
        if b.len() != 40 {
            return None;
        }
        let mut out = [0u8; 20];
        for (i, pair) in b.chunks(2).enumerate() {
            out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Some(ObjectId(out))
    }

    /// The ID from its 20 raw bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<ObjectId> {
        bytes.try_into().ok().map(ObjectId)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0; 20]
    }

    /// Lowercase hex, 40 characters.
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(40);
        for b in self.0 {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 15) as usize] as char);
        }
        s
    }
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.to_hex())
    }
}

impl FromStr for ObjectId {
    type Err = GitError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ObjectId::from_hex(s).ok_or_else(|| GitError::invalid(format!("Not an object ID: {s:?}")))
    }
}
