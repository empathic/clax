//! The Worker's sender of report queues' webhooks
//! (`clax_http::webhooks::Sender`): the platform's `fetch`, with
//! `redirect: "manual"` so a redirect comes back as the attempt's answer
//! and is never followed. Cloudflare's `fetch` reaches no private network,
//! so a hosted Worker needs no address check of its own beyond the URL
//! rule every endpoint passes when it is named
//! (`clax_core::webhook::check_url`). The instance's alarm runs the
//! delivery pass (`clax_http::webhooks::deliver_due`), set for the next
//! due delivery (`Store::webhooks_next_due`). Verification, the abuse
//! limits and the outbox are the store's and `deliver_due`'s, the same as
//! the daemon's; an answer is read as the daemon reads it, up to
//! `clax_core::webhook::MAX_ANSWER_BYTES`.

use clax_http::webhooks::{SendFuture, Sender};

/// Posts through the Worker's `fetch`.
pub struct FetchSender;

#[cfg(target_arch = "wasm32")]
impl Sender for FetchSender {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: &'a [u8],
    ) -> SendFuture<'a> {
        use worker::{Fetch, Headers, Method, Request, RequestInit, RequestRedirect};
        Box::pin(async move {
            let r = async {
                let h = Headers::new();
                for (k, v) in headers {
                    h.set(k, v)?;
                }
                let mut init = RequestInit::new();
                init.with_method(Method::Post)
                    .with_headers(h)
                    .with_redirect(RequestRedirect::Manual)
                    .with_body(Some(js_sys::Uint8Array::from(body).into()));
                let req = Request::new_with_init(url, &init)?;
                let mut res = Fetch::Request(req).send().await?;
                let status = res.status_code();
                // As the daemon's sender reads it: at most MAX_ANSWER_BYTES.
                let mut r = clax_http::webhooks::AnswerReader::new(status);
                let ended = match res.stream() {
                    Ok(mut body) => loop {
                        if !r.wants_more() {
                            break false;
                        }
                        match futures_util::StreamExt::next(&mut body).await {
                            Some(Ok(c)) => r.push(&c),
                            None => break true,
                            Some(Err(_)) => break false,
                        }
                    },
                    // No body at all.
                    Err(_) => true,
                };
                Ok::<_, worker::Error>(r.finish(ended))
            };
            r.await.map_err(|e| e.to_string())
        })
    }
}

/// The Worker's `fetch` exists only in a Worker.
#[cfg(not(target_arch = "wasm32"))]
impl Sender for FetchSender {
    fn post<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(String, String)],
        _body: &'a [u8],
    ) -> SendFuture<'a> {
        Box::pin(async { Err("the Worker's fetch is a Worker's (wasm32)".to_string()) })
    }
}
