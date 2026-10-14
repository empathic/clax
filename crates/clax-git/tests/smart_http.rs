//! The crate's smart-HTTP server over a `MemoryHost`, driven by the real
//! `git` and `jj` binaries through an axum adapter: clone with a Bearer
//! header (main, HEAD and the `v<n>` tags; `fsck --strict`); fetches that
//! negotiate a common commit and carry only the new objects, also over
//! several gzip-encoded rounds and through `jj git fetch`; a pack spanning
//! several side-band packets; pushes (thin and deltified) that move refs and
//! are reported, and the pushes refused; 401 and 403; the upload-pack
//! negotiation lines; and the request limits.

use std::path::Path;
use std::sync::{Arc, Mutex};

use clax_git::memory::MemoryHost;
use clax_git::pktline::{self, FLUSH, Packet};
use clax_git::server::{
    Access, GitRequest, MAX_LINES, ServeOptions, parse_upload_request, serve, upload_pack,
};
use clax_git::tree::Changes;
use clax_git::{GitVersions, MAIN, ObjectId, Person, Publish, Repo, Rev, VersionMeta};

use crate::common::{Home, Server, Stats, bearer_args, bearer_env, clax_server, jj, temp};

const ARTIFACT: &str = "0123456789ab";
const REPO: &str = "a-0123456789ab";
const READ: &str = "cxm_read_node-test-token_0123456789";
const WRITE: &str = "cxm_write_node-test-token_0123456789";
const NO_COMMON: &str = "no common commits";

struct Fixture {
    home: Home,
    host: MemoryHost,
    versions: GitVersions<MemoryHost>,
    server: Server,
    stats: Arc<Mutex<Stats>>,
    next: Mutex<u64>,
}

fn changes(files: &[(&str, &[u8])]) -> Changes {
    files
        .iter()
        .map(|(p, c)| (p.to_string(), Some(c.to_vec())))
        .collect()
}

impl Fixture {
    async fn new() -> Fixture {
        Fixture::with_options(ServeOptions::default()).await
    }

    async fn with_options(options: ServeOptions) -> Fixture {
        let host = MemoryHost::new();
        let versions = GitVersions::new(host.clone());
        versions.create(ARTIFACT).await.unwrap();
        let (server, stats) = clax_server(
            &host,
            &[(READ, Access::Read), (WRITE, Access::Write)],
            options,
        )
        .await;
        let f = Fixture {
            home: Home::new(),
            host,
            versions,
            server,
            stats,
            next: Mutex::new(1),
        };
        f.publish(
            &[
                ("index.html", b"<h1>v1</h1>\n"),
                ("css/site.css", b"h1 { color: teal }\n"),
            ],
            "First version",
        )
        .await;
        f.publish(
            &[("index.html", b"<h1>v2</h1>\n"), ("notes.md", b"notes\n")],
            "Second version",
        )
        .await;
        f
    }

    fn remote(&self) -> String {
        format!("{}/git/{REPO}.git", self.server.base)
    }

    async fn head(&self) -> ObjectId {
        self.host.repo(REPO).unwrap().refs().await.unwrap()[MAIN]
    }

    /// Publishes the next version with `files` changed; answers its commit.
    async fn publish(&self, files: &[(&str, &[u8])], note: &str) -> ObjectId {
        let n = {
            let mut next = self.next.lock().unwrap();
            let n = *next;
            *next += 1;
            n
        };
        let parent = self.versions.head(ARTIFACT).await.unwrap();
        let meta = VersionMeta {
            author: Person {
                name: "Rae".into(),
                email: "rae@users.clax.dev".into(),
            },
            time: 1_760_176_800 + n as i64,
            note: note.into(),
            publisher: "0123456789ab".into(),
            agent: None,
        };
        let p = Publish {
            version: n,
            parent,
            changes: changes(files),
            meta,
        };
        self.versions.publish(ARTIFACT, &p).await.unwrap().commit
    }

    /// Clones with `token` into a new directory; answers it.
    async fn clone(&self, token: &str, work: &Path) -> std::path::PathBuf {
        let [c, h] = bearer_args(token);
        let out = work.join("out");
        self.home
            .ok_async(
                "git",
                work,
                &[&c, &h, "clone", "-q", &self.remote(), out.to_str().unwrap()],
                &[],
            )
            .await;
        out
    }

