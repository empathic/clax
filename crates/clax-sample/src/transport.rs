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

    /// [`Transport::new`] connecting only to public addresses: a name is
    /// resolved here, and its answers that are not
    /// [`clax_core::config::public_address`]es (loopback, private,
    /// link-local, unique-local among them) are never connected to; a name
    /// with none left fails as unresolved. No proxy in the environment is
    /// used, since one would resolve the name itself. A hosted daemon's
    /// client, which sends the instance's key.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn public_only() -> Transport {
        Transport {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(std::sync::Arc::new(PublicOnly))
                .no_proxy()
                .build()
                .expect("a reqwest client with default TLS"),
        }
    }

    /// The Worker's `fetch`, which has no connect timeout to set.
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Transport {
        Transport {}
    }

    /// The Worker's `fetch`: its platform reaches no private network.
    #[cfg(target_arch = "wasm32")]
    pub fn public_only() -> Transport {
        Transport {}
    }

    /// Sends `method url` with `headers` and, when given, `json` as the
    /// body. `Err` when no answer came (the message may name the URL).
    pub async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        json: Option<&Value>,
    ) -> Result<Reply, String> {
        let body = json.map(|v| ("application/json", v.to_string()));
        self.send_body(method, url, headers, body).await
    }

    /// [`Transport::send`] with `body`, its content type and text.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn send_body(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: Option<(&str, String)>,
    ) -> Result<Reply, String> {
        use futures::StreamExt;
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut rb = self.client.request(method, url);
        for (k, v) in headers {
            let mut value = reqwest::header::HeaderValue::from_str(v)
                .map_err(|_| format!("the {k} header's value cannot be sent"))?;
            value.set_sensitive(k.eq_ignore_ascii_case("authorization"));
            rb = rb.header(*k, value);
        }
        if let Some((kind, text)) = body {
            rb = rb.header("content-type", kind).body(text);
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
    pub async fn send_body(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: Option<(&str, String)>,
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
        if let Some((kind, text)) = body {
            h.append("content-type", kind).map_err(js)?;
            init.set_body(&JsValue::from_str(&text));
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

/// A resolver that answers only a name's public addresses
/// ([`Transport::public_only`]).
#[cfg(not(target_arch = "wasm32"))]
struct PublicOnly;

#[cfg(not(target_arch = "wasm32"))]
impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs = public_addrs(tokio::net::lookup_host((host.as_str(), 0)).await?);
            if addrs.is_empty() {
                return Err(format!("{host} resolves to no public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The public ones of `addrs` ([`clax_core::config::public_address`]).
#[cfg(not(target_arch = "wasm32"))]
fn public_addrs(addrs: impl Iterator<Item = std::net::SocketAddr>) -> Vec<std::net::SocketAddr> {
    addrs
        .filter(|a| clax_core::config::public_address(a.ip()))
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A public-only client never connects to a name that resolves to a
    /// loopback address, where the plain client does.
    #[tokio::test]
    async fn a_public_only_client_connects_to_no_private_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            while let Ok((mut s, _)) = listener.accept().await {
                let _ = s
                    .write_all(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        let url = format!("http://localhost:{port}/v1/models");
        let plain = Transport::new().send("GET", &url, &[], None).await;
        assert_eq!(plain.map(|r| r.status).ok(), Some(204));
        let Err(e) = Transport::public_only().send("GET", &url, &[], None).await else {
            panic!("connected to localhost");
        };
        assert!(e.contains("error sending request"), "{e}");
    }

    /// A public-only client, in a process whose environment names a proxy.
    /// Run by [`a_public_only_client_uses_no_proxy_in_the_environment`], since
    /// a test cannot change the environment of its own process.
    #[tokio::test]
    #[ignore = "run in a process of its own"]
    async fn public_only_with_a_proxy_in_the_environment() {
        let client = Transport::public_only();
        let sent = client.send("GET", "http://forge.invalid/", &[], None);
        assert!(sent.await.is_err());
    }

    #[tokio::test]
    async fn a_public_only_client_uses_no_proxy_in_the_environment() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        // Counted before it is closed, and a client that came waits for that.
        let reached = Arc::new(AtomicUsize::new(0));
        let count = reached.clone();
        tokio::spawn(async move {
            while listener.accept().await.is_ok() {
                count.fetch_add(1, Ordering::SeqCst);
            }
        });
        let test = "transport::tests::public_only_with_a_proxy_in_the_environment";
        let out = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--ignored"])
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .await
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.contains("1 passed"), "{said}");
        assert_eq!(reached.load(Ordering::SeqCst), 0, "it went to the proxy");
    }

    #[test]
    fn only_public_answers_are_kept() {
        let addrs = ["10.0.0.5:443", "1.1.1.1:443", "[::1]:443", "[fd00::1]:443"]
            .map(|a| a.parse::<std::net::SocketAddr>().unwrap());
        assert_eq!(
            public_addrs(addrs.into_iter()),
            ["1.1.1.1:443".parse::<std::net::SocketAddr>().unwrap()]
        );
    }
}
