//! Sending report queues' webhooks (spec 2026-10-27-clax-sdk-design §9):
//! the pass that takes the store's due deliveries
//! (`clax_store::queues::webhooks`), signs and posts each
//! ([`clax_core::webhook`]) through the host's [`Sender`], and records
//! what came of it.
//!
//! The daemon runs [`deliver_due`] from a task of its own, woken by every
//! write that queued a delivery ([`Webhooks::queued`]) and when the next
//! retry is due; the Worker from its alarm, set for
//! `Store::webhooks_next_due`. Times are the state's clock's
//! (`AppState::question_clock`), so a test drives the retries by its
//! clock and runs the pass itself.
//!
//! A verification ([`VERIFY_EVENT`]) is delivered only when its 2xx
//! answer carries the challenge's answer ([`check_answer`]); the answer
//! is read up to [`MAX_ANSWER_BYTES`] and no further.
//!
//! No redirect is followed: a 3xx is a failed attempt. On a hosted Clax
//! the native sender connects only to public addresses
//! ([`clax_core::config::public_address`]), resolving the name itself, so
//! an endpoint whose name resolves to a loopback, private or link-local
//! address is never reached; the Worker's `fetch` reaches no private
//! network. A local Clax reaches loopback endpoints, for development.

use crate::state::AppState;
use clax_core::webhook::{
    BATCH, EVENT_HEADER, EVENT_ID_HEADER, MAX_ANSWER_BYTES, SIGNATURE_HEADER, TIMEOUT,
    TIMESTAMP_HEADER, VERIFY_EVENT, VerifyPayload, check_answer, logged, signature,
};
use clax_store::queues::webhooks::{DueDelivery, Outcome};
use std::sync::{Arc, RwLock};

/// The `User-Agent` each delivery carries.
pub const USER_AGENT: &str = concat!("Clax-Webhooks/", env!("CARGO_PKG_VERSION"));

/// Passes one [`deliver_due`] makes at most while each finds a full
/// [`BATCH`].
const MAX_PASSES: usize = 5;

/// An endpoint's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// The answer's body: its first [`MAX_ANSWER_BYTES`] at most.
    pub body: Vec<u8>,
    /// Whether `body` is all of it.
    pub whole: bool,
}

/// Reads an answer's body a chunk at a time and no further than
/// [`MAX_ANSWER_BYTES`]: a sender pushes chunks while
/// [`AnswerReader::wants_more`], then [`AnswerReader::finish`]es.
pub struct AnswerReader {
    status: u16,
    body: Vec<u8>,
}

impl AnswerReader {
    #[must_use]
    pub fn new(status: u16) -> AnswerReader {
        AnswerReader {
            status,
            body: Vec::new(),
        }
    }

    /// Whether another chunk is wanted: no more than [`MAX_ANSWER_BYTES`]
    /// came yet.
    pub fn wants_more(&self) -> bool {
        self.body.len() <= MAX_ANSWER_BYTES
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.body.extend_from_slice(chunk);
    }

    /// The answer; `ended` when the body's end was read.
    pub fn finish(mut self, ended: bool) -> Answer {
        let whole = ended && self.body.len() <= MAX_ANSWER_BYTES;
        self.body.truncate(MAX_ANSWER_BYTES);
        Answer {
            status: self.status,
            body: self.body,
            whole,
        }
    }
}

impl Answer {
    /// The body as text, cut for the delivery log.
    pub fn logged(&self) -> String {
        logged(&String::from_utf8_lossy(&self.body))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub type SendFuture<'a> = futures::future::BoxFuture<'a, Result<Answer, String>>;
#[cfg(target_arch = "wasm32")]
pub type SendFuture<'a> = futures::future::LocalBoxFuture<'a, Result<Answer, String>>;

/// Posts one delivery: never following a redirect. `Err` when no answer
/// came, saying why.
pub trait Sender: Send + Sync {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: &'a [u8],
    ) -> SendFuture<'a>;
}

/// The host's sender and the daemon's wake-up.
pub struct Webhooks {
    sender: RwLock<Option<Arc<dyn Sender>>>,
    queued: Arc<tokio::sync::Notify>,
}

impl Webhooks {
    /// The state's webhooks: natively [`ReqwestSender`] (connecting only
    /// to public addresses when `hosted`), on wasm32 none until the host
    /// sets its own ([`Webhooks::set_sender`]). Every committed write of
    /// `store`'s that queued a delivery wakes [`Webhooks::queued`].
    pub fn install(store: &clax_store::Store, hosted: bool) -> Arc<Webhooks> {
        #[cfg(not(target_arch = "wasm32"))]
        let sender: Option<Arc<dyn Sender>> = Some(Arc::new(ReqwestSender::new(hosted)));
        #[cfg(target_arch = "wasm32")]
        let sender: Option<Arc<dyn Sender>> = {
            let _ = hosted;
            None
        };
        let queued = Arc::new(tokio::sync::Notify::new());
        let tell = queued.clone();
        store.set_webhook_listener(move || tell.notify_one());
        Arc::new(Webhooks {
            sender: RwLock::new(sender),
            queued,
        })
    }

