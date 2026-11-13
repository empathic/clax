//! A host's content routes of the ClaxBot surfaces protocol
//! (`docs/contract.md`, "ClaxBot surfaces"; the design's "Content:
//! images, files and their metadata"), over `clax_store::bot_content`:
//!
//! - `PUT /api/bot/surfaces/<name>/content?registration=<id>&run=<run>`
//!   with the bytes as the body and their media type as `Content-Type` →
//!   the whole handle, `{id, media_type, bytes, sha256, name?, width?,
//!   height?}`: uploads an item for a run the registration has a call of
//!   that is not answered yet. Its host then answers that call with
//!   `{"id": …, "meta"?: …}` where the operation's type says `Content` or
//!   `Image`.
//! - `GET /api/bot/surfaces/<name>/content/<id>?registration=<id>&run=<run>`
//!   → the bytes, under their media type, as an attachment
//!   (`Content-Disposition: attachment`) under a policy that runs nothing
//!   in them (`Content-Security-Policy: sandbox; default-src 'none'`,
//!   beside `X-Content-Type-Options: nosniff`): an item the run holds, a
//!   handle of which one of the run's calls to the registration passed at
//!   a place its operation's type says is `Content` or `Image`
//!   ([`clax_http::bot_surfaces::Surfaces::passed`]); a copy of one in an
//!   `unknown` argument or in a handle's `meta` passes nothing.
//!
//! What the bytes are is Clax's finding ([`judge`]): the four image types
//! are read from the bytes (`clax_core::media::sniff`), and bytes that are
//! not what `Content-Type` says are refused, as are bytes that are an
//! image under another type, and `text/*` or JSON that is not UTF-8 (or
//! not JSON). A host may also state `sha256`, `bytes`, `width` and
//! `height` in the query: each is checked against Clax's finding, and a
//! request whose statement differs is refused; a statement it does not
//! know is refused, so a daemon never ignores one. A stated `width` and
//! `height` are the image's displayed size, as `sniff` reads it.
//!
//! Who may: the daemon's token, as the other host routes
//! ([`clax_http::bot_surfaces::DaemonToken`]). Every route asked with a
//! registration the hub does not hold (one never issued, or one gone), or
//! with another run's or another registration's IDs, answers as for
//! content that is not there: 404 `not_found`, never
//! `unknown_registration`, so a registration that is not there and
//! another's content look the same. Refusals are `{"error": {"code",
//! "message"}}`: 400 `invalid_request` and `content_mismatch`, 404
//! `not_found`, 413 `content_too_large` (the item) and
//! `run_content_full` (the run's content), 429 `too_many_uploads` (a
//! registration's uploads in flight), 507 `content_store_full` (all
//! runs' content), and 501 `not_yet` on a daemon that keeps no content.

use super::artifacts::path;
use axum::body::Body;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use clax_core::CoreError;
use clax_core::media::{self, Sniffed};
use clax_http::agent::DeniedToGrants;
use clax_http::bot_surfaces::{self as hub, DaemonToken, refused};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::bot_content::{
    CODE_CONTENT_STORE_FULL, CODE_CONTENT_TOO_LARGE, CODE_RUN_CONTENT_FULL, ContentRoom, NewContent,
};
use clax_surface::wire::{CODE_INVALID_REQUEST, CODE_NOT_YET};
use serde_json::Value;

/// The code of bytes that are not what the request says they are.
pub const CODE_CONTENT_MISMATCH: &str = "content_mismatch";
/// The code of content that is not there for the request's run and
/// registration.
pub const CODE_NOT_FOUND: &str = "not_found";
/// The code of an upload of a registration that has as many in flight
/// as [`clax_bot::bot_surfaces::HubLimits::uploads_per_registration`].
pub const CODE_TOO_MANY_UPLOADS: &str = "too_many_uploads";

/// The most bytes of an item's `name`.
const NAME_BYTES: usize = 255;

/// What a request names and states.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentQuery {
    /// The registration the request is made as.
    pub registration: String,
    /// The run the content is for.
    pub run: String,
    /// A file name for the item (an upload's alone).
    pub name: Option<String>,
    /// What the host says the bytes are; each is checked (an upload's
    /// alone).
    pub sha256: Option<String>,
    pub bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, CODE_INVALID_REQUEST, m)
}

fn mismatch(m: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, CODE_CONTENT_MISMATCH, m)
}

fn absent() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        CODE_NOT_FOUND,
        "no such content for this run and registration",
    )
}

fn query(q: Result<Query<ContentQuery>, QueryRejection>) -> Result<ContentQuery, ApiError> {
    q.map(|Query(q)| q)
        .map_err(|e| invalid(format!("the query: {}", e.body_text())))
}

