//! The stand-in's `/git/` route with real git and jj: a clone carries the
//! versions the content store wrote, tagged; a read token clones and
//! cannot push; a write token pushes to `main` and its own tags, never to
//! a version tag; no token, a revoked one or another repository's opens
//! nothing.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;
use clax_content_git::GitContent;
use clax_core::ArtifactId;
use clax_core::content::{ContentStore, wait};
use clax_git::versions::repo_name;
use rusqlite::Connection;

use super::{Home, InProc, clock, committer, jj, local_host, write};
use crate::repos::api::{DoHost, DoLedger, DoTokens, RepoService};
use crate::repos::assets::{BucketAssets, MemObjects};
use crate::repos::db::MAX_WRITE_BYTES;
use crate::repos::git_http::{GitHttpRequest, serve_git, serve_options};
use crate::repos::tokens::{self, GitToken, Holder, LocalTokens, Scope};

type Service = RepoService<Connection, LocalTokens<Connection>>;
type Local = InProc<LocalTokens<Connection>>;

async fn route(
    State(service): State<Arc<Service>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let served = serve_git(
        &service.host,
        &GitHttpRequest {
            method: method.as_str(),
            path: uri.path(),
            query: uri.query(),
            authorization: header("authorization"),
            content_encoding: header("content-encoding"),
            body: &body,
        },
        &serve_options(),
        service.now_ms(),
    )
    .await;
    // As RepoDO does: whom the push's token names, against main's commit.
    tokens::note_push(
        &service.host,
        uri.path(),
        header("authorization"),
        &served.pushed,
        service.now_ms(),
    )
    .unwrap();
    let mut b = Response::builder().status(served.response.status);
    for (k, v) in &served.response.headers {
        b = b.header(k, v);
    }
    b.body(axum::body::Body::from(served.response.body))
        .unwrap()
}

/// The stand-in served over HTTP on a free port, with a content store on
/// it through the internal API.
struct Fixture {
    /// Runs the server until the fixture is dropped.
    _rt: tokio::runtime::Runtime,
    base: String,
    transport: InProc<LocalTokens<Connection>>,
    store: GitContent<DoHost<Local>, BucketAssets<MemObjects>>,
}

impl Fixture {
    fn new() -> Fixture {
        let host = local_host();
        let service = Arc::new(RepoService {
            tokens: LocalTokens {
                host: host.clone(),
                public_origin: String::new(),
            },
            host,
            clock: clock(),
        });
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .fallback(route)
            .layer(DefaultBodyLimit::disable())
            .with_state(service.clone());
        rt.spawn(async move { axum::serve(listener, router).await.unwrap() });
        let transport = InProc { service };
        let store = GitContent::new(
            DoHost {
                transport: transport.clone(),
            },
            BucketAssets {
                objects: MemObjects::new(1000),
            },
            committer(),
            clock(),
        );
        Fixture {
            _rt: rt,
            base,
            transport,
            store,
        }
    }

    fn token(&self, id: &ArtifactId, scope: Scope) -> GitToken {
        self.token_for(id, scope, &Default::default())
    }

    fn token_for(&self, id: &ArtifactId, scope: Scope, holder: &Holder) -> GitToken {
        wait(
            DoTokens {
                transport: self.transport.clone(),
            }
            .mint(
                &repo_name(id.as_str()),
                scope,
                3600,
                Some(&self.base),
                holder,
            ),
        )
        .unwrap()
    }

    fn revoke(&self, id: &ArtifactId, token: &GitToken) {
        assert!(
            wait(
                DoTokens {
                    transport: self.transport.clone(),
                }
                .revoke(&repo_name(id.as_str()), &token.id)
            )
            .unwrap()
        );
    }
}

fn bearer(t: &GitToken) -> String {
    format!("http.extraHeader=Authorization: Bearer {}", t.token)
}

/// Two versions of a new artifact, through the content store.
fn two_versions(f: &Fixture) -> ArtifactId {
    let id = ArtifactId::generate();
    wait(f.store.write_version(
        &id,
        1,
        &write(
            &[],
            &[("index.html", b"<h1>one</h1>\n"), ("app.js", b"go()\n")],
        ),
    ))
    .unwrap();
    wait(f.store.write_version(
        &id,
        2,
        &write(&["app.js"], &[("index.html", b"<h1>two</h1>\n")]),
    ))
    .unwrap();
    id
}