    /// Sends through `sender` from now on.
    pub fn set_sender(&self, sender: Arc<dyn Sender>) {
        *self.sender.write().unwrap() = Some(sender);
    }

    fn sender(&self) -> Option<Arc<dyn Sender>> {
        self.sender.read().unwrap().clone()
    }

    /// Resolves once a write has queued a delivery since the last time it
    /// resolved (one wake-up is kept while nobody waits).
    pub async fn queued(&self) {
        self.queued.notified().await;
    }
}

/// Sends every due delivery, a [`BATCH`] at a time and the batch's at
/// once, each within [`TIMEOUT`], and records each outcome; at most
/// [`MAX_PASSES`] batches. Answers how many were sent. Nothing without a
/// sender.
pub async fn deliver_due(s: &AppState) -> usize {
    let Some(sender) = s.webhooks.sender() else {
        return 0;
    };
    let base = link_base(s);
    let mut sent = 0;
    for _ in 0..MAX_PASSES {
        let due = match s.store.call(|st| st.take_due_webhooks(BATCH)).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "taking due webhook deliveries failed");
                return sent;
            }
        };
        let n = due.len();
        if n == 0 {
            break;
        }
        let now = s.question_clock.now().timestamp();
        let tries = due.iter().map(|d| attempt(&*sender, d, &base, now));
        let outcomes = futures::future::join_all(tries).await;
        let done: Vec<(String, u32, Outcome)> = due
            .into_iter()
            .zip(outcomes)
            .map(|(d, o)| (d.id, d.attempts, o))
            .collect();
        if let Err(e) = s
            .store
            .call(move |st| {
                for (id, attempts, o) in &done {
                    st.record_webhook_outcome(id, *attempts, o)?;
                }
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %e, "recording webhook outcomes failed; they are sent again");
        }
        sent += n;
        if n < BATCH {
            break;
        }
    }
    sent
}

/// How long until the next pending delivery is due by the state's clock
/// (zero when one is due), or `None` with none pending.
///
/// # Errors
/// The store's.
pub async fn next_due_in(s: &AppState) -> clax_core::Result<Option<std::time::Duration>> {
    let at = s.store.call(|st| st.webhooks_next_due()).await?;
    let now = s.question_clock.now();
    Ok(at.map(|at| (at - now).to_std().unwrap_or_default()))
}

/// Where a report's `link` points: the hosted origin, or the daemon's
/// browser base.
fn link_base(s: &AppState) -> String {
    match &s.hosted {
        Some(h) => h.origin.clone(),
        None => s.browser_base.clone(),
    }
    .trim_end_matches('/')
    .to_string()
}

/// The body delivery `d` posts: its payload with `report.link` set to the
/// report in its queue at `base`.
pub fn body_of(d: &DueDelivery, base: &str) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&d.payload) else {
        return d.payload.clone().into_bytes();
    };
    let qid = v["queue"]["id"].as_str().unwrap_or_default().to_string();
    if let Some(report) = v.get_mut("report").and_then(|r| r.as_object_mut()) {
        let tid = report
            .get("id")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        report.insert(
            "link".into(),
            serde_json::Value::String(format!("{base}/queues/{qid}/{tid}")),
        );
    }
    serde_json::to_vec(&v).expect("JSON values serialise")
}

/// The headers delivery `d` of `body` is posted with, signed at Unix time
/// `now`.
pub fn headers_of(d: &DueDelivery, body: &[u8], now: i64) -> Vec<(String, String)> {
    vec![
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), USER_AGENT.into()),
        (EVENT_HEADER.into(), d.event.clone()),
        (EVENT_ID_HEADER.into(), d.event_id.clone()),
        (TIMESTAMP_HEADER.into(), now.to_string()),
        (
            SIGNATURE_HEADER.into(),
            signature(d.secret.expose(), now, body),
        ),
    ]
}

/// One attempt of `d`, signed at `now`. A verification's 2xx answer
/// delivers it only when [`check_answer`] takes its body.
async fn attempt(sender: &dyn Sender, d: &DueDelivery, base: &str, now: i64) -> Outcome {
    let body = body_of(d, base);
    let headers = headers_of(d, &body, now);
    match clax_rt::time::timeout(TIMEOUT, sender.post(&d.url, &headers, &body)).await {
        Err(_) => Outcome::Failed {
            status: None,
            error: format!("no answer within {} s", TIMEOUT.as_secs()),
        },
        Ok(Err(e)) => Outcome::Failed {
            status: None,
            error: e,
        },
        Ok(Ok(a)) if (200..300).contains(&a.status) => {
            if d.event != VERIFY_EVENT {
                return Outcome::Delivered { status: a.status };
            }
            let checked = serde_json::from_str::<VerifyPayload>(&d.payload)
                .map_err(|e| format!("the verification's payload does not parse: {e}"))
                .and_then(|p| check_answer(d.secret.expose(), &p.challenge, &a.body, a.whole));
            match checked {
                Ok(()) => Outcome::Delivered { status: a.status },
                Err(error) => Outcome::Failed {
                    status: Some(a.status),
                    error,
                },
            }
        }
        Ok(Ok(a)) if (300..400).contains(&a.status) || a.status == 0 => Outcome::Failed {
            status: Some(a.status),
            error: "the endpoint answered a redirect, which webhooks do not follow".into(),
        },
        Ok(Ok(a)) => Outcome::Failed {
            status: Some(a.status),
            error: a.logged(),
        },
    }
}

