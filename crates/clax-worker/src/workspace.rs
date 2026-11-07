//! ClaxBot's workspace on the Worker: a separate service holds the runs'
//! containers ([`clax_bot_remote`]), and the Worker holds its address and one
//! bearer secret. With `CLAX_WORKSPACE_SERVICE_URL` and the secret
//! `CLAX_WORKSPACE_SERVICE_TOKEN` both unset there is none ([`Setup::Off`]),
//! and the Worker runs as it does without one. With one of them alone, or an
//! address that is no service's, there is none either and the panel says so
//! ([`Setup::Unusable`]).
//!
//! `CLAX_WORKSPACE_PREVIEW_HOSTS` and `CLAX_WORKSPACE_FORGE_HOSTS` say where a
//! preview's link and a pull request's may point
//! ([`clax_bot_remote::LinkHosts`]): the service's word is not taken, and with
//! none set no such link goes to anyone. Every request names the instance, by
//! `CLAX_WORKSPACE_SERVICE_INSTANCE`, else `CLAX_INSTANCE`, and a service that
//! serves another refuses it. With neither set there is no workspace and the
//! panel says so: two Workers left unnamed would be one instance to a service,
//! and one's reap would end the other's containers.
//!
//! What fronts the service may ask a credential of its own, apart from the
//! bearer secret: the secrets `CLAX_WORKSPACE_ACCESS_CLIENT_ID` and
//! `CLAX_WORKSPACE_ACCESS_CLIENT_SECRET`, sent with every request, both or
//! neither. One alone is no workspace, and the panel says so.
//!
//! The service serves its operator's targets and no store link: it holds no
//! store and no key-encryption key. So a repository connected in Clax does not
//! run here, and [`facts`] says so.
//!
//! [`WorkerWorkspace`] is what the driver and the host hold: the remote
//! workspace, with its ends bounded so that one alarm cannot pass the
//! platform's wall limit waiting on a service that does not answer.

use clax_bot::run::OffersLapse;
use clax_bot::workspace::{Offer, PreviewSink, Proposal, Workspace};
use clax_bot_remote::{Kept, RemoteWorkspace, Unreachable};
use clax_bot_sandbox::HostError;
use clax_core::secret::Secret;
use clax_http::bot::RuntimeFacts;
use clax_store::bot_runs::Run;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// How often the workspace's timed work is done while it has any: well inside
/// the service's two-minute grace on a preview.
pub const TEND_EVERY: Duration = Duration::from_secs(30);
/// The longest that work takes of an alarm. What it did not get to stays.
pub const TEND_DEADLINE: Duration = Duration::from_secs(30);
/// How old the workspace's last word may be when a request that may change a
/// report is served, past which it is asked again: an idle Worker's offers
/// lapse, and a reopen read against none would start no run.
pub const TEND_FOR_A_REQUEST_AFTER: Duration = Duration::from_secs(5 * 60);
/// The longest that holds up a request.
pub const REQUEST_TEND_DEADLINE: Duration = Duration::from_secs(5);
/// The longest ending a run, or reaping, takes of an event: longer than the
/// service takes to answer. What was not done is kept, and done by
/// [`WorkerWorkspace::tend`] or the next reap.
pub const END_DEADLINE: Duration = Duration::from_secs(30);

/// The header that carries `CLAX_WORKSPACE_ACCESS_CLIENT_ID`.
const FRONT_ID_HEADER: &str = "cf-access-client-id";
/// The header that carries `CLAX_WORKSPACE_ACCESS_CLIENT_SECRET`.
const FRONT_SECRET_HEADER: &str = "cf-access-client-secret";