#[test]
fn git_clones_the_versions_and_pushes_with_a_write_token_but_never_a_version_tag() {
    let f = Fixture::new();
    let id = two_versions(&f);
    let read = f.token(&id, Scope::Read);
    assert_eq!(
        read.remote,
        format!("{}/git/{}.git", f.base, repo_name(id.as_str()))
    );
    let home = Home::new();
    let work = home.dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let out = work.join("out");
    let out_s = out.to_str().unwrap();

    // Without a token, or with a guessed one: refused.
    let no = home.run("git", &work, &["clone", "-q", &read.remote, out_s], &[]);
    assert!(!no.status.success());
    let guessed = home.run(
        "git",
        &work,
        &[
            "-c",
            "http.extraHeader=Authorization: Bearer cxm_read_guess",
            "clone",
            "-q",
            &read.remote,
            out_s,
        ],
        &[],
    );
    assert!(!guessed.status.success());
    // git asks for credentials after the 401, and may not prompt for them.
    let why = String::from_utf8_lossy(&guessed.stderr);
    assert!(
        why.contains("401") || why.contains("Username"),
        "{guessed:?}"
    );

    home.ok(
        "git",
        &work,
        &["-c", &bearer(&read), "clone", "-q", &read.remote, out_s],
        &[],
    );
    let log = home.ok("git", &out, &["log", "--format=%s|%an <%ae>"], &[]);
    assert_eq!(
        log,
        "Version 2|Clax <clax@users.clax.dev>\nVersion 1|Clax <clax@users.clax.dev>"
    );
    assert_eq!(home.ok("git", &out, &["tag"], &[]), "v1\nv2");
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "<h1>two</h1>\n"
    );
    assert_eq!(
        home.ok("git", &out, &["show", "v1:index.html"], &[]),
        "<h1>one</h1>"
    );

    // A read token does not push.
    let id_args = [
        "-c",
        "user.name=Pusher",
        "-c",
        "user.email=pusher@clax.test",
    ];
    std::fs::write(out.join("pushed.txt"), "pushed\n").unwrap();
    home.ok("git", &out, &["add", "pushed.txt"], &[]);
    home.ok(
        "git",
        &out,
        &[
            id_args[0], id_args[1], id_args[2], id_args[3], "commit", "-q", "-m", "Pushed",
        ],
        &[],
    );
    let refused = home.run(
        "git",
        &out,
        &["-c", &bearer(&read), "push", "-q", "origin", "main"],
        &[],
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("403"),
        "{refused:?}"
    );

    // A write token pushes main and the pusher's own tag, not a version tag.
    let w = f.token(&id, Scope::Write);
    home.ok(
        "git",
        &out,
        &["-c", &bearer(&w), "push", "-q", "origin", "main"],
        &[],
    );
    home.ok("git", &out, &["tag", "v3"], &[]);
    home.ok("git", &out, &["tag", "vendor"], &[]);
    let v3 = home.run(
        "git",
        &out,
        &["-c", &bearer(&w), "push", "origin", "v3"],
        &[],
    );
    assert!(!v3.status.success(), "a version tag was pushed");
    assert!(
        String::from_utf8_lossy(&v3.stderr).contains("version tags"),
        "{v3:?}"
    );
    home.ok(
        "git",
        &out,
        &["-c", &bearer(&w), "push", "-q", "origin", "vendor"],
        &[],
    );
    let refs = wait(async {
        let repo = clax_git::RepoHost::open(
            &DoHost {
                transport: f.transport.clone(),
            },
            &repo_name(id.as_str()),
        )
        .await
        .unwrap()
        .unwrap();
        clax_git::Repo::refs(&repo).await.unwrap()
    });
    let names: Vec<&str> = refs.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "refs/heads/main",
            "refs/tags/v1",
            "refs/tags/v2",
            "refs/tags/vendor"
        ]
    );

    // A revoked token opens nothing; nor does another repository's.
    f.revoke(&id, &w);
    let after = home.run(
        "git",
        &out,
        &["-c", &bearer(&w), "fetch", "-q", "origin"],
        &[],
    );
    assert!(!after.status.success());
    let other = two_versions(&f);
    let elsewhere = format!("{}/git/{}.git", f.base, repo_name(other.as_str()));
    let cross = home.run(
        "git",
        &work,
        &[
            "-c",
            &bearer(&read),
            "clone",
            "-q",
            &elsewhere,
            work.join("other").to_str().unwrap(),
        ],
        &[],
    );
    assert!(!cross.status.success());
}