/// The daemon's sender: reqwest, following no redirect and using no
/// proxy; with `public_only` (a hosted Clax) a name is resolved here and
/// only its public addresses are connected to, and an IP address in the
/// URL that is not public is refused.
#[cfg(not(target_arch = "wasm32"))]
pub struct ReqwestSender {
    client: reqwest::Client,
    public_only: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl ReqwestSender {
    #[must_use]
    pub fn new(public_only: bool) -> ReqwestSender {
        let mut b = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(TIMEOUT);
        if public_only {
            b = b.dns_resolver(Arc::new(PublicOnly));
        }
        ReqwestSender {
            client: b.build().expect("a reqwest client with default TLS"),
            public_only,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Sender for ReqwestSender {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: &'a [u8],
    ) -> SendFuture<'a> {
        Box::pin(async move {
            let parsed = url::Url::parse(url).map_err(|e| format!("{url} is not a URL: {e}"))?;
            if self.public_only
                && let Some(ip) = literal_ip(&parsed)
                && !clax_core::config::public_address(ip)
            {
                return Err(format!("{ip} is not a public address"));
            }
            let mut req = self.client.post(parsed);
            for (k, v) in headers {
                req = req.header(k, v);
            }
            let mut res = req.body(body.to_vec()).send().await.map_err(|e| {
                let mut msg = e.to_string();
                let mut src = std::error::Error::source(&e);
                while let Some(s) = src {
                    msg.push_str(": ");
                    msg.push_str(&s.to_string());
                    src = s.source();
                }
                msg
            })?;
            let status = res.status().as_u16();
            // At most MAX_ANSWER_BYTES are read: an endpoint's long answer
            // is not held in memory.
            let mut r = AnswerReader::new(status);
            let ended = loop {
                if !r.wants_more() {
                    break false;
                }
                match res.chunk().await {
                    Ok(Some(c)) => r.push(&c),
                    Ok(None) => break true,
                    Err(_) => break false,
                }
            };
            Ok(r.finish(ended))
        })
    }
}

/// The IP address `url` names as its host, if it names one.
#[cfg(not(target_arch = "wasm32"))]
fn literal_ip(url: &url::Url) -> Option<std::net::IpAddr> {
    match url.host()? {
        url::Host::Ipv4(a) => Some(a.into()),
        url::Host::Ipv6(a) => Some(a.into()),
        url::Host::Domain(_) => None,
    }
}

/// A resolver that answers only a name's public addresses.
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

/// The public ones of `addrs`.
#[cfg(not(target_arch = "wasm32"))]
fn public_addrs(addrs: impl Iterator<Item = std::net::SocketAddr>) -> Vec<std::net::SocketAddr> {
    addrs
        .filter(|a| clax_core::config::public_address(a.ip()))
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    /// A loopback receiver that answers every request 204.
    async fn receiver() -> u16 {
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
        port
    }

    /// A hosted Clax's sender never reaches a name or address that is not
    /// public, where a local Clax's does.
    #[tokio::test]
    async fn a_hosted_sender_reaches_no_loopback_endpoint() {
        let port = receiver().await;
        let local = ReqwestSender::new(false);
        let hosted = ReqwestSender::new(true);
        for url in [
            format!("http://localhost:{port}/hook"),
            format!("http://127.0.0.1:{port}/hook"),
        ] {
            let a = local.post(&url, &[], b"{}").await.unwrap();
            assert_eq!(a.status, 204, "{url}");
            let e = hosted.post(&url, &[], b"{}").await.unwrap_err();
            assert!(
                e.contains("no public address") || e.contains("not a public address"),
                "{url}: {e}"
            );
        }
    }

    #[test]
    fn only_public_answers_are_kept() {
        let addrs = [
            "10.0.0.5:443",
            "1.1.1.1:443",
            "[::1]:443",
            "169.254.169.254:80",
        ]
        .map(|a| a.parse::<std::net::SocketAddr>().unwrap());
        assert_eq!(
            public_addrs(addrs.into_iter()),
            ["1.1.1.1:443".parse::<std::net::SocketAddr>().unwrap()]
        );
    }
}