/// The media type `Content-Type` names: its essence, lowercase, without
/// parameters.
fn media_type(headers: &HeaderMap) -> Result<String, ApiError> {
    let raw = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| invalid("an upload says what its bytes are in Content-Type"))?;
    let essence = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match essence.split_once('/') {
        Some((t, sub)) if token(t) && token(sub) => Ok(essence),
        _ => Err(invalid(format!("{raw:?} is not a media type"))),
    }
}

/// What Clax finds `bytes` to be, stated as `stated`: the image's size
/// for one of the four image types, nothing more for any other type the
/// bytes do not contradict.
///
/// # Errors
/// [`CODE_CONTENT_MISMATCH`] when they are not what `stated` says.
pub fn judge(stated: &str, bytes: &[u8]) -> Result<Option<Sniffed>, ApiError> {
    let sniffed = media::sniff(bytes);
    if clax_surface::IMAGE_MEDIA_TYPES.contains(&stated) {
        return match sniffed {
            Ok(s) if s.media_type == stated => Ok(Some(s)),
            Ok(s) => Err(mismatch(format!(
                "the bytes are {}, not {stated}",
                s.media_type
            ))),
            Err(why) => Err(mismatch(format!("the bytes are not {stated}: {why}"))),
        };
    }
    if let Ok(s) = sniffed {
        return Err(mismatch(format!(
            "the bytes are {}, not {stated}",
            s.media_type
        )));
    }
    let utf8 = || std::str::from_utf8(bytes).is_ok();
    if stated.starts_with("text/") && !utf8() {
        return Err(mismatch(format!("the bytes are not {stated}: not UTF-8")));
    }
    if (stated == "application/json" || stated.ends_with("+json"))
        && serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_err()
    {
        return Err(mismatch(format!("the bytes are not {stated}: not JSON")));
    }
    Ok(None)
}