#[test]
fn jj_clones_with_the_header_in_the_git_configuration_environment() {
    let Some(jj) = jj() else {
        eprintln!("jj not found (set CLAX_TEST_JJ): not run");
        return;
    };
    let f = Fixture::new();
    let id = two_versions(&f);
    let t = f.token(&id, Scope::Read);
    let home = Home::new();
    let header = format!("Authorization: Bearer {}", t.token);
    let env = [
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "http.extraHeader"),
        ("GIT_CONFIG_VALUE_0", header.as_str()),
    ];
    let out = home.dir.path().join("jj-out");
    home.ok(
        &jj,
        home.dir.path(),
        &["git", "clone", &t.remote, out.to_str().unwrap()],
        &env,
    );
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "<h1>two</h1>\n"
    );
    let log = home.ok(
        &jj,
        &out,
        &[
            "log",
            "--no-graph",
            "-r",
            "::main@origin",
            "-T",
            "description.first_line() ++ \"\\n\"",
        ],
        &env,
    );
    assert_eq!(
        log.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        ["Version 2", "Version 1"]
    );
}

/// Bytes no compressor shrinks.
fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

#[test]
fn a_push_over_the_stand_ins_write_bound_is_refused_whole() {
    let f = Fixture::new();
    let id = two_versions(&f);
    let write = f.token(&id, Scope::Write);
    let home = Home::new();
    let out = home.dir.path().join("out");
    let out_s = out.to_str().unwrap();
    home.ok(
        "git",
        home.dir.path(),
        &["-c", &bearer(&write), "clone", "-q", &write.remote, out_s],
        &[],
    );
    let heads = |home: &Home| {
        home.ok(
            "git",
            &out,
            &["-c", &bearer(&write), "ls-remote", "origin"],
            &[],
        )
    };
    let before = heads(&home);
    let ident = ["-c", "user.name=P", "-c", "user.email=p@clax.test"];
    let commit = |name: &str, bytes: &[u8]| {
        std::fs::write(out.join(name), bytes).unwrap();
        home.ok("git", &out, &["add", name], &[]);
        home.ok(
            "git",
            &out,
            &[
                ident[0], ident[1], ident[2], ident[3], "commit", "-q", "-m", name,
            ],
            &[],
        );
    };
    let push = |home: &Home| {
        home.run(
            "git",
            &out,
            &["-c", &bearer(&write), "push", "origin", "main"],
            &[],
        )
    };

    // A pack that inflates past the bound: refused while unpacking.
    commit("zeros.bin", &vec![0u8; MAX_WRITE_BYTES + 1]);
    let r = push(&home);
    assert!(!r.status.success(), "{r:?}");
    let why = String::from_utf8_lossy(&r.stderr);
    assert!(why.contains(&MAX_WRITE_BYTES.to_string()), "{why}");
    assert_eq!(heads(&home), before);

    // A pack over the bound as sent: 413 before it is unpacked.
    home.ok("git", &out, &["reset", "-q", "--hard", "HEAD~1"], &[]);
    commit("noise.bin", &noise(MAX_WRITE_BYTES + (1 << 20)));
    let r = push(&home);
    assert!(!r.status.success(), "{r:?}");
    let why = String::from_utf8_lossy(&r.stderr);
    assert!(why.contains("413"), "{why}");
    assert_eq!(heads(&home), before);
}

/// A connected person named `name` whose key is `[key; 32]`, who may
/// publish `id`: their public ID and grant.
fn person(st: &clax_store::Store, name: &str, key: u8, id: &ArtifactId) -> (String, String) {
    use clax_core::rights::{PRESET_COLLABORATOR, Rights};
    use clax_store::people::{GrantStart, InvitePerson, NewGrant};
    let daemon = &clax_core::audit::AuditCtx::DAEMON;
    let (_, code) = st
        .create_invite(daemon, InvitePerson::New(name.into()), None)
        .unwrap();
    let start = st
        .request_grant(
            daemon,
            NewGrant {
                key: [key; 32],
                client_name: "laptop".into(),
                peer_ip: "192.168.1.20".into(),
                access: Rights::preset(PRESET_COLLABORATOR).unwrap(),
                invite: Some(clax_core::Secret::new(code.expose().to_string())),
            },
        )
        .unwrap();
    let GrantStart::Approved { grant, .. } = start else {
        panic!("{start:?}")
    };
    let pid = grant.person.unwrap();
    let rights = Rights::parse(&serde_json::json!([
        PRESET_COLLABORATOR,
        {"type": "clax-artifacts", "actions": ["read", "update"], "locations": [id.as_str()]}
    ]))
    .unwrap();
    st.set_person_rights(daemon, &pid, Some(rights)).unwrap();
    (pid, grant.id)
}

