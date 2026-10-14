//! The HTTP messages the client produces and the server answers, as plain
//! data: the caller moves them over its own transport (reqwest natively,
//! `fetch` in a Worker).

#![allow(async_fn_in_trait)]

use crate::error::Result;

/// An HTTP request to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET` or `POST`.
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// An HTTP response: its status, headers and whole body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response with `status` and `body` and no headers.
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// The first header named `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Sends a request and reads the whole response. A failure to reach the
/// remote is `repo_unavailable`; an HTTP error status is a response, not an
/// error.
pub trait Transport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse>;
}
