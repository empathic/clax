//! The bytes `clax_embed_call` takes and posts (spec §2.3): `CLX1`, a
//! u32 little-endian header length, the header's JSON, the body.

/// The four bytes every frame starts with.
pub const MAGIC: &[u8; 4] = b"CLX1";

/// Who a request speaks for; the core, not the caller, turns it into
/// credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum As {
    /// The app's own code and the shell calls its host relays.
    Owner,
    /// A scheme request to an artifact's content origin.
    Content,
    /// A scheme request to the shell's origin.
    ShellPage,
}

/// A request frame's header.
#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RequestHead {
    pub method: String,
    /// The path and query.
    pub path: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(rename = "as")]
    pub as_: As,
    /// The artifact ID a `content` request's origin names.
    #[serde(default)]
    pub host: Option<String>,
}

/// A response frame's header.
#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

/// Why a frame did not decode.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than its length says.
    Short,
    /// It does not start with [`MAGIC`].
    Magic,
    /// Its header is not the JSON of a [`RequestHead`].
    Header(String),
}

/// Splits a request frame into its head and body.
///
/// # Errors
/// A [`FrameError`] naming what is wrong.
pub fn decode_request(bytes: &[u8]) -> Result<(RequestHead, &[u8]), FrameError> {
    let (head, body) = split(bytes)?;
    let head = serde_json::from_slice(head).map_err(|e| FrameError::Header(e.to_string()))?;
    Ok((head, body))
}

/// A frame's header bytes and body, unparsed.
///
/// # Errors
/// [`FrameError::Magic`] or [`FrameError::Short`].
pub fn split(bytes: &[u8]) -> Result<(&[u8], &[u8]), FrameError> {
    let rest = bytes
        .strip_prefix(MAGIC.as_slice())
        .ok_or(FrameError::Magic)?;
    let (len, rest) = rest.split_first_chunk::<4>().ok_or(FrameError::Short)?;
    let n = u32::from_le_bytes(*len) as usize;
    if rest.len() < n {
        return Err(FrameError::Short);
    }
    Ok(rest.split_at(n))
}

/// A frame of `head` and `body`.
pub fn encode<H: serde::Serialize>(head: &H, body: &[u8]) -> Vec<u8> {
    encode_after(&[], head, body)
}

/// `prefix`, then a frame of `head` and `body`, in one buffer allocated
/// once.
pub fn encode_after<H: serde::Serialize>(prefix: &[u8], head: &H, body: &[u8]) -> Vec<u8> {
    let mut out = begin(prefix, head, body.len());
    out.extend_from_slice(body);
    out
}

/// `prefix` and a frame's head, with room for `body` more bytes: the
/// start of a frame whose body is appended as it is read.
pub fn begin<H: serde::Serialize>(prefix: &[u8], head: &H, body: usize) -> Vec<u8> {
    let h = serde_json::to_vec(head).expect("a frame head serializes");
    let mut out = Vec::with_capacity(prefix.len() + 8 + h.len() + body);
    out.extend_from_slice(prefix);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(h.len() as u32).to_le_bytes());
    out.extend_from_slice(&h);
    out
}

/// The 8 bytes, little endian, that precede a call's response frame as
/// posted to Dart.
pub fn tag(tag: i64) -> [u8; 8] {
    tag.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips_with_a_binary_body() {
        let head = RequestHead {
            method: "POST".into(),
            path: "/api/x?y=1".into(),
            headers: vec![("content-type".into(), "image/png".into())],
            as_: As::Owner,
            host: None,
        };
        let body = [0u8, 255, 10, 13];
        let bytes = encode(&head, &body);
        assert_eq!(decode_request(&bytes).unwrap(), (head, &body[..]));
    }

    #[test]
    fn the_as_field_is_kebab_case() {
        let j = br#"{"method":"GET","path":"/v/1/","as":"shell-page"}"#;
        let bytes = [MAGIC.as_slice(), &(j.len() as u32).to_le_bytes(), j].concat();
        assert_eq!(decode_request(&bytes).unwrap().0.as_, As::ShellPage);
    }

    #[test]
    fn bad_frames_are_named() {
        assert_eq!(decode_request(b"CLX2xxxx").unwrap_err(), FrameError::Magic);
        assert_eq!(
            decode_request(b"CLX1\x09\0\0\0{}").unwrap_err(),
            FrameError::Short
        );
        assert!(matches!(
            decode_request(b"CLX1\x02\0\0\0{]").unwrap_err(),
            FrameError::Header(_)
        ));
    }

    #[test]
    fn a_tag_precedes_the_frame_little_endian() {
        let framed = encode_after(
            &tag(258),
            &ResponseHead {
                status: 200,
                headers: vec![],
            },
            &[9],
        );
        assert_eq!(&framed[..8], &[2, 1, 0, 0, 0, 0, 0, 0]);
        let (head, body) = split(&framed[8..]).unwrap();
        assert_eq!(head, br#"{"status":200,"headers":[]}"#);
        assert_eq!(body, &[9]);
        assert_eq!(framed.capacity(), framed.len(), "allocated once");
    }
}
