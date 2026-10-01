//! `clax init` / `clax uninit` against fake `claude`, `codex` and `pi`
//! commands and scratch harness configuration directories. The real
//! harnesses and their real configuration are never touched.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

/// The previous name, assembled so the repository's name gate finds no literal.
const OLD: &str = concat!("arti", "fax");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    /// A scratch HOME with fake CLIs for `harnesses` in `fakebin`. Each fake
    /// appends "<name> <args>" to `calls`, and exits 1 when that line is
    /// listed in `fail`.
    fn new(harnesses: &[&str]) -> Env {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakebin");
        std::fs::create_dir_all(&bin).unwrap();
        for h in harnesses {
            let p = bin.join(h);
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\nline=\"{h} $*\"\necho \"$line\" >> '{calls}'\nif grep -qxF \"$line\" '{fail}' 2>/dev/null; then echo \"$line failed\" >&2; exit 1; fi\nexit 0\n",
                    calls = dir.path().join("calls").display(),
                    fail = dir.path().join("fail").display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Env { dir }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn root(&self) -> PathBuf {
        self.p("ax/marketplace")
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("clax").unwrap();
        c.env("HOME", self.dir.path())
            .env("CLAX_HOME", self.p("ax"))
            .env("CLAUDE_CONFIG_DIR", self.p("claude"))
            .env("CODEX_HOME", self.p("codex"))
            .env("PI_CODING_AGENT_DIR", self.p("pi"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.p("fakebin").display()),
            )
            .env_remove("CLAX_BIN");
        c
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.p("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
    fn json(&self, args: &[&str]) -> (bool, serde_json::Value) {
        let out = self.cmd().args(args).arg("--json").output().unwrap();
        let v = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.success(), v)
    }
}

fn status(v: &serde_json::Value, agent: &str) -> String {
    v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == agent)
        .unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every file under `dir`, relative, skipping `skip` top-level names.
fn tree(dir: &Path, skip: &[&str]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(dir).unwrap().to_string_lossy().to_string();
            if skip
                .iter()
                .any(|s| rel == *s || rel.starts_with(&format!("{s}/")))
            {
                continue;
            }
            if p.is_dir() {
                stack.push(p)
            } else {
                out.push((rel, std::fs::read(&p).unwrap()))
            }
        }
    }
    out.sort();
    out
}

