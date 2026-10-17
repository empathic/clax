//! `clax people`, `clax agents rename` and the People link (spec §11,
//! §16, M20): the built `clax` binary run by hand as the owner, in the
//! test daemon's own home, and under a harness. Changes M20 reserves for a
//! browser are never made from the CLI: their commands print the People
//! link and exit 2.

use clax_testkit::people::World;
use serde_json::Value;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// Longest a `clax` run may take.
const LIMIT: Duration = Duration::from_secs(60);

/// The harness environment a test inherits from the agent running it.
const HARNESS_ENV: [&str; 9] = [
    "CLAX_CHANNEL",
    "CLAUDECODE",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_CI",
    "PI_CODING_AGENT",
    "CLAX_REMOTE",
    "CLAX_PORT",
    "CLAX_SESSION_ID",
];

/// The owner's machine: the world's daemon home, which `daemon.json` names
/// the test daemon in, so the CLI starts none.
struct Owner<'a> {
    w: &'a World,
    dir: tempfile::TempDir,
}

impl<'a> Owner<'a> {
    fn new(w: &'a World) -> Owner<'a> {
        let info = clax_core::daemon::DaemonInfo {
            port: w.ts.addr.port(),
            pid: std::process::id(),
            token: w.ts.token.clone(),
            started_at: w.ts.store.now(),
            bind: "127.0.0.1".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            exe: None,
            commit: None,
        };
        clax_core::daemon::write_daemon_info(&w.ts.home, &info).unwrap();
        Owner {
            w,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// Runs `clax args` with `env`, to its end, off the test's runtime.
    async fn run_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(crate::common::clax());
        for v in HARNESS_ENV {
            c.env_remove(v);
        }
        c.env("CLAX_HOME", self.w.ts.home.root())
            .env("HOME", self.dir.path())
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env("RUST_LOG", "error")
            .env("NO_COLOR", "1")
            .args(args)
            .stdin(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        let label = args.join(" ");
        tokio::time::timeout(
            LIMIT,
            tokio::task::spawn_blocking(move || c.output().unwrap()),
        )
        .await
        .unwrap_or_else(|_| panic!("clax {label} did not finish"))
        .unwrap()
    }

    async fn run(&self, args: &[&str]) -> Output {
        self.run_with(args, &[]).await
    }

    /// Runs `clax args`, asserting it succeeds; its stdout.
    async fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args).await;
        assert!(
            out.status.success(),
            "clax {}: {}",
            args.join(" "),
            text(&out)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// The People link the CLI prints.
    fn people_link(&self) -> String {
        format!("http://localhost:{}/people", self.w.ts.addr.port())
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn people_lists_people_machines_and_agents_with_the_people_link() {
    let w = World::two_people().await;
    let o = Owner::new(&w);
    let sams = w.ts.store.get_session(&w.sam_session).unwrap().unwrap();
    let machine = w.ts.store.grant(&w.sam.grant_id()).unwrap().client_name;
    let out = o.ok(&["people"]).await;
    for want in [
        "Sam: clax:collaborator",
        "Mia: clax:collaborator",
        &format!("machine {machine}, connected"),
        &format!("agent {} (claude)", sams.name.clone().unwrap()),
        &o.people_link(),
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    let v: Value = serde_json::from_str(&o.ok(&["--json", "people"]).await).unwrap();
    let names: Vec<&str> = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Sam", "Mia"], "{v}");
    assert_eq!(v["url"], o.people_link(), "{v}");
    // Under a harness, the people are for people.
    let out = o.run_with(&["people"], &[("CLAUDECODE", "1")]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains(&o.people_link()), "{}", text(&out));
    assert!(!text(&out).contains("Mia"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_rename_names_an_agent_and_not_from_a_harness() {
    let w = World::two_people().await;
    let o = Owner::new(&w);
    let name = |sid: &str| w.ts.store.get_session(sid).unwrap().unwrap();
    let sams = name(&w.sam_session);
    // By name, then by handle.
    let out = o
        .ok(&[
            "agents",
            "rename",
            sams.name.as_deref().unwrap(),
            "sams-task",
        ])
        .await;
    assert!(
        out.contains(&format!("renamed {} to sams-task", sams.name.unwrap())),
        "{out}"
    );
    assert_eq!(name(&w.sam_session).name.as_deref(), Some("sams-task"));
    let v: Value = serde_json::from_str(
        &o.ok(&[
            "--json",
            "agents",
            "rename",
            &sams.agent_handle,
            "sams-plan",
        ])
        .await,
    )
    .unwrap();
    assert_eq!(v["agent"]["name"], "sams-plan", "{v}");
    assert_eq!(name(&w.sam_session).name.as_deref(), Some("sams-plan"));
    // An agent that is not live, or a name taken, fails.
    let out = o.run(&["agents", "rename", "nobody-here", "x"]).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let owners = name(&w.owner_session).name.unwrap();
    let out = o.run(&["agents", "rename", "sams-plan", &owners]).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("name_taken"), "{}", text(&out));
    // Under a harness, refused with the People link; nothing changes.
    let out = o
        .run_with(
            &["agents", "rename", "sams-plan", "by-an-agent"],
            &[("CLAUDECODE", "1")],
        )
        .await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains(&o.people_link()), "{}", text(&out));
    assert_eq!(name(&w.sam_session).name.as_deref(), Some("sams-plan"));
    // The listing still works.
    assert!(o.ok(&["agents"]).await.contains("sams-plan"));
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_only_changes_print_the_people_link_and_change_nothing() {
    let w = World::two_people().await;
    let o = Owner::new(&w);
    let invites = w.ts.store.list_invites().unwrap().len();
    let sams = w.ts.store.get_session(&w.sam_session).unwrap().unwrap();
    let agent = sams.name.unwrap();
    for args in [
        vec!["people", "invite", "Ivy"],
        vec!["people", "rights", "Sam", "clax:full"],
        vec!["people", "revoke", "Sam"],
        vec!["agents", "narrow", &agent],
        vec!["agents", "elevate", &agent],
        vec!["approvals", "approve", "Mia"],
        vec!["approvals", "decide", "r_1", "thread"],
        vec!["approvals", "revoke", "a_1"],
    ] {
        let out = o.run(&args).await;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        let t = text(&out);
        assert!(t.contains(&o.people_link()), "{args:?}: {t}");
        assert!(t.contains("browser"), "{args:?}: {t}");
    }
    assert_eq!(w.ts.store.list_invites().unwrap().len(), invites);
    assert!(
        w.ts.store
            .get_session(&w.sam_session)
            .unwrap()
            .unwrap()
            .name
            == Some(agent)
    );
    // `clax approvals` lists, and names the People link.
    assert!(o.ok(&["approvals"]).await.contains(&o.people_link()));
}