/// Refuses a statement of the host's that differs from Clax's finding.
fn stated_agrees(
    q: &ContentQuery,
    sha256: &str,
    size: u64,
    found: Option<Sniffed>,
) -> Result<(), ApiError> {
    if let Some(s) = &q.sha256
        && !s.eq_ignore_ascii_case(sha256)
    {
        return Err(mismatch(format!("the bytes' SHA-256 is {sha256}, not {s}")));
    }
    if let Some(b) = q.bytes
        && b != size
    {
        return Err(mismatch(format!("the body is {size} bytes, not {b}")));
    }
    for (what, stated, found) in [
        ("width", q.width, found.map(|f| f.width)),
        ("height", q.height, found.map(|f| f.height)),
    ] {
        match (stated, found) {
            (Some(s), Some(f)) if s != f => {
                return Err(mismatch(format!(
                    "the image's {what} as displayed is {f}, not {s}: a JPEG's sides \
                     after its EXIF orientation, a GIF's those of the canvas its first \
                     frame is drawn on"
                )));
            }
            (Some(_), None) => {
                return Err(mismatch(format!(
                    "a {what} is stated, and Clax reads none from these bytes"
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The refusal of an upload past one of the content limits, under the
/// code of the bound it passes ([`ContentRoom::most`]).
fn full(code: &'static str, message: impl Into<String>) -> ApiError {
    let status = if code == CODE_CONTENT_STORE_FULL {
        StatusCode::INSUFFICIENT_STORAGE
    } else {
        StatusCode::PAYLOAD_TOO_LARGE
    };
    ApiError::new(status, code, message)
}

/// The length `Content-Length` states, if the request has one.
fn content_length(headers: &HeaderMap) -> Result<Option<u64>, ApiError> {
    headers
        .get(header::CONTENT_LENGTH)
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .ok_or_else(|| invalid("Content-Length is not a number of bytes"))
        })
        .transpose()
}

/// The body, read a piece at a time until it ends, or until it holds
/// more than `most` bytes: then `None`, and nothing more of it is read.
async fn read_at_most(
    body: Body,
    most: u64,
    hint: Option<u64>,
) -> Result<Option<Vec<u8>>, ApiError> {
    use futures::StreamExt as _;
    let most = usize::try_from(most).unwrap_or(usize::MAX);
    let hint = hint.and_then(|h| usize::try_from(h).ok()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(hint.min(most));
    let mut stream = body.into_data_stream();
    while let Some(piece) = stream.next().await {
        let piece =
            piece.map_err(|e| invalid(format!("the request's body could not be read: {e}")))?;
        if bytes.len().saturating_add(piece.len()) > most {
            return Ok(None);
        }
        bytes.extend_from_slice(&piece);
    }
    Ok(Some(bytes))
}

/// `PUT /api/bot/surfaces/<name>/content` (see the module documentation).
///
/// Everything a request can be refused for without its body is checked
/// before a byte of it is read: the registration, a call of the run's to
/// it not yet answered, `Content-Length` against the room left for the
/// item ([`Store::bot_content_room`]: the item's limit, the run's and all
/// runs'), and the registration's uploads in flight
/// ([`clax_bot::bot_surfaces::HubLimits::uploads_per_registration`]).
/// The body is then read no further than a piece past that room.
///
/// [`Store::bot_content_room`]: clax_store::Store::bot_content_room
pub async fn upload(
    State(s): State<AppState>,
    _token: DaemonToken,
    _d: DeniedToGrants,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<ContentQuery>, QueryRejection>,
    headers: HeaderMap,
    body: Body,
) -> Result<axum::Json<Value>, ApiError> {
    let name = path(p)?;
    let q = query(q)?;
    let limits = s.bot_surfaces.limits();
    if !limits.content {
        return Err(refused(CODE_NOT_YET, "this daemon keeps no content"));
    }
    let media_type = media_type(&headers)?;
    if let Some(n) = &q.name
        && (n.is_empty() || n.len() > NAME_BYTES || n.chars().any(char::is_control))
    {
        return Err(invalid(format!(
            "a name is 1 to {NAME_BYTES} bytes with no control character"
        )));
    }
    let length = content_length(&headers)?;
    let content_limits = limits.content_limits();
    let fits = |room: ContentRoom, size: u64| {
        room.fits(size, content_limits).map_err(|e| match e {
            CoreError::Invalid { code, message } => full(code, message),
            e => ApiError::from(e),
        })
    };
    let item = ContentRoom {
        item: limits.content_item_bytes,
        run: u64::MAX,
        total: u64::MAX,
    };
    if let Some(n) = length {
        fits(item, n)?;
    }
    let (surfaces, at) = (s.bot_surfaces.clone(), (name.clone(), q.clone()));
    let room = s
        .store_call(move |st| {
            let (name, q) = at;
            if surfaces.find(&name, &q.registration).is_err()
                || !st.serves_open_call(&q.run, &q.registration)?
            {
                return Ok(None);
            }
            st.bot_content_room(&q.run, content_limits).map(Some)
        })
        .await?
        .ok_or_else(absent)?;
    if let Some(n) = length {
        fits(room, n)?;
    }
    let _upload = s
        .bot_surfaces
        .begin_upload(&q.registration)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                CODE_TOO_MANY_UPLOADS,
                format!(
                    "a registration has {} uploads in flight at the most",
                    limits.uploads_per_registration
                ),
            )
        })?;
    let (most, _) = room.most();
    let Some(bytes) = read_at_most(body, most, length).await? else {
        return Err(fits(room, most.saturating_add(1))
            .err()
            .unwrap_or_else(absent));
    };
    let found = judge(&media_type, &bytes)?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    stated_agrees(&q, &clax_core::audit::sha256_hex(&bytes), size, found)?;
    let surfaces = s.bot_surfaces.clone();
    let id = hub::fresh_id();
    let row = s
        .store_call(move |st| {
            if surfaces.find(&name, &q.registration).is_err()
                || !st.serves_open_call(&q.run, &q.registration)?
            {
                return Ok(Err(absent()));
            }
            let new = NewContent {
                id: &id,
                run_id: &q.run,
                surface: &name,
                registration_id: Some(&q.registration),
                name: q.name.as_deref(),
                media_type: &media_type,
                width: found.map(|f| f.width),
                height: found.map(|f| f.height),
                bytes: &bytes,
            };
            match st.put_bot_content(&new, content_limits) {
                Ok(row) => Ok(Ok(row)),
                Err(CoreError::Invalid { code, message })
                    if code == CODE_CONTENT_TOO_LARGE
                        || code == CODE_RUN_CONTENT_FULL
                        || code == CODE_CONTENT_STORE_FULL =>
                {
                    Ok(Err(full(code, message)))
                }
                Err(e) => Err(e),
            }
        })
        .await??;
    Ok(axum::Json(row.handle()))
}

/// `GET /api/bot/surfaces/<name>/content/<id>` (see the module
/// documentation).
pub async fn download(
    State(s): State<AppState>,
    _token: DaemonToken,
    _d: DeniedToGrants,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<ContentQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let (name, id) = path(p)?;
    let q = query(q)?;
    let surfaces = s.bot_surfaces.clone();
    let found = s
        .store_call(move |st| {
            if surfaces.find(&name, &q.registration).is_err() {
                return Ok(None);
            }
            let Some(row) = st.bot_content_of(&q.run, &id)? else {
                return Ok(None);
            };
            let passed = surfaces
                .passed(&row, &q.registration)
                .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
            if !passed {
                return Ok(None);
            }
            Ok(st.bot_content_bytes(&row)?.map(|bytes| (row, bytes)))
        })
        .await?;
    let (row, bytes) = found.ok_or_else(absent)?;
    let mut res = Response::new(Body::from(bytes));
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&row.media_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    // The bytes are a host's, of any type: never a page of the daemon's
    // origin, whatever reads them.
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("\"{}\"", row.sha256)) {
        h.insert(header::ETAG, v);
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x1 PNG: the signature and a whole IHDR chunk.
    fn png() -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        let crc = crc(&b[12..29]);
        b.extend_from_slice(&crc.to_be_bytes());
        b
    }

    fn crc(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    fn code(r: Result<Option<Sniffed>, ApiError>) -> &'static str {
        r.map_or_else(|e| e.code, |_| "ok")
    }

    #[test]
    fn the_bytes_are_held_to_the_type_stated() {
        let png = png();
        let found = judge("image/png", &png).unwrap().unwrap();
        assert_eq!((found.width, found.height), (2, 1));
        assert_eq!(code(judge("image/jpeg", &png)), CODE_CONTENT_MISMATCH);
        assert_eq!(code(judge("text/plain", &png)), CODE_CONTENT_MISMATCH);
        assert_eq!(
            code(judge("image/png", b"not a png")),
            CODE_CONTENT_MISMATCH
        );
        assert_eq!(code(judge("image/png", &png[..20])), CODE_CONTENT_MISMATCH);
        assert_eq!(code(judge("text/plain", b"plain words")), "ok");
        assert_eq!(
            code(judge("text/plain", b"\xff\xfe")),
            CODE_CONTENT_MISMATCH
        );
        assert_eq!(code(judge("application/json", b"{\"a\": 1}")), "ok");
        assert_eq!(code(judge("application/json", b"{")), CODE_CONTENT_MISMATCH);
        assert_eq!(code(judge("application/pdf", b"%PDF-1.7")), "ok");
    }

    #[test]
    fn a_hosts_statements_are_checked_against_the_findings() {
        let q = |sha256: Option<&str>, bytes, width, height| ContentQuery {
            registration: "r".into(),
            run: "u".into(),
            name: None,
            sha256: sha256.map(str::to_string),
            bytes,
            width,
            height,
        };
        let png = png();
        let sha = clax_core::audit::sha256_hex(&png);
        let found = judge("image/png", &png).unwrap();
        let size = u64::try_from(png.len()).unwrap();
        let agrees = |q: ContentQuery, found| stated_agrees(&q, &sha, size, found).is_ok();
        assert!(agrees(q(None, None, None, None), found));
        assert!(agrees(
            q(Some(&sha.to_uppercase()), Some(size), Some(2), Some(1)),
            found
        ));
        assert!(!agrees(q(Some(&"0".repeat(64)), None, None, None), found));
        assert!(!agrees(q(None, Some(size + 1), None, None), found));
        assert!(!agrees(q(None, None, Some(3), None), found));
        assert!(!agrees(q(None, None, None, Some(1)), None));
    }

    #[test]
    fn a_rotated_jpeg_is_stated_at_its_displayed_size() {
        // SOI, an APP1 of EXIF whose Orientation is 6 (a quarter turn),
        // and a frame header of 2x1 stored.
        let mut jpeg = b"\xff\xd8\xff\xe1\x00\x22Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
        jpeg.extend(b"\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0");
        jpeg.extend(b"\xff\xc0\0\x0b\x08\0\x01\0\x02\x01\x01\x11\0");
        let found = judge("image/jpeg", &jpeg).unwrap();
        assert_eq!(found.map(|f| (f.width, f.height)), Some((1, 2)));
        let sha = clax_core::audit::sha256_hex(&jpeg);
        let size = u64::try_from(jpeg.len()).unwrap();
        let q = |width, height| ContentQuery {
            registration: "r".into(),
            run: "u".into(),
            name: None,
            sha256: None,
            bytes: None,
            width: Some(width),
            height: Some(height),
        };
        assert!(stated_agrees(&q(1, 2), &sha, size, found).is_ok());
        let stored = stated_agrees(&q(2, 1), &sha, size, found).unwrap_err();
        assert_eq!(stored.code, CODE_CONTENT_MISMATCH);
        assert!(
            stored
                .message
                .starts_with("the image's width as displayed is 1, not 2: "),
            "{}",
            stored.message
        );
    }
}