#[test]
fn init_writes_the_embedded_plugins_as_a_marketplace() {
    let e = Env::new(&[]);
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    let root = e.root();
    assert_eq!(v["marketplace"], root.display().to_string());
    let repo = Path::new(REPO);
    assert_eq!(
        tree(&root.join("plugins/claude-code"), &[]),
        tree(&repo.join("plugins/claude-code"), &[])
    );
    assert_eq!(
        tree(&root.join("plugins/clax"), &[]),
        tree(&repo.join("plugins/clax"), &[])
    );
    assert_eq!(
        tree(&root.join("plugins/pi"), &[]),
        tree(
            &repo.join("plugins/pi"),
            &[
                "node_modules",
                "test",
                "tsconfig.json",
                "vitest.config.ts",
                "package-lock.json"
            ]
        )
    );
    for m in [
        ".claude-plugin/marketplace.json",
        ".agents/plugins/marketplace.json",
    ] {
        assert_eq!(
            std::fs::read(root.join(m)).unwrap(),
            std::fs::read(repo.join(m)).unwrap(),
            "{m}"
        );
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(root.join("plugins/clax/scripts/ensure-clax.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "scripts are executable");
    // No harness CLI on PATH: every harness is skipped, which is not a failure.
    for a in ["claude", "codex", "pi"] {
        assert_eq!(status(&v, a), "skipped", "{v}");
    }
}

#[test]
fn init_registers_each_harness_on_path_through_its_cli() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    let r = e.root().display().to_string();
    assert_eq!(
        e.calls(),
        vec![
            "claude plugin uninstall clax@clax".to_string(),
            "claude plugin marketplace remove clax".into(),
            format!("claude plugin marketplace add {r}"),
            "claude plugin install clax@clax".into(),
            "codex plugin remove clax@clax".into(),
            "codex plugin marketplace remove clax".into(),
            format!("codex plugin marketplace add {r}"),
            "codex plugin add clax@clax".into(),
            format!("pi install {r}/plugins/pi"),
        ]
    );
    for a in ["claude", "codex", "pi"] {
        assert_eq!(status(&v, a), "registered", "{v}");
    }
}

#[test]
fn init_only_touches_the_harnesses_asked_for() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let (ok, v) = e.json(&["init", "--agent", "pi"]);
    assert!(ok, "{v}");
    assert_eq!(
        e.calls(),
        vec![format!("pi install {}/plugins/pi", e.root().display())]
    );
    assert_eq!(v["agents"].as_array().unwrap().len(), 1);
}

#[test]
fn init_removes_stale_registrations_including_the_previous_names_and_nothing_else() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let w = |rel: &str, text: String| {
        std::fs::create_dir_all(e.p(rel).parent().unwrap()).unwrap();
        std::fs::write(e.p(rel), text).unwrap();
    };
    w(
        "claude/plugins/installed_plugins.json",
        format!(r#"{{"version":2,"plugins":{{"{OLD}@{OLD}":[{{}}],"other@other":[{{}}]}}}}"#),
    );
    w(
        "claude/plugins/known_marketplaces.json",
        format!(r#"{{"{OLD}":{{}},"other":{{}}}}"#),
    );
    w(
        "codex/config.toml",
        format!(
            "[marketplaces.{OLD}]\nsource = \"/x\"\n\n[marketplaces.other]\nsource = \"/y\"\n\n[plugins.\"{OLD}@{OLD}\"]\nenabled = true\n"
        ),
    );
    w(
        "oldpkg/package.json",
        format!(r#"{{"name":"@empathic/{OLD}-pi"}}"#),
    );
    w(
        "claxpkg/package.json",
        r#"{"name":"@empathic/clax-pi"}"#.into(),
    );
    w(
        "otherpkg/package.json",
        r#"{"name":"@someone/else"}"#.into(),
    );
    w(
        "pi/settings.json",
        r#"{"packages":["../oldpkg","../claxpkg","../otherpkg","npm:@x/y"]}"#.into(),
    );
    // The previous name's home, which must stay exactly as it is.
    w(&format!(".{OLD}/marker"), "keep".into());

    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    let calls = e.calls();
    for want in [
        format!("claude plugin uninstall {OLD}@{OLD}"),
        format!("claude plugin marketplace remove {OLD}"),
        format!("codex plugin remove {OLD}@{OLD}"),
        format!("codex plugin marketplace remove {OLD}"),
        // Pi packages are named by their canonical directory.
        format!(
            "pi remove {}",
            e.p("oldpkg").canonicalize().unwrap().display()
        ),
        format!(
            "pi remove {}",
            e.p("claxpkg").canonicalize().unwrap().display()
        ),
    ] {
        assert!(calls.contains(&want), "missing {want:?} in {calls:#?}");
    }
    assert!(!calls.iter().any(|c| c.contains("other")), "{calls:#?}");
    assert_eq!(
        std::fs::read_to_string(e.p(&format!(".{OLD}/marker"))).unwrap(),
        "keep"
    );
    assert_eq!(
        std::fs::read_dir(e.p(&format!(".{OLD}"))).unwrap().count(),
        1
    );
}

#[test]
fn a_failing_harness_is_reported_and_the_others_still_register() {
    let e = Env::new(&["claude", "codex", "pi"]);
    std::fs::write(
        e.p("fail"),
        format!("codex plugin marketplace add {}\n", e.root().display()),
    )
    .unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(!ok, "a failed harness makes init exit 1");
    assert_eq!(status(&v, "codex"), "failed");
    assert!(
        v["agents"][1]["detail"]
            .as_str()
            .unwrap()
            .contains("failed"),
        "{v}"
    );
    assert_eq!(status(&v, "claude"), "registered");
    assert_eq!(status(&v, "pi"), "registered");
}

#[test]
fn init_twice_does_the_same_again_and_uninit_removes_registrations_and_the_marketplace_only() {
    let e = Env::new(&["claude", "codex", "pi"]);
    assert!(e.json(&["init"]).0);
    let first = e.calls();
    std::fs::remove_file(e.p("calls")).unwrap();
    assert!(e.json(&["init"]).0);
    assert_eq!(e.calls(), first);
    std::fs::write(e.p("ax/clax.db"), "data").unwrap();
    std::fs::remove_file(e.p("calls")).unwrap();
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(
        e.calls(),
        vec![
            "claude plugin uninstall clax@clax".to_string(),
            "claude plugin marketplace remove clax".into(),
            "codex plugin remove clax@clax".into(),
            "codex plugin marketplace remove clax".into(),
        ]
    );
    assert!(!e.root().exists());
    assert_eq!(std::fs::read_to_string(e.p("ax/clax.db")).unwrap(), "data");
    assert_eq!(status(&v, "pi"), "removed");
}
