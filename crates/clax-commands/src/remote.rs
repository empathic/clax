//! The CLI's side of remote mode (spec §16, M26): [`SignedClient`] sends
//! each request [`crate::client::Client`] built to a remote daemon, signed
//! with `clax-ed25519 v2` (spec §6.6) by `clax_core::remote`'s shared
//! helpers, and retries it by [`Retry`]'s rule: once on `stale_request`
//! with the offset the daemon's `Date` gives (never more than
//! `MAX_OFFSET_S`), and on `limit_reached` and `busy` after 1, 2 and 4
//! seconds with jitter, signing every attempt anew. It reads at most
//! [`MAX_RESPONSE_BYTES`] of an answer, and the client it sends with
//! follows no redirect.
//!
//! It sends only requests `Client` built (with the agent-channel marker
//! when the CLI is an agent channel); it makes no HTTP client of its own.

use crate::term::clean_line;
use anyhow::anyhow;
use clax_core::remote::{
    Credential, MAX_RESPONSE_BYTES, Next, Retry, Signer, clock_skew_message, path_and_query,
    prepare_headers, refusal_message, sign_headers,
};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

/// How a client waits between attempts: the thread's sleep, or a test's
/// record of the waits.
pub type Sleep = Arc<dyn Fn(Duration) + Send + Sync>;
/// A jitter value in `[0, 1)` for each backoff.
pub type Jitter = Arc<dyn Fn() -> f64 + Send + Sync>;

/// The remote daemon could not be reached (spec §22): the CLI says so,
/// naming it, and starts nothing.
#[derive(Debug)]
pub struct Unreachable {
    pub url: String,
    pub why: String,
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "daemon_unreachable: the Clax at {} does not answer ({})",
            self.url, self.why
        )
    }
}

impl std::error::Error for Unreachable {}

/// A daemon's refusal: one [`SignedClient::send`] gave up on
/// (`stale_request`, `limit_reached`, `busy`, `clock_skew`, or another 401,
/// 429 or 503), or any error answer `Client` gets; worded `<code>:
/// <message>`, with the daemon's words made safe to print.
#[derive(Debug)]
pub struct Refused {
    /// The HTTP status of the answer.
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Refused {}

/// Reads at most [`MAX_RESPONSE_BYTES`] of `res`'s body.
///
/// # Errors
/// A body declared or found larger, or a read that fails.
fn capped_body(res: &mut reqwest::blocking::Response) -> anyhow::Result<Vec<u8>> {
    let too_big = || anyhow!("the daemon's answer is larger than {MAX_RESPONSE_BYTES} bytes");
    if res.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES) {
        return Err(too_big());
    }
    let mut body = Vec::new();
    res.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(too_big());
    }
    Ok(body)
}

/// Signs and sends requests to one remote daemon.
#[derive(Clone)]
pub struct SignedClient {
    signer: Arc<Signer>,
    sleep: Sleep,
    jitter: Jitter,
}

impl SignedClient {
    /// A client signing with `signer`, waiting on the thread's clock.
    pub fn new(signer: Arc<Signer>) -> SignedClient {
        SignedClient {
            signer,
            sleep: Arc::new(std::thread::sleep),
            jitter: Arc::new(clax_core::remote::jitter),
        }
    }

    /// This client waiting with `sleep`.
    #[cfg(test)]
    pub fn with_sleep(mut self, sleep: Sleep) -> SignedClient {
        self.sleep = sleep;
        self
    }

    /// This client drawing its jitter from `jitter`.
    #[cfg(test)]
    pub fn with_jitter(mut self, jitter: Jitter) -> SignedClient {
        self.jitter = jitter;
        self
    }

    pub fn signer(&self) -> &Arc<Signer> {
        &self.signer
    }

