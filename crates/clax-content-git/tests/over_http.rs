//! The store over repositories it writes through receive-pack, as it writes
//! Cloudflare Artifacts: a [`PushingRepo`] whose transport serves each
//! request in process with clax-git's smart-HTTP server (which refuses
//! deleting `main`, as git's receive-pack does), with and without the
//! `atomic` capability, refusing non-fast-forward updates of `main` (as git
//! does with `receive.denyNonFastForwards`, and the stand-in by default) or
//! taking them. Races are made at the point a remote decides them:
//! just before the store's first receive-pack POST, after its read of the
//! refs.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::content::memory::MemContent;
use clax_core::content::{ContentStore, contract, wait};
use clax_git::client::{ObjectReader, PushingRepo, Remote};
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::memory::{MemoryHost, MemoryRepo};
use clax_git::pktline::{self, Packet};
use clax_git::server::{Access, GitRequest, ReceiveOptions, ServeOptions, serve, split_git_path};
use clax_git::versions::repo_name;
use clax_git::{MAIN, Object, ObjectId, Repo, RepoHost};

use crate::common::{clock, committer, write};

type Hook = Box<dyn FnOnce() -> Option<HttpResponse> + Send>;

/// Serves smart HTTP for `host`'s repositories in process. Without
/// `atomic`, the receive-pack advertisement leaves the capability out, so
/// the client sends one update per request; with `back`, a non-fast-forward
/// update of `main` is taken. A hook set with
/// [`InProcess::before_next_push`] runs once before the next receive-pack
/// POST, and answers it instead when it returns a response.
#[derive(Clone)]
struct InProcess {
    host: MemoryHost,
    atomic: bool,
    back: bool,
    hook: Arc<Mutex<Option<Hook>>>,
}

impl InProcess {
    fn before_next_push(&self, f: impl FnOnce() -> Option<HttpResponse> + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }
}

impl Transport for InProcess {
    async fn send(&self, request: HttpRequest) -> clax_git::Result<HttpResponse> {
        let (path, query) = match request.url.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (request.url.as_str(), None),
        };
        let path = path.strip_prefix("http://git.test").expect("a test URL");
        let (name, rest) = split_git_path(path).expect("a git path");
        if request.method == "POST" && rest == "git-receive-pack" {
            let hook = self.hook.lock().unwrap().take();
            if let Some(answer) = hook.and_then(|h| h()) {
                return Ok(answer);
            }
        }
        let repo = self.host.repo(name).expect("the repository");
        let content_encoding = request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
            .map(|(_, v)| v.as_str());
        let options = ServeOptions {
            // Clax's own writes: the store owns its version tags.
            receive: ReceiveOptions {
                protect_version_tags: false,
                deny_non_fast_forwards: !self.back,
                ..ReceiveOptions::default()
            },
            ..ServeOptions::default()
        };
        let git = GitRequest {
            method: request.method,
            rest,
            query,
            content_encoding,
            body: &request.body,
        };
        let mut response = serve(&repo, Access::Write, &git, &options).await.response;
        if !self.atomic && rest == "info/refs" && query == Some("service=git-receive-pack") {
            response.body = without_atomic(&response.body);
        }
        Ok(response)
    }
}

/// `advertisement` with `atomic` dropped from its capabilities.
fn without_atomic(advertisement: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in pktline::read_all(advertisement).expect("pkt-lines") {
        match p {
            Packet::Data(d) => {
                let text = String::from_utf8_lossy(d).replace(" atomic", "");
                pktline::write(&mut out, text.as_bytes());
            }
            Packet::Flush => out.extend_from_slice(pktline::FLUSH),
            other => panic!("unexpected packet {other:?}"),
        }
    }
    out
}

/// Reads objects straight from the repository, as the Artifacts binding
/// reads them; only writes go over receive-pack.
struct Reader(MemoryRepo);

impl ObjectReader for Reader {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        self.0.read_object(id).await
    }
}

/// `inner`'s repositories, written through [`InProcess`].
#[derive(Clone)]
struct HttpHost {
    inner: MemoryHost,
    transport: InProcess,
}

impl HttpHost {
    fn new(atomic: bool, back: bool) -> HttpHost {
        let inner = MemoryHost::new();
        HttpHost {
            transport: InProcess {
                host: inner.clone(),
                atomic,
                back,
                hook: Arc::default(),
            },
            inner,
        }
    }
}

impl RepoHost for HttpHost {
    type Repo = PushingRepo<Reader, InProcess>;

