//! An in-process HTTP endpoint for webhook tests ([`Receiver`]): it keeps
//! every request it is sent and answers each with the status a test set
//! (a 3xx with a `Location` of its own `/elsewhere`), or as a test's
//! [`Receiver::respond`] says; [`Receiver::verify_with`] answers
//! verifications as a receiver holding the secret does.

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

/// One request the receiver was sent.
#[derive(Clone, Debug)]
pub struct Received {
    pub path: String,
    /// Header names lowercased.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Received {
    /// The header `name` (any case).
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map_or("", String::as_str)
    }

    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }

    /// Whether it is a verification (`webhook.verify`).
    pub fn is_verification(&self) -> bool {
        self.header("clax-event") == clax_core::webhook::VERIFY_EVENT
    }
}

/// How the receiver answers a request: a status and a body.
pub type Respond = dyn Fn(&Received) -> (u16, Vec<u8>) + Send + Sync;

/// A loopback HTTP endpoint at [`Receiver::url`], answering 200 until
/// [`Receiver::answer`] or [`Receiver::respond`] says otherwise; it serves
/// until dropped.
pub struct Receiver {
    /// `http://127.0.0.1:<port>/hook`.
    pub url: String,
    got: Arc<Mutex<Vec<Received>>>,
    status: Arc<AtomicU16>,
    respond: Arc<Mutex<Option<Arc<Respond>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Receiver {
    pub async fn start() -> Receiver {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let got: Arc<Mutex<Vec<Received>>> = Arc::default();
        let status = Arc::new(AtomicU16::new(200));
        let respond: Arc<Mutex<Option<Arc<Respond>>>> = Arc::default();
        let (g, st, rs) = (got.clone(), status.clone(), respond.clone());
        let elsewhere = format!("http://127.0.0.1:{port}/elsewhere");
        let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let (g, st, rs, elsewhere) = (g.clone(), st.clone(), rs.clone(), elsewhere.clone());
            async move {
                let r = Received {
                    path: uri.path().to_string(),
                    headers: headers
                        .iter()
                        .filter_map(|(k, v)| {
                            Some((
                                k.as_str().to_ascii_lowercase(),
                                v.to_str().ok()?.to_string(),
                            ))
                        })
                        .collect(),
                    body: body.to_vec(),
                };
                let f = rs.lock().unwrap().clone();
                let (code, text) = match f {
                    Some(f) => f(&r),
                    None => (st.load(Ordering::SeqCst), b"receiver says so".to_vec()),
                };
                g.lock().unwrap().push(r);
                let code = StatusCode::from_u16(code).unwrap();
                let mut h = HeaderMap::new();
                if code.is_redirection() {
                    h.insert("location", elsewhere.parse().unwrap());
                }
                (code, h, text)
            }
        });
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Receiver {
            url: format!("http://127.0.0.1:{port}/hook"),
            got,
            status,
            respond,
            task,
        }
    }

    /// Answers every request from now on with `status` and a short text.
    pub fn answer(&self, status: u16) {
        *self.respond.lock().unwrap() = None;
        self.status.store(status, Ordering::SeqCst);
    }

    /// Answers every request from now on as `f` says.
    pub fn respond(&self, f: impl Fn(&Received) -> (u16, Vec<u8>) + Send + Sync + 'static) {
        *self.respond.lock().unwrap() = Some(Arc::new(f));
    }

    /// Answers a verification from now on as a receiver holding `secret`
    /// does (200 `{"answer": <the challenge's HMAC>}`), and every other
    /// request with `status`.
    pub fn verify_with(&self, secret: &str, status: u16) {
        let secret = secret.to_string();
        self.respond(move |r| {
            if !r.is_verification() {
                return (status, b"receiver says so".to_vec());
            }
            let challenge = r.json()["challenge"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let answer = clax_core::webhook::challenge_answer(&secret, &challenge);
            (
                200,
                serde_json::to_vec(&serde_json::json!({ "answer": answer })).unwrap(),
            )
        });
    }

    /// The requests sent since the last call, oldest first.
    pub fn take(&self) -> Vec<Received> {
        std::mem::take(&mut *self.got.lock().unwrap())
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}