    /// Sends the request `req` builds, as `cred`: each attempt names the
    /// signer's host, carries the agent headers it has trimmed of
    /// surrounding spaces and tabs, and is signed now. A refusal that
    /// [`Retry`] gives up on is a [`Refused`]; any other answer is returned
    /// with its body read whole, at most [`MAX_RESPONSE_BYTES`] (`stream`:
    /// a success returned unread, for a reader that takes it a line at a
    /// time or under its own cap; a refusal is still read whole).
    ///
    /// # Errors
    /// A body that streams (it cannot be hashed), duplicate agent headers,
    /// [`Unreachable`] when no connection is made, a refusal given up on,
    /// an answer too large.
    pub fn send(
        &self,
        req: reqwest::blocking::RequestBuilder,
        cred: Credential<'_>,
        stream: bool,
    ) -> anyhow::Result<reqwest::blocking::Response> {
        let (client, built) = req.build_split();
        let mut built = built?;
        let body = match built.body() {
            None => Vec::new(),
            Some(b) => b
                .as_bytes()
                .ok_or_else(|| anyhow!("a streamed body cannot be signed"))?
                .to_vec(),
        };
        let pq = path_and_query(built.url());
        let method = built.method().as_str().to_string();
        prepare_headers(&self.signer, built.headers_mut(), cred).map_err(|e| anyhow!(e))?;
        let mut retry = Retry::default();
        loop {
            let mut attempt = built
                .try_clone()
                .ok_or_else(|| anyhow!("a streamed body cannot be signed"))?;
            sign_headers(
                &self.signer,
                &method,
                &pq,
                &body,
                cred,
                attempt.headers_mut(),
            )
            .map_err(|e| anyhow!(e))?;
            let mut res = client.execute(attempt).map_err(|e| {
                if e.is_connect() {
                    anyhow::Error::new(Unreachable {
                        url: self.signer.base().to_string(),
                        why: e.to_string(),
                    })
                } else {
                    anyhow::Error::new(e)
                }
            })?;
            let status = res.status().as_u16();
            if !matches!(status, 401 | 429 | 503) {
                // Only a success streams: a refusal's body is read under
                // the cap, as every other answer is.
                if stream && res.status().is_success() {
                    return Ok(res);
                }
                let bytes = capped_body(&mut res)?;
                let mut whole = http::Response::builder().status(res.status());
                if let Some(h) = whole.headers_mut() {
                    *h = res.headers().clone();
                }
                return Ok(whole.body(bytes)?.into());
            }
            let date = res
                .headers()
                .get(reqwest::header::DATE)
                .and_then(|d| d.to_str().ok())
                .map(str::to_string);
            let body: serde_json::Value = capped_body(&mut res)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let code = body["error"]["code"]
                .as_str()
                .unwrap_or("error")
                .to_string();
            match retry.next(
                status,
                &code,
                date.as_deref(),
                &self.signer,
                (self.jitter)(),
            ) {
                Next::Resign => {}
                Next::After(d) => (self.sleep)(d),
                Next::ClockSkew(o) => {
                    return Err(Refused {
                        status,
                        code: "clock_skew".into(),
                        message: clock_skew_message(o),
                    }
                    .into());
                }
                Next::GiveUp => {
                    let message = body["error"]["message"]
                        .as_str()
                        .unwrap_or("request failed");
                    return Err(Refused {
                        status,
                        message: clean_line(&refusal_message(&code, message, &self.signer)),
                        code: clean_line(&code),
                    }
                    .into());
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
pub(crate) mod tests {
    use super::*;
    use clax_core::secret::Secret;
    use clax_core::sig;
    use sha2::Digest;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::Mutex;

    /// One request the fake daemon received.
    #[derive(Debug, Clone)]
    pub struct Seen {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Seen {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    /// One answer of a [`FakeDaemon`]: status, extra headers, JSON body.
    pub type Answer = (u16, Vec<(String, String)>, serde_json::Value);

    /// A daemon that answers each connection with the next of `answers`
    /// (status, extra headers, JSON body) and records what it was sent.
    pub struct FakeDaemon {
        pub base: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl FakeDaemon {
        pub fn start(answers: Vec<Answer>) -> FakeDaemon {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            std::thread::spawn(move || {
                for (status, extra, body) in answers {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();
                    let mut headers = Vec::new();
                    loop {
                        let mut h = String::new();
                        r.read_line(&mut h).unwrap();
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        let (k, v) = h.split_once(':').unwrap();
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                    let len = headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map_or(0, |(_, v)| v.parse().unwrap());
                    let mut body_in = vec![0; len];
                    r.read_exact(&mut body_in).unwrap();
                    log.lock().unwrap().push(Seen {
                        method,
                        path,
                        headers,
                        body: body_in,
                    });
                    let text = body.to_string();
                    // An answer's own content-length stands in for the
                    // body's real one (a daemon that lies about it).
                    let declared = extra
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
                    let mut out = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\nconnection: close\r\n"
                    );
                    if !declared {
                        out.push_str(&format!("content-length: {}\r\n", text.len()));
                    }
                    for (k, v) in extra {
                        out.push_str(&format!("{k}: {v}\r\n"));
                    }
                    out.push_str("\r\n");
                    out.push_str(&text);
                    let mut s = stream;
                    s.write_all(out.as_bytes()).unwrap();
                }
            });
            FakeDaemon { base, seen }
        }

        pub fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    fn refusal(code: &str) -> serde_json::Value {
        serde_json::json!({"error": {"code": code, "message": format!("{code} now")}})
    }

    /// A client of `d` whose waits are recorded, with no jitter, signing
    /// with a new key at `now`.
    fn client(d: &FakeDaemon, now: i64) -> (SignedClient, Arc<Mutex<Vec<Duration>>>) {
        let signer = Signer::new(clax_core::sig::KeyPair::generate(), &d.base)
            .unwrap()
            .with_clock(Arc::new(move || now));
        let waits = Arc::new(Mutex::new(Vec::new()));
        let w = waits.clone();
        let c = SignedClient::new(Arc::new(signer))
            .with_sleep(Arc::new(move |d| w.lock().unwrap().push(d)))
            .with_jitter(Arc::new(|| 0.0));
        (c, waits)
    }

    /// Whether `s` carries a signature by `signer`'s key over what it sent.
    fn verifies(s: &Seen, signer: &Signer, token: &str) -> bool {
        let h = sig::parse_header(s.header("clax-signature").unwrap()).unwrap();
        let agent =
            sig::agent_header_values(s.headers.iter().map(|(k, v)| (k.as_str(), v.as_bytes())))
                .unwrap();
        sig::verify(
            &signer.key().public(),
            &sig::SignInput {
                method: &s.method,
                path_and_query: &s.path,
                host: s.header("host").unwrap(),
                t: h.t,
                nonce: &h.nonce,
                body_sha256: sha2::Sha256::digest(&s.body).into(),
                token_sha256: Some(clax_core::secret::sha256(token)),
                agent_headers_sha256: sig::agent_headers_sha256(&agent),
            },
            &h.sig,
        )
    }

    fn upload(c: &SignedClient, base: &str, token: &Secret) -> anyhow::Result<u16> {
        let http = reqwest::blocking::Client::new();
        let req = http
            .post(format!("{base}/api/artifacts/k3m9x2a7b8c4/assets"))
            .header("x-clax-channel", "agent ")
            .body(b"the asset's bytes".to_vec());
        c.send(req, Credential::Grant(token), false)
            .map(|r| r.status().as_u16())
    }

    #[test]
    fn client_retries_busy_with_backoff() {
        let token = Secret::new("cxg_t".into());
        // Busy twice, then the upload is made: two re-signed retries.
        let d = FakeDaemon::start(vec![
            (503, vec![], refusal("busy")),
            (503, vec![], refusal("busy")),
            (201, vec![], serde_json::json!({"asset": {"id": "a1"}})),
        ]);
        let (c, waits) = client(&d, 1_760_000_000);
        assert_eq!(upload(&c, &d.base, &token).unwrap(), 201);
        assert_eq!(
            *waits.lock().unwrap(),
            [Duration::from_secs(1), Duration::from_secs(2)]
        );
        let seen = d.seen();
        assert_eq!(seen.len(), 3);
        let nonces: std::collections::HashSet<String> = seen
            .iter()
            .map(|s| {
                sig::parse_header(s.header("clax-signature").unwrap())
                    .unwrap()
                    .nonce
            })
            .collect();
        assert_eq!(nonces.len(), 3, "each attempt is signed anew");
        for s in &seen {
            assert!(verifies(s, c.signer(), "cxg_t"), "{s:?}");
            assert_eq!(s.header("authorization"), Some("Clax-Grant cxg_t"));
            assert_eq!(s.header("x-clax-channel"), Some("agent"), "trimmed");
            assert_eq!(s.body, b"the asset's bytes");
        }
        // Four limits: three retries, then the refusal is the answer.
        let d = FakeDaemon::start(
            (0..4)
                .map(|_| (429, vec![], refusal("limit_reached")))
                .collect(),
        );
        let (c, waits) = client(&d, 1_760_000_000);
        let e = upload(&c, &d.base, &token).unwrap_err().to_string();
        assert!(e.starts_with("limit_reached:"), "{e}");
        assert_eq!(
            *waits.lock().unwrap(),
            [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4)
            ]
        );
        assert_eq!(d.seen().len(), 4);
    }

    #[test]
    fn a_stale_request_is_signed_again_with_the_daemons_time() {
        let token = Secret::new("cxg_t".into());
        let ahead = chrono::DateTime::from_timestamp(1_760_000_300, 0)
            .unwrap()
            .to_rfc2822();
        let d = FakeDaemon::start(vec![
            (
                401,
                vec![("date".into(), ahead.clone())],
                refusal("stale_request"),
            ),
            (200, vec![], serde_json::json!({"artifacts": []})),
        ]);
        let (c, waits) = client(&d, 1_760_000_000);
        let http = reqwest::blocking::Client::new();
        let res = c
            .send(
                http.get(format!("{}/api/artifacts", d.base)),
                Credential::Grant(&token),
                false,
            )
            .unwrap();
        assert_eq!(res.status(), 200);
        assert!(waits.lock().unwrap().is_empty());
        let ts: Vec<i64> = d
            .seen()
            .iter()
            .map(|s| {
                sig::parse_header(s.header("clax-signature").unwrap())
                    .unwrap()
                    .t
            })
            .collect();
        assert_eq!(ts, [1_760_000_000, 1_760_000_300]);
        // A second stale answer is the answer.
        let d = FakeDaemon::start(vec![
            (
                401,
                vec![("date".into(), ahead.clone())],
                refusal("stale_request"),
            ),
            (401, vec![("date".into(), ahead)], refusal("stale_request")),
        ]);
        let (c, _) = client(&d, 1_760_000_000);
        let e = c
            .send(
                http.get(format!("{}/api/artifacts", d.base)),
                Credential::Grant(&token),
                false,
            )
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("stale_request:"), "{e}");
    }

    /// A forged `Date` far ahead (an on-path peer answering for the
    /// daemon) teaches no offset: nothing is signed for that time.
    #[test]
    fn a_date_a_year_ahead_signs_nothing_for_it() {
        let token = Secret::new("cxg_t".into());
        let year = chrono::DateTime::from_timestamp(1_760_000_000 + 365 * 86_400, 0)
            .unwrap()
            .to_rfc2822();
        let d = FakeDaemon::start(vec![
            (401, vec![("date".into(), year)], refusal("stale_request")),
            (200, vec![], serde_json::json!({"artifacts": []})),
        ]);
        let (c, _) = client(&d, 1_760_000_000);
        let http = reqwest::blocking::Client::new();
        let e = c
            .send(
                http.get(format!("{}/api/artifacts", d.base)),
                Credential::Grant(&token),
                false,
            )
            .unwrap_err();
        let r = e.downcast_ref::<Refused>().expect("a refusal");
        assert_eq!(r.code, "clock_skew");
        assert!(
            r.message.contains("set this machine's clock"),
            "{}",
            r.message
        );
        assert_eq!(d.seen().len(), 1, "no second signature");
        assert_eq!(c.signer().offset(), 0);
    }

    /// An answer larger than the cap is refused unread, and daemon words
    /// are made safe to print.
    #[test]
    fn answers_are_capped_and_refusals_cleaned() {
        let token = Secret::new("cxg_t".into());
        let huge = (clax_core::remote::MAX_RESPONSE_BYTES + 1).to_string();
        let d = FakeDaemon::start(vec![
            (
                200,
                vec![("content-length".into(), huge)],
                serde_json::json!({}),
            ),
            (
                503,
                vec![],
                serde_json::json!({"error": {"code": "\u{1b}]0;x\u{7}", "message": "\u{1b}[31mred"}}),
            ),
        ]);
        let (c, _) = client(&d, 1_760_000_000);
        let http = reqwest::blocking::Client::new();
        let get = || http.get(format!("{}/api/artifacts", d.base));
        let e = c
            .send(get(), Credential::Grant(&token), false)
            .unwrap_err()
            .to_string();
        assert!(e.contains("larger than"), "{e}");
        let e = c
            .send(get(), Credential::Grant(&token), false)
            .unwrap_err()
            .to_string();
        assert!(!e.contains('\u{1b}') && !e.contains('\u{7}'), "{e:?}");
    }

    /// A streamed request streams only a success: a refusal's body is read
    /// under the cap like any answer's, so a huge error body is refused
    /// unread.
    #[test]
    fn a_streamed_refusal_is_capped() {
        let token = Secret::new("cxg_t".into());
        let huge = (clax_core::remote::MAX_RESPONSE_BYTES + 1).to_string();
        let d = FakeDaemon::start(vec![
            (
                404,
                vec![("content-length".into(), huge)],
                refusal("not_found"),
            ),
            (404, vec![], refusal("not_found")),
            (200, vec![], serde_json::json!({"ok": true})),
        ]);
        let (c, _) = client(&d, 1_760_000_000);
        let http = reqwest::blocking::Client::new();
        let get = || http.get(format!("{}/api/artifacts/k3m9x2a7b8c4/clip", d.base));
        let e = c
            .send(get(), Credential::Grant(&token), true)
            .unwrap_err()
            .to_string();
        assert!(e.contains("larger than"), "{e}");
        let res = c.send(get(), Credential::Grant(&token), true).unwrap();
        assert_eq!(res.status().as_u16(), 404);
        assert_eq!(
            res.json::<serde_json::Value>().unwrap(),
            refusal("not_found")
        );
        let res = c.send(get(), Credential::Grant(&token), true).unwrap();
        assert_eq!(res.status().as_u16(), 200);
    }

    #[test]
    fn an_unreachable_daemon_is_named() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let signer = Signer::new(clax_core::sig::KeyPair::generate(), &base).unwrap();
        let c = SignedClient::new(Arc::new(signer));
        let token = Secret::new("cxg_t".into());
        let http = reqwest::blocking::Client::new();
        let e = c
            .send(
                http.get(format!("{base}/api/artifacts")),
                Credential::Grant(&token),
                false,
            )
            .unwrap_err();
        assert!(e.downcast_ref::<Unreachable>().is_some(), "{e}");
        assert!(e.to_string().contains(&base), "{e}");
        assert!(!e.to_string().contains("cxg_t"));
    }
}