    async fn open(&self, name: &str) -> clax_git::Result<Option<Self::Repo>> {
        Ok(self.inner.repo(name).map(|repo| PushingRepo {
            reader: Reader(repo),
            remote: Remote::new(format!("http://git.test/git/{name}.git"), "token"),
            transport: self.transport.clone(),
        }))
    }
    async fn create(&self, name: &str) -> clax_git::Result<()> {
        self.inner.create(name).await
    }
    async fn fork(&self, from: &str, to: &str) -> clax_git::Result<()> {
        self.inner.fork(from, to).await
    }
    async fn delete(&self, name: &str) -> clax_git::Result<()> {
        self.inner.delete(name).await
    }
    fn can_prune(&self) -> bool {
        false
    }

    async fn prune(&self, name: &str) -> clax_git::Result<()> {
        self.inner.prune(name).await
    }
}

fn store(host: &HttpHost) -> GitContent<HttpHost, MemContent> {
    GitContent::new(host.clone(), MemContent::new(), committer(), clock())
}

/// A store writing straight to `host`'s repositories: another process.
fn racer(host: &HttpHost) -> GitContent<MemoryHost, MemContent> {
    GitContent::new(host.inner.clone(), MemContent::new(), committer(), clock())
}

fn refs(host: &HttpHost, id: &ArtifactId) -> BTreeMap<String, ObjectId> {
    wait(host.inner.repo(&repo_name(id.as_str())).unwrap().refs()).unwrap()
}

fn read(c: &impl ContentStore, id: &ArtifactId, n: u32, path: &str) -> Option<Vec<u8>> {
    wait(c.read_file(id, n, path)).unwrap()
}

/// Every kind of remote: with and without atomic pushes, refusing or
/// taking `main` moved back.
const REMOTES: [(bool, bool); 4] = [(true, false), (false, false), (true, true), (false, true)];

fn refused() -> HttpResponse {
    HttpResponse {
        status: 500,
        headers: Vec::new(),
        body: b"refused\n".to_vec(),
    }
}

#[test]
fn the_contract_holds_over_receive_pack() {
    for (atomic, back) in REMOTES {
        let host = HttpHost::new(atomic, back);
        wait(contract::run(&store(&host)));
    }
}

#[test]
fn a_second_writer_of_a_version_is_refused_over_receive_pack() {
    for (atomic, back) in REMOTES {
        let host = HttpHost::new(atomic, back);
        let (store, racer) = (store(&host), Arc::new(racer(&host)));
        let id = ArtifactId::generate();
        wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
        let (rid, r) = (id.clone(), racer.clone());
        host.transport.before_next_push(move || {
            wait(r.write_version(&rid, 2, &write(&[], &[("index.html", b"theirs")]))).unwrap();
            None
        });
        let mine = wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"mine")])));
        assert!(mine.is_err(), "{atomic} {back}: both writers won");
        assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"theirs");
        let now = refs(&host, &id);
        assert_eq!(now[MAIN], now["refs/tags/v2"], "{atomic} {back}");
    }
}

#[test]
fn a_remove_never_discards_a_commit_put_on_top_over_receive_pack() {
    for (atomic, back) in REMOTES {
        let host = HttpHost::new(atomic, back);
        let (store, racer) = (store(&host), racer(&host));
        let id = ArtifactId::generate();
        wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
        wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"two")]))).unwrap();
        let rid = id.clone();
        host.transport.before_next_push(move || {
            wait(racer.write_version(&rid, 3, &write(&["index.html"], &[]))).unwrap();
            None
        });
        assert!(wait(store.remove_version(&id, 2)).is_err());
        let now = refs(&host, &id);
        assert_eq!(now[MAIN], now["refs/tags/v3"], "{atomic} {back}");
        assert!(now.contains_key("refs/tags/v2"), "{atomic} {back}");
    }
}

#[test]
fn without_atomic_a_remove_cut_short_after_main_deletes_the_tag_alone() {
    let host = HttpHost::new(false, true);
    let store = store(&host);
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"two")]))).unwrap();
    let v1 = refs(&host, &id)["refs/tags/v1"];
    // Main's request goes through; the tag's is refused, then the tag
    // alone is taken.
    let transport = host.transport.clone();
    host.transport.before_next_push(move || {
        transport.before_next_push(|| Some(refused()));
        None
    });
    wait(store.remove_version(&id, 2)).unwrap();
    let now = refs(&host, &id);
    assert!(!now.contains_key("refs/tags/v2"));
    assert_eq!(now[MAIN], v1);
}

#[test]
fn without_atomic_a_remove_cut_short_after_main_finishes_on_a_retry() {
    let host = HttpHost::new(false, true);
    let store = store(&host);
    let id = ArtifactId::generate();
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
    wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"two")]))).unwrap();
    let v1 = refs(&host, &id)["refs/tags/v1"];
    // Main's request goes through; every request for the tag is refused.
    let transport = host.transport.clone();
    host.transport.before_next_push(move || {
        let again = transport.clone();
        transport.before_next_push(move || {
            again.before_next_push(|| Some(refused()));
            Some(refused())
        });
        None
    });
    assert!(wait(store.remove_version(&id, 2)).is_err());
    let now = refs(&host, &id);
    assert_eq!(now[MAIN], v1, "main went back");
    assert!(now.contains_key("refs/tags/v2"), "the tag stayed");
    wait(store.remove_version(&id, 2)).unwrap();
    let now = refs(&host, &id);
    assert!(!now.contains_key("refs/tags/v2"));
    assert_eq!(now[MAIN], v1);
    wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"again")]))).unwrap();
    assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"again");
}

