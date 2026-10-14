//! The repositories the store writes, read and pushed to by real git over
//! clax-git's smart-HTTP server: one commit per version on `main`, tagged
//! `v<n>`, holding exactly the version's files, with its trailers; a fork's
//! history; who published each version, as `git log` shows it; and a push
//! to `main` that the next write will not discard.

use clax_content_git::MemGitContent;
use clax_core::ArtifactId;
use clax_core::content::{ContentStore, VersionMeta};
use clax_git::versions::repo_name;
use clax_git::{MAIN, Repo};

use crate::common::{Git, Server, clock, committer, write};

#[tokio::test(flavor = "multi_thread")]
async fn a_clone_shows_one_commit_per_version_tagged_with_its_files_and_trailers() {
    let c = MemGitContent::in_memory(committer(), clock());
    let id = ArtifactId::generate();
    c.write_version(
        &id,
        1,
        &write(
            &[],
            &[
                ("index.html", b"<h1>one</h1>\n"),
                ("js/app.js", b"go()\n"),
                ("gone.css", b"x\n"),
            ],
        ),
    )
    .await
    .unwrap();
    c.write_version(
        &id,
        2,
        &write(&["js/app.js"], &[("index.html", b"<h1>two</h1>\n")]),
    )
    .await
    .unwrap();
    c.write_version(
        &id,
        3,
        &write(&["index.html", "js/app.js"], &[("img/a.svg", b"<svg/>\n")]),
    )
    .await
    .unwrap();
    let server = Server::start(c.host().clone()).await;
    let git = Git::new();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    git.text(
        work.path(),
        &[
            "clone",
            "-q",
            &server.url(&repo_name(id.as_str())),
            out.to_str().unwrap(),
        ],
    )
    .await;
    git.text(&out, &["fsck", "--strict", "--no-dangling"]).await;
    assert_eq!(git.text(&out, &["tag", "--list"]).await, "v1\nv2\nv3");
    assert_eq!(
        git.text(&out, &["log", "--format=%s", "main"]).await,
        "Version 3\nVersion 2\nVersion 1"
    );
    let expected: [&[(&str, &[u8])]; 3] = [
        &[
            ("gone.css", b"x\n"),
            ("index.html", b"<h1>one</h1>\n"),
            ("js/app.js", b"go()\n"),
        ],
        &[("index.html", b"<h1>two</h1>\n"), ("js/app.js", b"go()\n")],
        &[
            ("img/a.svg", b"<svg/>\n"),
            ("index.html", b"<h1>two</h1>\n"),
            ("js/app.js", b"go()\n"),
        ],
    ];
    for (i, files) in expected.iter().enumerate() {
        let n = i + 1;
        let tag = format!("v{n}");
        // v<n> is main's commit n - 1 back: one commit per version.
        assert_eq!(
            git.text(&out, &["rev-parse", &tag]).await,
            git.text(&out, &["rev-parse", &format!("main~{}", 3 - n)])
                .await
        );
        let names: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            git.text(&out, &["ls-tree", "-r", "--name-only", &tag])
                .await,
            names.join("\n")
        );
        for (path, bytes) in *files {
            assert_eq!(
                git.bytes(&out, &["show", &format!("{tag}:{path}")]).await,
                *bytes
            );
        }
        assert_eq!(
            git.text(
                &out,
                &[
                    "log",
                    "-1",
                    "--format=%(trailers:key=Clax-Version,valueonly)",
                    &tag
                ]
            )
            .await,
            n.to_string()
        );
        assert_eq!(
            git.text(
                &out,
                &["log", "-1", "--format=%an <%ae> %ad", "--date=raw", &tag]
            )
            .await,
            "Clax <clax@users.clax.dev> 1791892800 +0000"
        );
    }
    assert_eq!(
        git.text(&out, &["rev-list", "--max-parents=0", "main"])
            .await,
        git.text(&out, &["rev-parse", "v1"]).await
    );
    // A carried file is the same blob.
    assert_eq!(
        git.text(&out, &["rev-parse", "v1:js/app.js"]).await,
        git.text(&out, &["rev-parse", "v3:js/app.js"]).await
    );

    // A fork: the source's history up to the forked version behind its own
    // v1, no other tag, made by the forking person and agent.
    let fork = ArtifactId::generate();
    let by = VersionMeta {
        person: Some("u_mia".into()),
        person_name: Some("Mia Park".into()),
        agent: Some("claude".into()),
        note: Some("ignored: a fork's subject names its source".into()),
    };
    c.fork(&id, 2, &["index.html", "js/app.js"], &fork, &by)
        .await
        .unwrap();
    let fork_out = work.path().join("fork");
    git.text(
        work.path(),
        &[
            "clone",
            "-q",
            &server.url(&repo_name(fork.as_str())),
            fork_out.to_str().unwrap(),
        ],
    )
    .await;
    git.text(&fork_out, &["fsck", "--strict", "--no-dangling"])
        .await;
    assert_eq!(git.text(&fork_out, &["tag", "--list"]).await, "v1");
    assert_eq!(
        git.text(&fork_out, &["rev-parse", "v1^"]).await,
        git.text(&out, &["rev-parse", "v2"]).await,
        "v1 sits on the forked version"
    );
    assert_eq!(
        git.text(&fork_out, &["ls-tree", "-r", "--name-only", "v1"])
            .await,
        "index.html\njs/app.js"
    );
    let reference = format!("{id}@v2");
    assert_eq!(
        git.text(
            &fork_out,
            &[
                "log",
                "-1",
                "--format=%s%n%(trailers:key=Clax-Version,valueonly)%(trailers:key=Clax-Publisher,valueonly)%(trailers:key=Clax-Agent,valueonly)%(trailers:key=Clax-Forked-From,valueonly)",
                "v1"
            ]
        )
        .await,
        format!("Forked from {reference}\n1\nu_mia\nclaude\n{reference}")
    );
    assert_eq!(
        git.text(
            &fork_out,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", "v1"]
        )
        .await,
        "Mia Park <u_mia@users.clax.dev>|Mia Park <u_mia@users.clax.dev>"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_after_a_push_to_main_is_refused_and_the_push_kept() {
    let c = MemGitContent::in_memory(committer(), clock());
    let id = ArtifactId::generate();
    c.write_version(&id, 1, &write(&[], &[("index.html", b"one\n")]))
        .await
        .unwrap();
    let server = Server::start(c.host().clone()).await;
    let git = Git::new();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    git.text(
        work.path(),
        &[
            "clone",
            "-q",
            &server.url(&repo_name(id.as_str())),
            out.to_str().unwrap(),
        ],
    )
    .await;
    std::fs::write(out.join("index.html"), "pushed\n").unwrap();
    git.text(
        &out,
        &[
            "-c",
            "user.name=Rae",
            "-c",
            "user.email=rae@example.com",
            "commit",
            "-qam",
            "pushed",
        ],
    )
    .await;
    git.text(&out, &["push", "-q", "origin", "HEAD:main"]).await;
    let pushed = git.text(&out, &["rev-parse", "HEAD"]).await;
    let repo = c.host().repo(&repo_name(id.as_str())).unwrap();
    let before = repo.refs().await.unwrap();
    assert_eq!(before[MAIN].to_hex(), pushed);

    assert!(
        c.write_version(&id, 2, &write(&[], &[("index.html", b"two\n")]))
            .await
            .is_err()
    );
    assert!(c.remove_version(&id, 1).await.is_err());
    assert_eq!(repo.refs().await.unwrap(), before);
    assert_eq!(
        c.read_file(&id, 1, "index.html").await.unwrap().unwrap(),
        b"one\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_log_shows_who_published_each_version_and_the_note() {
    let c = MemGitContent::in_memory(committer(), clock());
    let id = ArtifactId::generate();
    let by =
        |person: &str, name: Option<&str>, agent: Option<&str>, note: Option<&str>| VersionMeta {
            person: Some(person.into()),
            person_name: name.map(Into::into),
            agent: agent.map(Into::into),
            note: note.map(Into::into),
        };
    let metas = [
        by("u_rae", Some("Rae <3"), Some("web"), Some("First draft")),
        by("u_sam", Some("Sam Li"), None, None),
        VersionMeta::default(),
    ];
    for (i, meta) in metas.into_iter().enumerate() {
        let n = u32::try_from(i).unwrap() + 1;
        let body = format!("<p>{n}\n");
        let mut w = write(&[], &[("index.html", body.as_bytes())]);
        w.meta = meta;
        c.write_version(&id, n, &w).await.unwrap();
    }
    let server = Server::start(c.host().clone()).await;
    let git = Git::new();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");
    git.text(
        work.path(),
        &[
            "clone",
            "-q",
            &server.url(&repo_name(id.as_str())),
            out.to_str().unwrap(),
        ],
    )
    .await;
    git.text(&out, &["fsck", "--strict", "--no-dangling"]).await;
    let format = "--format=%an <%ae>|%cn <%ce>|%s|\
        %(trailers:key=Clax-Version,valueonly,separator=)|\
        %(trailers:key=Clax-Publisher,valueonly,separator=)|\
        %(trailers:key=Clax-Agent,valueonly,separator=)";
    assert_eq!(
        git.text(&out, &["log", format, "main"]).await,
        [
            "Clax <clax@users.clax.dev>|Clax <clax@users.clax.dev>|Version 3|3||",
            "Sam Li <u_sam@users.clax.dev>|Sam Li <u_sam@users.clax.dev>|Version 2|2|u_sam|",
            "Rae 3 <u_rae@users.clax.dev>|Rae 3 <u_rae@users.clax.dev>|First draft|1|u_rae|web",
        ]
        .join("\n")
    );
}

/// A version write of `files` published by `name` (`u_<name>`).
fn by<'a>(name: &str, files: &[(&'a str, &'a [u8])]) -> clax_core::content::VersionWrite<'a> {
    let mut w = write(&[], files);
    w.meta = VersionMeta {
        person: Some(format!("u_{}", name.to_lowercase())),
        person_name: Some(name.into()),
        ..Default::default()
    };
    w
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clone_of_a_fork_holds_nothing_the_source_gained_after_the_forked_version() {
    let c = MemGitContent::in_memory(committer(), clock());
    let id = ArtifactId::generate();
    let pages: [(&str, &[u8]); 4] = [
        ("Ann", b"<h1>one</h1>\n"),
        ("Bo", b"<h1>two</h1>\n"),
        ("Cy", b"<h1>three, withheld</h1>\n"),
        ("Di", b"<h1>four, withheld</h1>\n"),
    ];
    for (i, (who, page)) in pages.iter().enumerate() {
        let n = i as u32 + 1;
        c.write_version(
            &id,
            n,
            &by(
                who,
                &[("index.html", page), (&format!("v{n}.txt")[..], page)],
            ),
        )
        .await
        .unwrap();
    }
    let fork = ArtifactId::generate();
    let mia = VersionMeta {
        person: Some("u_mia".into()),
        person_name: Some("Mia".into()),
        ..Default::default()
    };
    c.fork(&id, 2, &["index.html"], &fork, &mia).await.unwrap();

    let server = Server::start(c.host().clone()).await;
    let git = Git::new();
    let work = tempfile::tempdir().unwrap();
    let (src_out, fork_out) = (work.path().join("src"), work.path().join("fork"));
    for (repo, out) in [(&id, &src_out), (&fork, &fork_out)] {
        git.text(
            work.path(),
            &[
                "clone",
                "-q",
                &server.url(&repo_name(repo.as_str())),
                out.to_str().unwrap(),
            ],
        )
        .await;
    }
    git.text(&fork_out, &["fsck", "--strict", "--no-dangling"])
        .await;
    assert_eq!(git.text(&fork_out, &["tag", "--list"]).await, "v1");
    assert_eq!(
        git.text(&fork_out, &["log", "--all", "--format=%an"]).await,
        "Mia\nBo\nAnn",
        "only the authors up to the forked version"
    );
    // What only v3 and v4 hold: their commits, trees and files.
    let mut later = Vec::new();
    for v in ["v3", "v4"] {
        later.push(git.text(&src_out, &["rev-parse", v]).await);
        later.push(
            git.text(&src_out, &["rev-parse", &format!("{v}^{{tree}}")])
                .await,
        );
        later.push(
            git.text(&src_out, &["rev-parse", &format!("{v}:index.html")])
                .await,
        );
        later.push(
            git.text(&src_out, &["rev-parse", &format!("{v}:{v}.txt")])
                .await,
        );
    }
    let objects = git
        .text(&fork_out, &["rev-list", "--all", "--objects"])
        .await;
    let held: Vec<&str> = objects
        .lines()
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    for o in &later {
        assert!(!held.contains(&o.as_str()), "{o} reached in the fork");
        let found = git.run(&fork_out, &["cat-file", "-e", o]).await;
        assert!(!found.status.success(), "{o} in the fork's clone");
    }
    // Nor does the server give them to a fetch that names them.
    for o in &later {
        let fetched = git.run(&fork_out, &["fetch", "-q", "origin", o]).await;
        assert!(!fetched.status.success(), "{o} fetched from the fork");
    }
    // And the repository itself no longer holds them.
    let repo = c.host().repo(&repo_name(fork.as_str())).unwrap();
    for o in &later {
        let oid = clax_git::ObjectId::from_hex(o).unwrap();
        assert!(
            !repo.has_object(&oid).await.unwrap(),
            "{o} held by the fork"
        );
    }
    assert_eq!(
        git.text(&fork_out, &["rev-parse", "v1^"]).await,
        git.text(&src_out, &["rev-parse", "v2"]).await
    );
}