#[test]
fn a_push_is_its_token_holders_whichever_reconcile_records_it_and_blind_agents_lose_their_tokens() {
    let f = Fixture::new();
    let daemon = &clax_core::audit::AuditCtx::DAEMON;
    let dir = tempfile::tempdir().unwrap();
    let content = Arc::new(
        GitContent::new(
            DoHost {
                transport: f.transport.clone(),
            },
            BucketAssets {
                objects: MemObjects::new(1000),
            },
            committer(),
            clock(),
        )
        .with_ledger(Arc::new(DoLedger {
            transport: f.transport.clone(),
        })),
    );
    let st = clax_store::Store::open(&clax_core::Home::at(dir.path().join("clax")))
        .unwrap()
        .with_content(content);
    let publish = |html: &str, v: Option<u32>| {
        clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "if_version": v, "files": {"index.html": {"content": html}}
            }))
            .unwrap(),
        )
        .unwrap()
    };
    let (a, _) = wait(st.create_artifact_async(daemon, publish("<p>one</p>", None), None)).unwrap();
    let id = ArtifactId::parse(&a.id).unwrap();
    let (rae, _) = person(&st, "Rae", 7, &id);
    let (sam, sam_grant) = person(&st, "Sam", 8, &id);
    let sam_agent = st
        .register_session(
            daemon,
            clax_core::RegisterSession {
                harness: "claude".into(),
                cwd: "/w".into(),
                grant_id: Some(sam_grant),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let rae_write = f.token_for(
        &id,
        Scope::Write,
        &Holder {
            person: Some(rae.clone()),
            session: None,
        },
    );
    let sam_read = f.token_for(
        &id,
        Scope::Read,
        &Holder {
            person: Some(sam.clone()),
            session: Some(sam_agent.clone()),
        },
    );

    let home = Home::new();
    let work = home.dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let out = work.join("out");
    let out_s = out.to_str().unwrap();
    home.ok(
        "git",
        &work,
        &[
            "-c",
            &bearer(&rae_write),
            "clone",
            "-q",
            &rae_write.remote,
            out_s,
        ],
        &[],
    );
    std::fs::write(out.join("index.html"), "<p>rae</p>").unwrap();
    home.ok("git", &out, &["add", "-A"], &[]);
    home.ok(
        "git",
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@clax.test",
            "commit",
            "-qm",
            "Rae's take",
        ],
        &[],
    );
    home.ok(
        "git",
        &out,
        &["-c", &bearer(&rae_write), "push", "-q", "origin", "main"],
        &[],
    );

    let before = work.join("before");
    home.ok(
        "git",
        &work,
        &[
            "-c",
            &bearer(&sam_read),
            "clone",
            "-q",
            &sam_read.remote,
            before.to_str().unwrap(),
        ],
        &[],
    );

    // No reconcile ran for the push: the publish's records it as Rae's.
    let stale = wait(st.publish_version_async(daemon, &id, publish("<p>mine</p>", Some(1)), None));
    assert!(
        matches!(stale, Err(clax_core::CoreError::Conflict { current: 2 })),
        "{stale:?}"
    );
    let v2 = st.get_version(&id, 2).unwrap().unwrap();
    assert_eq!(v2.authors.person.as_deref(), Some(rae.as_str()));
    assert_eq!(v2.note.as_deref(), Some("Rae's take"));

    // Sam has not let Rae's work reach Sam's agents: their token went
    // before version 2 was recorded; Rae's own stays.
    let again = work.join("again");
    let refused = home.run(
        "git",
        &work,
        &[
            "-c",
            &bearer(&sam_read),
            "clone",
            "-q",
            &sam_read.remote,
            again.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        !refused.status.success(),
        "Sam's agent's token still clones"
    );
    home.ok(
        "git",
        &out,
        &["-c", &bearer(&rae_write), "fetch", "-q", "origin"],
        &[],
    );
}