    fn last_pack(&self) -> u32 {
        *self
            .stats
            .lock()
            .unwrap()
            .packs
            .last()
            .expect("a pack served")
    }

    fn git_env(&self, token: &str) -> Vec<(String, String)> {
        bearer_env(token)
    }

    async fn git_async(&self, cwd: &Path, token: &str, args: &[&str]) -> std::process::Output {
        let env = self.git_env(token);
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.home.run_async("git", cwd, args, &env).await
    }
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn clones_main_head_and_the_version_tags_and_passes_fsck() {
    let f = Fixture::new().await;
    let work = temp("clone");
    let out = f.clone(READ, work.path()).await;
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "<h1>v2</h1>\n"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("css/site.css")).unwrap(),
        "h1 { color: teal }\n"
    );
    assert_eq!(
        f.home.git_text(&out, &["rev-parse", "HEAD"]),
        f.head().await.to_hex()
    );
    assert_eq!(
        f.home
            .git_text(&out, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/main"
    );
    assert_eq!(f.home.git_text(&out, &["tag", "--list"]), "v1\nv2");
    assert_eq!(
        f.home.git_text(&out, &["show", "v1:index.html"]),
        "<h1>v1</h1>"
    );
    assert_eq!(
        f.home
            .git_text(&out, &["log", "-1", "--format=%an <%ae>%n%B", "v1"]),
        "Rae <rae@users.clax.dev>\nFirst version\n\nClax-Version: 1\nClax-Publisher: 0123456789ab"
    );
    f.home.git(&out, &["fsck", "--strict", "--no-dangling"]);
    // The token is not kept in the clone's configuration.
    assert!(
        !std::fs::read_to_string(out.join(".git/config"))
            .unwrap()
            .contains(READ)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fetches_a_new_commit_through_a_common_commit_with_only_the_new_objects() {
    let f = Fixture::new().await;
    let work = temp("fetch");
    let out = f.clone(READ, work.path()).await;
    let c3 = f
        .publish(&[("notes.md", b"more notes\n")], "Third version")
        .await;
    let o = f
        .git_async(&out, READ, &["fetch", "--tags", "origin"])
        .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stderr(&o).contains(NO_COMMON), "{}", stderr(&o));
    assert_eq!(
        f.home.git_text(&out, &["rev-parse", "origin/main", "v3"]),
        format!("{c3}\n{c3}")
    );
    // The new commit, its root tree and the changed file.
    assert_eq!(f.last_pack(), 3);
    f.home
        .git(&out, &["merge", "-q", "--ff-only", "origin/main"]);
    assert_eq!(
        std::fs::read_to_string(out.join("notes.md")).unwrap(),
        "more notes\n"
    );
    f.home.git(&out, &["fsck", "--strict", "--no-dangling"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_a_pack_larger_than_one_side_band_packet() {
    let f = Fixture::new().await;
    let mut big = vec![0u8; 300_000];
    let mut x: u32 = 7;
    for b in &mut big {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        *b = (x >> 16) as u8;
    }
    f.publish(&[("big.bin", &big)], "A big file").await;
    let work = temp("big");
    let out = f.clone(READ, work.path()).await;
    assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), big);
    f.home.git(&out, &["fsck", "--strict", "--no-dangling"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn negotiates_over_several_gzip_rounds_when_the_client_holds_commits_the_server_lacks() {
    let f = Fixture::new().await;
    let work = temp("rounds");
    let out = f.clone(READ, work.path()).await;
    // 60 local commits the server lacks: git offers them in rounds of 16, 16,
    // then the rest with the common commit, a request over git's 1 KiB gzip
    // threshold (stateless rounds resend the wants and the common haves).
    let base = f.home.git_text(&out, &["rev-parse", "origin/main"]);
    let at = 1_900_000_000;
    let mut stream = String::new();
    for i in 0..60 {
        let msg = format!("local {i}\n");
        let file = format!("{i}\n");
        stream.push_str(&format!(
            "commit refs/heads/local\ncommitter Rae <rae@example.com> {} +0000\ndata {}\n{msg}",
            at + i,
            msg.len()
        ));
        if i == 0 {
            stream.push_str(&format!("from {base}\n"));
        }
        stream.push_str(&format!(
            "M 644 inline local.txt\ndata {}\n{file}\n",
            file.len()
        ));
    }
    f.home.git_with(
        &out,
        &["fast-import", "--quiet"],
        &[],
        Some(stream.as_bytes()),
    );
    let last = f.publish(&[("count.txt", b"last\n")], "Last").await;
    let before = f.stats.lock().unwrap().gzip_requests;
    let o = f.git_async(&out, READ, &["fetch", "origin"]).await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stderr(&o).contains(NO_COMMON), "{}", stderr(&o));
    assert!(f.stats.lock().unwrap().gzip_requests > before);
    assert_eq!(
        f.home.git_text(&out, &["rev-parse", "origin/main"]),
        last.to_hex()
    );
    assert_eq!(f.last_pack(), 3);
    f.home.git(&out, &["fsck", "--strict", "--no-dangling"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_another_token_with_401_and_a_push_with_a_read_token_with_403() {
    let f = Fixture::new().await;
    let work = temp("refused");
    let [c, h] = bearer_args("cxm_read_wrong");
    let o = f
        .home
        .run_async(
            "git",
            work.path(),
            &[&c, &h, "clone", &f.remote(), "out"],
            &[],
        )
        .await;
    assert!(!o.status.success());
    let client = reqwest::Client::new();
    let url = format!("{}/info/refs?service=git-upload-pack", f.remote());
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
    let other = url.replace(REPO, "a-zzzzzzzzzzzz");
    assert_eq!(
        client
            .get(&other)
            .bearer_auth(READ)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // The scheme is case-insensitive.
    assert_eq!(
        client
            .get(&url)
            .header("authorization", format!("bearer {READ}"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let out = f.clone(READ, work.path()).await;
    std::fs::write(out.join("index.html"), "<h1>pushed</h1>\n").unwrap();
    f.home.git(
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "commit",
            "-qam",
            "push me",
        ],
    );
    let o = f
        .git_async(&out, READ, &["push", "origin", "HEAD:main"])
        .await;
    assert!(!o.status.success());
    assert!(stderr(&o).contains("403"), "{}", stderr(&o));
    let post = client
        .post(format!("{}/git-receive-pack", f.remote()))
        .bearer_auth(READ)
        .body(FLUSH.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn takes_pushes_with_a_write_token_thin_and_deltified_and_reports_them() {
    let f = Fixture::new().await;
    let work = temp("push");
    let out = f.clone(WRITE, work.path()).await;
    let big: String = (0..4000)
        .map(|i| format!("line {i} of a long page\n"))
        .collect();
    std::fs::write(out.join("page.html"), &big).unwrap();
    f.home.git(&out, &["add", "-A"]);
    f.home.git(
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "commit",
            "-qm",
            "Edit from git",
        ],
    );
    let o = f
        .git_async(&out, WRITE, &["push", "-q", "origin", "HEAD:main"])
        .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let first = f.home.git_text(&out, &["rev-parse", "HEAD"]);
    assert_eq!(f.head().await.to_hex(), first);

    // A one-line change: git sends a thin pack, the new page a delta on the old.
    std::fs::write(out.join("page.html"), format!("{big}one more line\n")).unwrap();
    f.home.git(
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "commit",
            "-qam",
            "Second edit",
        ],
    );
    f.home.git(
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "tag",
            "-a",
            "-m",
            "annotated",
            "release",
        ],
    );
    let o = f
        .git_async(
            &out,
            WRITE,
            &["push", "-q", "origin", "HEAD:main", "release"],
        )
        .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let second = f.home.git_text(&out, &["rev-parse", "HEAD"]);
    assert_eq!(f.head().await.to_hex(), second);
    let pushes = f.stats.lock().unwrap().pushes.clone();
    assert_eq!(pushes.len(), 2);
    assert_eq!(
        pushes[1]
            .iter()
            .map(|u| u.name.as_str())
            .collect::<Vec<_>>(),
        ["refs/heads/main", "refs/tags/release"]
    );

    // The pushed commits read back as the artifact's history, and clone back.
    let log = f.versions.log(ARTIFACT, None, Some(2)).await.unwrap();
    assert_eq!(log[0].commit.to_hex(), second);
    assert_eq!(log[0].message, "Second edit");
    assert_eq!(
        f.versions
            .read_file(ARTIFACT, &Rev::Commit(log[1].commit), "page.html")
            .await
            .unwrap(),
        Some(big.into_bytes())
    );
    let again = temp("again");
    let back = f.clone(READ, again.path()).await;
    f.home.git(&back, &["fsck", "--strict", "--no-dangling"]);
    assert_eq!(
        f.home.git_text(&back, &["rev-parse", "release^{commit}"]),
        second
    );
    assert_eq!(
        f.home.git_text(&back, &["cat-file", "-t", "release"]),
        "tag"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_non_fast_forward_push_and_deleting_main() {
    let f = Fixture::new().await;
    let work = temp("nff");
    let out = f.clone(WRITE, work.path()).await;
    let head = f.head().await;
    f.home.git(&out, &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(out.join("side.txt"), "side\n").unwrap();
    f.home.git(&out, &["add", "-A"]);
    f.home.git(
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "commit",
            "-qm",
            "side",
        ],
    );
    let o = f
        .git_async(&out, WRITE, &["push", "--force", "origin", "HEAD:main"])
        .await;
    assert!(!o.status.success());
    assert!(stderr(&o).contains("non-fast-forward"), "{}", stderr(&o));
    let o = f.git_async(&out, WRITE, &["push", "origin", ":main"]).await;
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("deletion of the current branch prohibited"),
        "{}",
        stderr(&o)
    );
    assert_eq!(f.head().await, head);
    // Another branch, and deleting it, are fine.
    let o = f
        .git_async(&out, WRITE, &["push", "origin", "HEAD:refs/heads/side"])
        .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let o = f.git_async(&out, WRITE, &["push", "origin", ":side"]).await;
    assert!(o.status.success(), "{}", stderr(&o));
    let refs = f.host.repo(REPO).unwrap().refs().await.unwrap();
    assert!(!refs.contains_key("refs/heads/side"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jj_clones_fetches_and_pushes_through_the_git_configuration_environment() {
    let Some(jj) = jj() else {
        eprintln!("jj not found (set CLAX_TEST_JJ): skipping");
        return;
    };
    let f = Fixture::new().await;
    let work = temp("jj");
    let env = bearer_env(WRITE);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    f.home
        .ok_async(
            &jj,
            work.path(),
            &["git", "clone", &f.remote(), "out"],
            &env,
        )
        .await;
    let out = work.path().join("out");
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "<h1>v2</h1>\n"
    );
    // jj keeps its git store colocated (`.git`) or inside `.jj`, by version and configuration.
    let store = if out.join(".git").exists() {
        out.join(".git")
    } else {
        out.join(".jj/repo/store/git")
    };
    let store = store.to_str().unwrap();
    let v1 = f
        .versions
        .commit_of(ARTIFACT, &Rev::Version(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        f.home
            .git_text(&out, &["--git-dir", store, "rev-parse", "refs/tags/v1"]),
        v1.to_hex()
    );

    let next = f.publish(&[("jj.txt", b"for jj\n")], "For jj").await;
    let err = f.home.ok_async(&jj, &out, &["git", "fetch"], &env).await;
    assert!(!err.contains(NO_COMMON), "{err}");
    assert_eq!(f.last_pack(), 3);
    assert_eq!(
        f.home.git_text(
            &out,
            &["--git-dir", store, "rev-parse", "refs/remotes/origin/main"]
        ),
        next.to_hex()
    );
    f.home.git(
        &out,
        &["--git-dir", store, "fsck", "--strict", "--no-dangling"],
    );

    f.home
        .ok_async(&jj, &out, &["new", "main@origin"], &env)
        .await;
    std::fs::write(out.join("jj.txt"), "from jj\n").unwrap();
    f.home
        .ok_async(&jj, &out, &["commit", "-m", "Edit from jj"], &env)
        .await;
    f.home
        .ok_async(&jj, &out, &["bookmark", "set", "main", "-r", "@-"], &env)
        .await;
    f.home
        .ok_async(&jj, &out, &["git", "push", "--bookmark", "main"], &env)
        .await;
    let log = f.versions.log(ARTIFACT, None, Some(1)).await.unwrap();
    assert_eq!(log[0].message, "Edit from jj");
    assert_eq!(log[0].parents, [next]);
}

// ── upload-pack requests ────────────────────────────────────────────────────

fn body(lines: &[Option<&str>]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        match l {
            Some(l) => pktline::write(&mut out, l.as_bytes()),
            None => out.extend_from_slice(FLUSH),
        }
    }
    out
}

fn text(b: &[u8]) -> Vec<Option<String>> {
    pktline::read_all(b)
        .unwrap()
        .into_iter()
        .map(|p| match p {
            Packet::Data(d) => Some(String::from_utf8_lossy(d).into_owned()),
            _ => None,
        })
        .collect()
}

async fn memory_repo() -> (MemoryHost, ObjectId, ObjectId) {
    let host = MemoryHost::new();
    let v = GitVersions::new(host.clone());
    v.create(ARTIFACT).await.unwrap();
    let meta = |n: i64| VersionMeta {
        author: Person {
            name: "Rae".into(),
            email: "rae@x".into(),
        },
        time: n,
        note: String::new(),
        publisher: "p".into(),
        agent: None,
    };
    let c1 = v
        .publish(
            ARTIFACT,
            &Publish {
                version: 1,
                parent: None,
                changes: changes(&[("a", b"1")]),
                meta: meta(1),
            },
        )
        .await
        .unwrap()
        .commit;
    let c2 = v
        .publish(
            ARTIFACT,
            &Publish {
                version: 2,
                parent: Some(c1),
                changes: changes(&[("a", b"2")]),
                meta: meta(2),
            },
        )
        .await
        .unwrap()
        .commit;
    (host, c1, c2)
}

#[tokio::test]
async fn answers_ack_common_nak_ready_and_a_final_ack() {
    let (host, parent, head) = memory_repo().await;
    let repo = host.repo(REPO).unwrap();
    let unknown = "1".repeat(40);
    let want = format!("want {head} multi_ack_detailed side-band-64k\n");
    let have_unknown = format!("have {unknown}\n");
    let have_parent = format!("have {parent}\n");
    let s = |v: &str| Some(v.to_string());
    let r = upload_pack(
        &repo,
        &body(&[Some(&want), None, Some(&have_unknown), None]),
    )
    .await;
    assert_eq!(text(&r), [s("NAK\n")]);
    let r = upload_pack(&repo, &body(&[Some(&want), None, Some(&have_parent), None])).await;
    assert_eq!(
        text(&r),
        [
            s(&format!("ACK {parent} common\n")),
            s(&format!("ACK {parent} ready\n")),
            s("NAK\n")
        ]
    );
    // Once ready, a have the server lacks is acknowledged ready.
    let r = upload_pack(
        &repo,
        &body(&[
            Some(&want),
            None,
            Some(&have_parent),
            Some(&have_unknown),
            None,
        ]),
    )
    .await;
    assert_eq!(
        text(&r),
        [
            s(&format!("ACK {parent} common\n")),
            s(&format!("ACK {unknown} ready\n")),
            s("NAK\n")
        ]
    );
    let r = upload_pack(
        &repo,
        &body(&[Some(&want), None, Some(&have_parent), Some("done\n")]),
    )
    .await;
    let lines = text(&r);
    assert_eq!(
        lines[..2],
        [
            s(&format!("ACK {parent} common\n")),
            s(&format!("ACK {parent}\n"))
        ]
    );
    // Then the pack on band 1, without the parent's objects: commit, tree, blob.
    let pack_line = lines[2].clone().unwrap();
    assert!(pack_line.starts_with("\u{1}PACK"), "{pack_line:?}");
    let raw = &r[r.windows(4).position(|w| w == b"PACK").unwrap()..];
    assert_eq!(u32::from_be_bytes(raw[8..12].try_into().unwrap()), 3);
    // Without multi_ack_detailed: NAK only.
    let plain = format!("want {head}\n");
    let r = upload_pack(
        &repo,
        &body(&[Some(&plain), None, Some(&have_parent), None]),
    )
    .await;
    assert_eq!(text(&r), [s("NAK\n")]);
}

#[tokio::test]
async fn serves_a_want_a_ref_reaches_and_answers_err_for_a_deepen_and_too_many_lines() {
    let (host, parent, head) = memory_repo().await;
    let repo = host.repo(REPO).unwrap();
    // v1 untagged: its commit is no ref tip, but main reaches it. (A want
    // no ref reaches is refused: tests/hostile.rs.)
    let untag = [("v1".to_string(), None)].into_iter().collect();
    GitVersions::new(host.clone())
        .set_tags(ARTIFACT, &untag)
        .await
        .unwrap();
    let r = upload_pack(
        &repo,
        &body(&[Some(&format!("want {parent}\n")), None, Some("done\n")]),
    )
    .await;
    assert!(
        r.starts_with(b"0008NAK\nPACK"),
        "{:?}",
        String::from_utf8_lossy(&r)
    );
    let r = upload_pack(
        &repo,
        &body(&[Some(&format!("want {head}\n")), Some("deepen 1\n"), None]),
    )
    .await;
    assert!(
        text(&r)[0]
            .as_deref()
            .unwrap()
            .starts_with("ERR deepen is not supported")
    );
    let have = format!("have {}\n", "1".repeat(40));
    let many: Vec<Option<&str>> = (0..=MAX_LINES).map(|_| Some(have.as_str())).collect();
    assert!(
        parse_upload_request(&body(&many))
            .unwrap_err()
            .contains("more than 65536")
    );
    // A blob gone from the store.
    let broken = NoBlobs(repo);
    let r = upload_pack(
        &broken,
        &body(&[Some(&format!("want {head}\n")), None, Some("done\n")]),
    )
    .await;
    let first = text(&r)[0].clone().unwrap();
    assert!(
        first.starts_with("ERR upload-pack: blob ") && first.ends_with(" is missing\n"),
        "{first}"
    );
}

/// A repository that has lost its blobs.
struct NoBlobs(clax_git::memory::MemoryRepo);

impl Repo for NoBlobs {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<clax_git::Object>> {
        Ok(self
            .0
            .read_object(id)
            .await?
            .filter(|o| o.kind != clax_git::ObjectKind::Blob))
    }
    async fn refs(&self) -> clax_git::Result<std::collections::BTreeMap<String, ObjectId>> {
        self.0.refs().await
    }
    async fn apply(
        &self,
        objects: Vec<clax_git::Object>,
        updates: &[clax_git::RefUpdate],
        atomic: bool,
    ) -> clax_git::Result<Vec<clax_git::RefStatus>> {
        self.0.apply(objects, updates, atomic).await
    }
}

#[tokio::test]
async fn answers_413_past_the_request_limits_and_inflates_gzip() {
    let (host, _, head) = memory_repo().await;
    let repo = host.repo(REPO).unwrap();
    let options = ServeOptions::default();
    let post = |body: Vec<u8>, gzip: bool| {
        let repo = repo.clone();
        let options = options.clone();
        async move {
            let request = GitRequest {
                method: "POST",
                rest: "git-upload-pack",
                query: None,
                content_encoding: gzip.then_some("gzip"),
                body: &body,
            };
            serve(&repo, Access::Read, &request, &options)
                .await
                .response
                .status
        }
    };
    assert_eq!(post(vec![b'0'; 1024 * 1024 + 1], false).await, 413);
    let bomb = gzip(&vec![0u8; 8 * 1024 * 1024 + 1]);
    assert!(bomb.len() < 1024 * 1024);
    assert_eq!(post(bomb, true).await, 413);
    let fine = gzip(&body(&[
        Some(&format!("want {head}\n")),
        None,
        Some("done\n"),
    ]));
    assert_eq!(post(fine, true).await, 200);
}

/// A gzip member around raw deflate of `data`.
fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    out.extend_from_slice(&miniz_oxide_deflate(data));
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn miniz_oxide_deflate(data: &[u8]) -> Vec<u8> {
    // The crate's zlib output less its 2-byte header and 4-byte trailer.
    let z = clax_git::pack::deflate_zlib(data);
    z[2..z.len() - 4].to_vec()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