/// Extra headers for whatever fronts the service, each a credential.
pub type FrontHeaders = Vec<(&'static str, Secret)>;

/// A service the Worker may use.
pub struct Service {
    /// Its address, with no last `/`.
    pub url: String,
    pub secret: Secret,
    pub front: FrontHeaders,
    /// `CLAX_WORKSPACE_PREVIEW_HOSTS`.
    pub preview_hosts: String,
    /// `CLAX_WORKSPACE_FORGE_HOSTS`.
    pub forge_hosts: String,
    /// What every request names this instance as.
    pub instance: String,
}

/// What the settings' values for a [`Service`] are.
#[derive(Clone, Copy, Debug, Default)]
pub struct Settings<'a> {
    pub url: &'a str,
    pub secret: &'a str,
    /// `CLAX_WORKSPACE_ACCESS_CLIENT_ID`.
    pub front_id: &'a str,
    /// `CLAX_WORKSPACE_ACCESS_CLIENT_SECRET`.
    pub front_secret: &'a str,
    pub preview_hosts: &'a str,
    pub forge_hosts: &'a str,
    /// `CLAX_WORKSPACE_SERVICE_INSTANCE`.
    pub instance: &'a str,
    /// `CLAX_INSTANCE`.
    pub object: &'a str,
    /// `http` on a loopback host will do: the test vars.
    pub loopback: bool,
}

/// What the Worker's settings say of a workspace.
pub enum Setup {
    Off,
    /// Set up, and not usable: why, in words holding no secret.
    Unusable(String),
    On(Service),
}

