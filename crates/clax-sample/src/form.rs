//! One `application/x-www-form-urlencoded` POST on the provider path's
//! transport, for a service that takes a form: no redirect is followed, so a
//! credential in `headers` goes to `url` alone, and on either host the same
//! code runs (`crate::transport`).

use crate::transport::Transport;
use futures::StreamExt;

/// The most of an answer's body that is read.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// What a form's POST was answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormAnswer {
    /// A 3xx comes back as it is; a browser's opaque redirect is 0.
    pub status: u16,
    pub retry_after: Option<String>,
    /// At most [`MAX_BODY_BYTES`] of it.
    pub body: Vec<u8>,
}

/// POSTs `form`, already encoded, to `url` with `headers`, connecting only to
/// public addresses when `public_only` ([`Transport::public_only`]; a Worker
/// reaches no others). `Err` when no answer came: its message may name the
/// URL, never a header.
///
/// # Errors
/// As above.
pub async fn post_form(
    url: &str,
    headers: &[(&str, String)],
    form: String,
    public_only: bool,
) -> Result<FormAnswer, String> {
    let transport = if public_only {
        Transport::public_only()
    } else {
        Transport::new()
    };
    let kind = "application/x-www-form-urlencoded";
    let mut reply = transport
        .send_body("POST", url, headers, Some((kind, form)))
        .await?;
    let mut body = Vec::new();
    while body.len() < MAX_BODY_BYTES {
        match reply.body.next().await {
            Some(Ok(chunk)) => body.extend_from_slice(&chunk),
            Some(Err(e)) => return Err(e),
            None => break,
        }
    }
    body.truncate(MAX_BODY_BYTES);
    Ok(FormAnswer {
        status: reply.status,
        retry_after: reply.retry_after,
        body,
    })
}