#[test]
fn without_atomic_a_write_whose_tag_is_refused_leaves_main_for_the_next_write() {
    for back in [false, true] {
        let host = HttpHost::new(false, back);
        let store = store(&host);
        let id = ArtifactId::generate();
        wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
        let v1 = refs(&host, &id)["refs/tags/v1"];
        let transport = host.transport.clone();
        host.transport.before_next_push(move || {
            transport.before_next_push(|| Some(refused()));
            None
        });
        assert!(wait(store.write_version(&id, 2, &write(&[], &[("index.html", b"two")]))).is_err());
        let now = refs(&host, &id);
        assert!(!now.contains_key("refs/tags/v2"), "back {back}");
        assert_ne!(now[MAIN], v1, "back {back}: main took the commit");
        assert_eq!(read(&store, &id, 2, "index.html"), None);
        wait(store.write_version(&id, 2, &write(&["index.html"], &[]))).unwrap();
        assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"one");
        let now = refs(&host, &id);
        assert_eq!(now[MAIN], now["refs/tags/v2"], "back {back}");
    }
}

#[test]
fn where_main_cannot_move_back_removed_versions_stay_below_the_next_and_are_never_read() {
    let host = HttpHost::new(true, false);
    let store = store(&host);
    let id = ArtifactId::generate();
    wait(store.write_version(
        &id,
        1,
        &write(&[], &[("index.html", b"one"), ("a.js", b"a")]),
    ))
    .unwrap();
    let v1 = refs(&host, &id)["refs/tags/v1"];
    // Two unrecorded attempts at version 2, each removed.
    for body in [b"two?" as &[u8], b"two??"] {
        wait(store.write_version(&id, 2, &write(&[], &[("index.html", body)]))).unwrap();
        let attempt = refs(&host, &id)["refs/tags/v2"];
        wait(store.remove_version(&id, 2)).unwrap();
        let now = refs(&host, &id);
        assert_eq!(now.len(), 2, "main and v1");
        assert_eq!(now[MAIN], attempt, "main kept the attempt");
        assert_eq!(read(&store, &id, 2, "index.html"), None);
    }
    // Version 2 carries from version 1, not from the attempts.
    wait(store.write_version(&id, 2, &write(&["a.js"], &[("index.html", b"two")]))).unwrap();
    assert_eq!(read(&store, &id, 2, "index.html").unwrap(), b"two");
    assert_eq!(read(&store, &id, 2, "a.js").unwrap(), b"a");
    let now = refs(&host, &id);
    assert_eq!(now[MAIN], now["refs/tags/v2"]);
    let repo = host.inner.repo(&repo_name(id.as_str())).unwrap();
    let mut chain = Vec::new();
    let mut at = Some(now[MAIN]);
    while let Some(c) = at {
        chain.push(c);
        at = wait(repo.read_commit_meta(&c))
            .unwrap()
            .unwrap()
            .parents
            .first()
            .copied();
    }
    assert_eq!(chain.len(), 4, "v2, two attempts, v1");
    assert_eq!(chain[3], v1);
    // An older version under a removed newest one is removed too.
    wait(store.remove_version(&id, 2)).unwrap();
    wait(store.remove_version(&id, 1)).unwrap();
    assert_eq!(refs(&host, &id).keys().collect::<Vec<_>>(), [MAIN]);
    wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"new")]))).unwrap();
    assert_eq!(read(&store, &id, 1, "index.html").unwrap(), b"new");
}

#[test]
fn a_first_version_is_removed_and_written_again_on_a_remote_that_keeps_main() {
    for (atomic, back) in REMOTES {
        let host = HttpHost::new(atomic, back);
        let store = store(&host);
        let id = ArtifactId::generate();
        wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"unrecorded")]))).unwrap();
        wait(store.remove_version(&id, 1)).unwrap();
        assert_eq!(read(&store, &id, 1, "index.html"), None);
        assert_eq!(
            refs(&host, &id).keys().collect::<Vec<_>>(),
            [MAIN],
            "{atomic} {back}"
        );
        wait(store.write_version(&id, 1, &write(&[], &[("index.html", b"one")]))).unwrap();
        assert_eq!(read(&store, &id, 1, "index.html").unwrap(), b"one");
        let now = refs(&host, &id);
        assert_eq!(now[MAIN], now["refs/tags/v1"], "{atomic} {back}");
    }
}
