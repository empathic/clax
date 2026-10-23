//! The one HTTP path to a provider, shared by the `sample` provider
//! ([`crate::anthropic`]) and the Messages client ([`crate::messages`]).
//!
//! It never follows a redirect: a request carries the key in `x-api-key`,
//! which a followed redirect would hand to whatever host the `Location`
//! names (reqwest strips only its own credential headers across hosts, and
//! `fetch` strips none of a custom one). A 3xx answer comes back as it is,
//! for the caller to refuse. Natively this is reqwest with
//! `redirect::Policy::none()`; on wasm32 it is the Worker's global `fetch`
//! with `redirect: "manual"`, whose 3xx (or, in a browser, an opaque
//! redirect with status 0) is returned likewise.
//!
//! Dropping a request's future or its body stops it: natively reqwest
//! closes the connection; on wasm32 an `AbortController` aborts the fetch.

use futures::Stream;
use serde_json::Value;
use std::pin::Pin;

/// A response body, chunk by chunk; an error is the transport's message
/// (which may name the URL, never a header).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Body = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
pub(crate) type Body = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>>>>;

pub(crate) struct Reply {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: Body,
}

impl Reply {
    pub fn success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A 3xx, or a browser's opaque redirect (status 0).
    pub fn redirect(&self) -> bool {
        self.status == 0 || (300..400).contains(&self.status)
    }
}

#[derive(Clone)]
pub(crate) struct Transport {
    #[cfg(not(target_arch = "wasm32"))]
    client: reqwest::Client,
}

impl Transport {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Transport {
        Transport {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("a reqwest client with default TLS"),
        }
    }

    /// The Worker's `fetch`, which has no connect timeout to set.
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Transport {
        Transport {}
    }

    /// Sends `method url` with `headers` and, when given, `json` as the
    /// body. `Err` when no answer came (the message may name the URL).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        json: Option<&Value>,
    ) -> Result<Reply, String> {
        use futures::StreamExt;
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut rb = self.client.request(method, url);
        for (k, v) in headers {
            rb = rb.header(*k, v);
        }
        if let Some(v) = json {
            rb = rb
                .header("content-type", "application/json")
                .body(v.to_string());
        }
        let res = rb.send().await.map_err(|e| e.to_string())?;
        let retry_after = res
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .map(str::to_string);
        Ok(Reply {
            status: res.status().as_u16(),
            retry_after,
            body: Box::pin(
                res.bytes_stream()
                    .map(|c| c.map(|b| b.to_vec()).map_err(|e| e.to_string())),
            ),
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        json: Option<&Value>,
    ) -> Result<Reply, String> {
        use futures::StreamExt;
        use wasm_bindgen::{JsCast, JsValue};
        let js = |e: JsValue| format!("{e:?}");
        // Aborts the fetch when dropped: while waiting for the answer (a
        // timeout drops this future) and, moved into the body stream below,
        // once the body is dropped unread.
        let abort = AbortOnDrop(web_sys::AbortController::new().map_err(js)?);
        let init = web_sys::RequestInit::new();
        init.set_method(method);
        init.set_redirect(web_sys::RequestRedirect::Manual);
        init.set_signal(Some(&abort.0.signal()));
        let h = web_sys::Headers::new().map_err(js)?;
        for (k, v) in headers {
            h.append(k, v).map_err(js)?;
        }
        if let Some(v) = json {
            h.append("content-type", "application/json").map_err(js)?;
            init.set_body(&JsValue::from_str(&v.to_string()));
        }
        init.set_headers(&h);
        let req = web_sys::Request::new_with_str_and_init(url, &init).map_err(js)?;
        let global = js_sys::global();
        let fetch: js_sys::Function = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))
            .map_err(js)?
            .dyn_into()
            .map_err(js)?;
        let promise: js_sys::Promise = fetch
            .call1(&global, &req)
            .map_err(js)?
            .dyn_into()
            .map_err(js)?;
        let res: web_sys::Response = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(js)?
            .dyn_into()
            .map_err(js)?;
        let retry_after = res.headers().get("retry-after").ok().flatten();
        let body: Body = match res.body() {
            Some(raw) => Box::pin(
                wasm_streams::ReadableStream::from_raw(raw.unchecked_into())
                    .into_stream()
                    .map(move |c| {
                        let _abort = &abort;
                        c.map(|v| js_sys::Uint8Array::new(&v).to_vec())
                            .map_err(|e| format!("{e:?}"))
                    }),
            ),
            None => Box::pin(futures::stream::empty()),
        };
        Ok(Reply {
            status: res.status(),
            retry_after,
            body,
        })
    }
}

/// Aborts its fetch when dropped (a no-op once the fetch has finished).
#[cfg(target_arch = "wasm32")]
struct AbortOnDrop(web_sys::AbortController);

#[cfg(target_arch = "wasm32")]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