/// `url` as a service's address: `https`, or with `loopback` (the test vars)
/// `http` on a loopback host, with no credentials, query or fragment.
fn service_url(url: &str, loopback: bool) -> Result<String, String> {
    const NOT_A_SERVICE: &str = "CLAX_WORKSPACE_SERVICE_URL must be an https:// address with no \
                                 credentials, query or fragment";
    let u = worker::Url::parse(url.trim()).map_err(|_| NOT_A_SERVICE.to_string())?;
    let local = matches!(u.host_str(), Some("127.0.0.1" | "localhost"));
    let scheme = u.scheme() == "https" || (u.scheme() == "http" && local && loopback);
    let plain = u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none();
    if u.host_str().is_none() || !scheme || !plain {
        return Err(NOT_A_SERVICE.to_string());
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}

/// The headers `set` asks for whatever fronts the service: both or neither.
fn front_headers(set: &Settings<'_>) -> Result<FrontHeaders, String> {
    let (id, secret) = (set.front_id.trim(), set.front_secret.trim());
    match (id.is_empty(), secret.is_empty()) {
        (true, true) => Ok(Vec::new()),
        (false, false) => Ok(vec![
            (FRONT_ID_HEADER, Secret::new(id.to_string())),
            (FRONT_SECRET_HEADER, Secret::new(secret.to_string())),
        ]),
        _ => Err(
            "the secrets CLAX_WORKSPACE_ACCESS_CLIENT_ID and CLAX_WORKSPACE_ACCESS_CLIENT_SECRET \
             are set together or not at all"
                .to_string(),
        ),
    }
}

/// What the settings' values make of a workspace (the module documentation).
#[must_use]
pub fn setup_of(set: &Settings<'_>) -> Setup {
    match (set.url.trim().is_empty(), set.secret.trim().is_empty()) {
        (true, true) => Setup::Off,
        (false, false) => match service_url(set.url, set.loopback) {
            Ok(url) => {
                let named = [set.instance, set.object].map(str::trim);
                let Some(instance) = named.into_iter().find(|n| !n.is_empty()) else {
                    return Setup::Unusable(
                        "a workspace service needs CLAX_WORKSPACE_SERVICE_INSTANCE or \
                         CLAX_INSTANCE to name this instance"
                            .to_string(),
                    );
                };
                let front = match front_headers(set) {
                    Ok(front) => front,
                    Err(why) => return Setup::Unusable(why),
                };
                Setup::On(Service {
                    url,
                    secret: Secret::new(set.secret.trim().to_string()),
                    front,
                    preview_hosts: set.preview_hosts.to_string(),
                    forge_hosts: set.forge_hosts.to_string(),
                    instance: instance.to_string(),
                })
            }
            Err(why) => Setup::Unusable(why),
        },
        _ => Setup::Unusable(
            "CLAX_WORKSPACE_SERVICE_URL and the secret CLAX_WORKSPACE_SERVICE_TOKEN are set \
             together or not at all"
                .to_string(),
        ),
    }
}

/// What the routes say of `setup`, with what the last incarnation kept. The Worker does not know who runs the service's containers or what
/// holds their network, so it answers as for a third party's, with no
/// allow-list. The queues are those of the snapshot it started with: until a
/// snapshot names one, its operator's target runs and the panel says only
/// that a link does not.
#[must_use]
pub fn facts(setup: &Setup, kept: Option<&Kept>) -> RuntimeFacts {
    match setup {
        Setup::Off => RuntimeFacts::default(),
        Setup::Unusable(_) => RuntimeFacts {
            configured: true,
            third_party: true,
            failed: true,
            operator_only: true,
            ..RuntimeFacts::default()
        },
        Setup::On(_) => RuntimeFacts {
            configured: true,
            third_party: true,
            operator_only: true,
            operator_queues: kept
                .and_then(|k| k.snapshot.as_ref())
                .map_or(Vec::new(), |s| s.queues.keys().cloned().collect()),
            ..RuntimeFacts::default()
        },
    }
}

/// `request`'s URL and headers at service `base`: the headers it names, as
/// given, the bearer secret, and the headers for what fronts it.
#[must_use]
pub fn outgoing(
    base: &str,
    secret: &str,
    front: &[(&'static str, Secret)],
    request: &clax_bot_remote::WireRequest,
) -> (String, Vec<(String, String)>) {
    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    headers.push(("authorization".to_string(), format!("Bearer {secret}")));
    for (name, value) in front {
        headers.push(((*name).to_string(), value.expose().to_string()));
    }
    (format!("{base}{}", request.path), headers)
}

/// `body`, whole, and no answer the moment it passes `limit`: what is past it
/// is never read.
///
/// # Errors
/// As above, and when the body breaks off.
pub async fn read_within<E>(
    body: impl Stream<Item = Result<Vec<u8>, E>>,
    limit: usize,
) -> Result<Vec<u8>, Unreachable> {
    let mut body = std::pin::pin!(body);
    let mut read = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| Unreachable)?;
        if read.len() + chunk.len() > limit {
            return Err(Unreachable);
        }
        read.extend_from_slice(&chunk);
    }
    Ok(read)
}

/// See the module documentation.
pub struct WorkerWorkspace {
    remote: RemoteWorkspace,
}

impl WorkerWorkspace {
    #[must_use]
    pub fn new(remote: RemoteWorkspace) -> WorkerWorkspace {
        WorkerWorkspace { remote }
    }

    /// Whether there is timed work to do ([`RemoteWorkspace::next_due_ms`]).
    #[must_use]
    pub fn has_work(&self) -> bool {
        self.remote.next_due_ms().is_some()
    }

    /// When timed work done every `every` is next worth trying, while it
    /// keeps failing ([`RemoteWorkspace::tend_held_until_ms`]).
    #[must_use]
    pub fn tend_held_until_ms(&self, every: Duration) -> Option<i64> {
        self.remote.tend_held_until_ms(every)
    }

    /// The timed work, for at most `deadline`
    /// ([`RemoteWorkspace::tend_within`]): a snapshot grown old, the previews,
    /// and the ends not answered.
    pub async fn tend(&self, deadline: Duration) {
        if !self.remote.tend_within(deadline).await {
            worker::console_warn!("the workspace's timed work ran out of time");
        }
    }
}

impl OffersLapse for WorkerWorkspace {
    fn offers_lapsed(&self) -> bool {
        self.remote.offers_lapsed()
    }

    fn lapse_reaches(&self, run: &Run, queue: Option<&str>) -> bool {
        self.remote.lapse_reaches(run, queue)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Workspace for WorkerWorkspace {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.remote.offer(run, queue)
    }

    fn target_private(&self, run: &Run, queue: Option<&str>) -> bool {
        self.remote.target_private(run, queue)
    }

    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.remote
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }

    fn resumable(&self) -> bool {
        self.remote.resumable()
    }

    async fn exec_keyed(
        &self,
        run: &Run,
        queue: Option<&str>,
        key: &str,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.remote
            .exec_keyed(run, queue, key, command, cwd, timeout_secs)
            .await
    }

    async fn resume_exec(&self, run: &Run, key: &str) -> Result<Option<Value>, HostError> {
        self.remote.resume_exec(run, key).await
    }

    async fn propose(&self, run: &Run, proposal: &Proposal) -> Result<Value, HostError> {
        self.remote.propose(run, proposal).await
    }

    fn previews(&self, sink: Arc<dyn PreviewSink>) {
        self.remote.previews(sink);
    }

    async fn reap(&self, active: &[String]) {
        if clax_rt::time::timeout(END_DEADLINE, self.remote.reap(active))
            .await
            .is_err()
        {
            worker::console_warn!("the workspace service did not reap in time");
        }
    }

    async fn finish(&self, run: &Run) {
        if clax_rt::time::timeout(END_DEADLINE, self.remote.finish(run))
            .await
            .is_err()
        {
            worker::console_warn!("run {} was not ended in time", run.id);
        }
    }
}

/// The Worker's `fetch`, to its service.
#[cfg(target_arch = "wasm32")]
mod fetch {
    use super::{FrontHeaders, Service, outgoing, read_within};
    use clax_bot_remote::wire::MAX_RESPONSE_BYTES;
    use clax_bot_remote::{Method, Transport, Unreachable, WireRequest, WireResponse};
    use clax_core::secret::Secret;

    pub struct FetchTransport {
        pub url: String,
        pub secret: Secret,
        pub front: FrontHeaders,
    }

    impl FetchTransport {
        pub fn of(service: &Service) -> FetchTransport {
            FetchTransport {
                url: service.url.clone(),
                secret: Secret::new(service.secret.expose().to_string()),
                front: service.front.clone(),
            }
        }
    }

    /// Aborts its request when dropped: a tick that gives up a poll leaves no
    /// connection behind, of the few a Worker may have waiting for headers.
    struct AbortOnDrop(Option<worker::AbortController>);

    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            if let Some(c) = self.0.take() {
                c.abort();
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl Transport for FetchTransport {
        /// A redirect is not followed, since it would carry the secret: it
        /// comes back as the status it is.
        async fn send(&self, request: WireRequest) -> Result<WireResponse, Unreachable> {
            let (url, headers) = outgoing(&self.url, self.secret.expose(), &self.front, &request);
            let mut init = worker::RequestInit::new();
            init.with_method(match request.method {
                Method::Get => worker::Method::Get,
                Method::Post => worker::Method::Post,
                Method::Delete => worker::Method::Delete,
            });
            init.with_redirect(worker::RequestRedirect::Manual);
            let h = worker::Headers::new();
            for (k, v) in &headers {
                h.set(k, v).map_err(|_| Unreachable)?;
            }
            init.with_headers(h);
            if let Some(body) = &request.body {
                let bytes = js_sys::Uint8Array::from(body.as_slice());
                init.with_body(Some(bytes.into()));
            }
            let req = worker::Request::new_with_init(&url, &init).map_err(|_| Unreachable)?;
            let abort = AbortOnDrop(Some(worker::AbortController::default()));
            let signal = abort.0.as_ref().map(worker::AbortController::signal);
            let fetch = worker::Fetch::Request(req);
            let mut res = match &signal {
                Some(s) => fetch.send_with_signal(s).await,
                None => fetch.send().await,
            }
            .map_err(|_| Unreachable)?;
            let status = res.status_code();
            let body = match res.stream() {
                Ok(body) => read_within(body, MAX_RESPONSE_BYTES).await?,
                // A body that is null, as a 204's is, has nothing to bound.
                Err(_) => res.bytes().await.map_err(|_| Unreachable)?,
            };
            Ok(WireResponse { status, body })
        }
    }
}

/// The workspace of `service`, starting from `kept`, what it keeps going to
/// `keeper`.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn workspace(
    service: &Service,
    keeper: Arc<dyn clax_bot_remote::Keeper>,
    kept: Option<Kept>,
) -> Option<Arc<WorkerWorkspace>> {
    use clax_bot_remote::LinkHosts;
    let transport = Arc::new(fetch::FetchTransport::of(service));
    let remote = RemoteWorkspace::new(transport, keeper, kept, service.instance.as_str())
        .with_preview_hosts(LinkHosts::parse(&service.preview_hosts))
        .with_forge_hosts(LinkHosts::parse(&service.forge_hosts));
    Some(Arc::new(WorkerWorkspace::new(remote)))
}

/// `fetch` is a Worker's.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn workspace(
    _service: &Service,
    _keeper: Arc<dyn clax_bot_remote::Keeper>,
    _kept: Option<Kept>,
) -> Option<Arc<WorkerWorkspace>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_bot_remote::wire::Snapshot;
    use clax_bot_remote::{Method, WireRequest};
    use std::collections::{BTreeMap, BTreeSet};

    const LOCAL: &str = "http://127.0.0.1:9000";

    fn set<'a>(url: &'a str, secret: &'a str) -> Settings<'a> {
        Settings {
            url,
            secret,
            ..Settings::default()
        }
    }

    #[test]
    fn with_neither_setting_there_is_no_workspace_and_the_panel_hears_nothing() {
        let setup = setup_of(&Settings {
            url: " ",
            preview_hosts: "*.example",
            ..Settings::default()
        });
        assert!(matches!(setup, Setup::Off));
        assert_eq!(facts(&setup, None), RuntimeFacts::default());
    }

    #[test]
    fn one_setting_alone_is_a_workspace_that_did_not_come_up() {
        for setup in [
            setup_of(&set("https://svc.example", "")),
            setup_of(&set("", "a-secret")),
        ] {
            let Setup::Unusable(why) = &setup else {
                panic!("not refused");
            };
            assert!(
                why.contains("together") && !why.contains("a-secret"),
                "{why}"
            );
            let f = facts(&setup, None);
            assert!(f.configured && f.failed && f.third_party, "{f:?}");
        }
    }

    #[test]
    fn a_service_is_https_unless_the_test_vars_allow_loopback() {
        let on = |url, loopback| {
            let s = Settings {
                loopback,
                instance: "I1",
                ..set(url, "s")
            };
            matches!(setup_of(&s), Setup::On(_))
        };
        assert!(on("https://svc.example", false));
        assert!(on(LOCAL, true));
        assert!(!on(LOCAL, false));
        assert!(!on("http://svc.example", true));
        assert!(!on("https://me:pw@svc.example", false));
        assert!(!on("https://svc.example/?a=1", false));
        assert!(!on("not a url", false));
    }

    #[test]
    fn the_instance_is_named_by_its_own_setting_else_the_objects_name() {
        let named = |instance, object| {
            let s = Settings {
                instance,
                object,
                ..set("https://svc.example", "s")
            };
            match setup_of(&s) {
                Setup::On(service) => service.instance,
                _ => panic!("refused"),
            }
        };
        assert_eq!(named("clax-dev", "other"), "clax-dev");
        assert_eq!(named("", "other"), "other");
    }

    #[test]
    fn a_service_with_no_name_for_the_instance_is_a_workspace_that_did_not_come_up() {
        let setup = setup_of(&Settings {
            instance: " ",
            object: "",
            ..set("https://svc.example", "s")
        });
        let Setup::Unusable(why) = &setup else {
            panic!("not refused");
        };
        assert!(why.contains("CLAX_WORKSPACE_SERVICE_INSTANCE"), "{why}");
        assert!(why.contains("CLAX_INSTANCE"), "{why}");
        let f = facts(&setup, None);
        assert!(f.configured && f.failed, "{f:?}");
    }

    #[test]
    fn with_no_service_set_no_name_is_wanted() {
        assert!(matches!(setup_of(&Settings::default()), Setup::Off));
    }

    #[tokio::test]
    async fn a_body_is_read_up_to_its_limit_and_no_further() {
        let chunks = |sizes: &[usize]| {
            let chunks: Vec<Result<Vec<u8>, ()>> =
                sizes.iter().map(|n| Ok(vec![b'x'; *n])).collect();
            futures_util::stream::iter(chunks)
        };
        let whole = read_within(chunks(&[3, 3, 4]), 10).await.unwrap();
        assert_eq!(whole.len(), 10);
        assert_eq!(
            read_within(chunks(&[]), 10).await.unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(read_within(chunks(&[3, 3, 5]), 10).await, Err(Unreachable));

        let pulled = std::cell::Cell::new(0);
        let endless = futures_util::stream::repeat_with(|| {
            pulled.set(pulled.get() + 1);
            Ok::<_, ()>(vec![b'x'; 4])
        });
        assert_eq!(read_within(endless, 10).await, Err(Unreachable));
        assert_eq!(pulled.get(), 3, "it stops at the chunk that passes");

        let broken = futures_util::stream::iter([Err::<Vec<u8>, _>(())]);
        assert_eq!(read_within(broken, 10).await, Err(Unreachable));
    }

    struct Answers(Value);

    #[async_trait::async_trait]
    impl clax_bot_remote::Transport for Answers {
        async fn send(
            &self,
            _request: WireRequest,
        ) -> Result<clax_bot_remote::WireResponse, Unreachable> {
            let body = self.0.to_string().into_bytes();
            Ok(clax_bot_remote::WireResponse { status: 200, body })
        }
    }

    struct Forgets;

    #[async_trait::async_trait]
    impl clax_bot_remote::Keeper for Forgets {
        async fn save(&self, _kept: &Kept) {}
    }

    /// Offers `queue`'s runs what `offer` says, with `queue` private or not.
    async fn offering(queue: &str, offer: Offer, private: bool) -> WorkerWorkspace {
        let snapshot = Snapshot {
            seq: 1,
            runs: BTreeMap::new(),
            queues: BTreeMap::from([(queue.to_string(), offer)]),
            others: Offer::default(),
            private_runs: BTreeSet::new(),
            private_queues: BTreeSet::from_iter(private.then(|| queue.to_string())),
            previews_until_ms: None,
        };
        let answers = Arc::new(Answers(serde_json::to_value(&snapshot).unwrap()));
        let remote = RemoteWorkspace::new(answers, Arc::new(Forgets), None, "I1");
        let w = WorkerWorkspace::new(remote);
        assert!(w.offers_lapsed());
        w.reap(&[]).await;
        assert!(!w.offers_lapsed());
        w
    }

    #[tokio::test]
    async fn a_target_is_private_on_the_services_word_and_not_by_its_offer() {
        let run = clax_bot_remote::wire::RunRef {
            id: "R1".to_string(),
            thread_ids: Vec::new(),
        }
        .to_run();
        let confidential = Offer::CHANGE.confidential();
        let public = offering("Q1", confidential.clone(), false).await;
        assert!(public.offer(&run, Some("Q1")).confidential);
        assert!(!public.target_private(&run, Some("Q1")));
        let private = offering("Q1", confidential, true).await;
        assert!(private.target_private(&run, Some("Q1")));
    }

    #[test]
    fn a_workspace_says_it_runs_its_operators_repositories_alone() {
        let setup = setup_of(&Settings {
            instance: "I1",
            ..set("https://svc.example/", "s")
        });
        let Setup::On(service) = &setup else {
            panic!("refused");
        };
        assert_eq!(service.url, "https://svc.example");
        let kept = Kept {
            snapshot: Some(Snapshot {
                seq: 1,
                runs: BTreeMap::new(),
                queues: BTreeMap::from([("Q1".to_string(), Offer::CHANGE)]),
                others: Offer::default(),
                private_runs: BTreeSet::new(),
                private_queues: BTreeSet::new(),
                previews_until_ms: None,
            }),
            ..Kept::default()
        };
        let f = facts(&setup, Some(&kept));
        assert!(f.configured && f.third_party && f.operator_only && !f.failed);
        assert!(!f.egress_held);
        assert_eq!(f.operator_queues, ["Q1"]);
        assert!(facts(&setup, None).operator_queues.is_empty());
    }

    #[test]
    fn a_request_carries_its_headers_as_given_and_the_bearer_secret() {
        let r = WireRequest {
            method: Method::Get,
            path: "/v1/previews".to_string(),
            headers: vec![("clax-workspace-wire", "1".to_string())],
            body: None,
        };
        let (url, headers) = outgoing("https://svc.example", "s3", &[], &r);
        assert_eq!(url, "https://svc.example/v1/previews");
        assert_eq!(
            headers,
            [
                ("clax-workspace-wire".to_string(), "1".to_string()),
                ("authorization".to_string(), "Bearer s3".to_string()),
            ]
        );

        let Setup::On(service) = setup_of(&Settings {
            front_id: "the-id",
            front_secret: "the-secret",
            instance: "I1",
            ..set("https://svc.example", "s3")
        }) else {
            panic!("refused");
        };
        let (_, headers) = outgoing(&service.url, "s3", &service.front, &r);
        assert_eq!(
            headers[2..],
            [
                ("cf-access-client-id".to_string(), "the-id".to_string()),
                (
                    "cf-access-client-secret".to_string(),
                    "the-secret".to_string()
                ),
            ]
        );
    }

    #[test]
    fn what_fronts_a_service_takes_both_of_its_headers_or_neither() {
        for (front_id, front_secret) in [("the-id", " "), ("", "the-secret")] {
            let setup = setup_of(&Settings {
                front_id,
                front_secret,
                instance: "I1",
                ..set("https://svc.example", "s")
            });
            let Setup::Unusable(why) = &setup else {
                panic!("not refused");
            };
            assert!(why.contains("together"), "{why}");
            assert!(
                !why.contains("the-id") && !why.contains("the-secret"),
                "{why}"
            );
            let f = facts(&setup, None);
            assert!(f.configured && f.failed, "{f:?}");
        }
        let neither = setup_of(&Settings {
            instance: "I1",
            ..set("https://svc.example", "s")
        });
        assert!(matches!(neither, Setup::On(s) if s.front.is_empty()));
    }

    /// What the workspace service's routes take of an ID and a key.
    fn well_formed(id: &str, limit: usize, extra: &[u8]) -> bool {
        let allowed = |b: &u8| b.is_ascii_alphanumeric() || b"_-".contains(b) || extra.contains(b);
        id.len() <= limit
            && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && id.as_bytes().iter().all(allowed)
    }

    #[test]
    fn the_ids_a_worker_mints_are_ones_the_service_takes() {
        // Runs and threads are ULIDs, and a call's key is the run's and two
        // numbers (`clax_bot::host::OpCtx::key`).
        for _ in 0..100 {
            let id = clax_core::new_ulid();
            assert!(well_formed(&id, 128, b""), "{id}");
            let key = format!("{id}:{}:{}", i64::MAX, i64::MAX);
            assert!(well_formed(&key, 192, b":"), "{key}");
        }
    }
}
