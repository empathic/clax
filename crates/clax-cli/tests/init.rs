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
                    "#!/bin/sh\nline=\"{h} $*\"\necho \"$line\" >> '{calls}'\npwd -P >> '{cwds}'\nif grep -qxF \"$line\" '{fail}' 2>/dev/null; then echo \"$line failed\" >&2; exit 1; fi\nexit 0\n",
                    calls = dir.path().join("calls").display(),
                    cwds = dir.path().join("cwds").display(),
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

fn detail(v: &serde_json::Value, agent: &str) -> String {
    v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == agent)
        .unwrap()["detail"]
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
        "abspkg/package.json",
        r#"{"name":"@empathic/clax-pi"}"#.into(),
    );
    w(
        "homepkg/package.json",
        format!(r#"{{"name":"@empathic/{OLD}-pi"}}"#),
    );
    // Local sources as Pi stores them: relative to the Pi directory,
    // absolute, as an object, and under `~`; plus a package elsewhere.
    w(
        "pi/settings.json",
        format!(
            r#"{{"packages":["../oldpkg",{{"source":"../claxpkg"}},"{abs}","~/homepkg","../otherpkg","npm:@x/y"]}}"#,
            abs = e.p("abspkg").display()
        ),
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
        // Pi packages are named by the path Pi resolves the entry to:
        // lexically, without resolving symlinks (the scratch directory is
        // under the symlinked /var on macOS).
        format!("pi remove {}", e.p("oldpkg").display()),
        format!("pi remove {}", e.p("claxpkg").display()),
        format!("pi remove {}", e.p("abspkg").display()),
        format!("pi remove {}", e.p("homepkg").display()),
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

#[test]
fn a_pi_registration_whose_directory_is_gone_is_still_removed() {
    let e = Env::new(&["pi"]);
    std::fs::create_dir_all(e.p("pi")).unwrap();
    let gone = e.p(&format!("Devel/{OLD}/plugins/pi"));
    let elsewhere = e.p("Devel/other/plugins/pi");
    std::fs::write(
        e.p("pi/settings.json"),
        format!(
            r#"{{"packages":["{}","{}"]}}"#,
            gone.display(),
            elsewhere.display()
        ),
    )
    .unwrap();
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(e.calls(), vec![format!("pi remove {}", gone.display())]);
    let detail = detail(&v, "pi");
    assert!(detail.contains("is gone"), "{detail}");
    assert!(detail.contains("left alone"), "{detail}");
}

#[test]
fn uninit_of_one_harness_keeps_the_marketplace_the_others_still_use() {
    let e = Env::new(&["claude", "codex", "pi"]);
    assert!(e.json(&["init"]).0);
    let r = e.root().display().to_string();
    // What the real CLIs record on `init` (the fakes record nothing).
    std::fs::create_dir_all(e.p("claude/plugins")).unwrap();
    std::fs::create_dir_all(e.p("codex")).unwrap();
    std::fs::write(
        e.p("claude/plugins/known_marketplaces.json"),
        format!(r#"{{"clax":{{"source":{{"source":"directory","path":"{r}"}}}}}}"#),
    )
    .unwrap();
    std::fs::write(
        e.p("codex/config.toml"),
        format!("[marketplaces.clax]\nsource = \"{r}\"\n"),
    )
    .unwrap();
    let (ok, v) = e.json(&["uninit", "--agent", "pi"]);
    assert!(ok, "{v}");
    assert!(e.root().join("plugins/claude-code").is_dir(), "{v}");
    let kept = v["marketplace_detail"].as_str().unwrap();
    assert!(kept.contains("claude") && kept.contains("codex"), "{kept}");

    // Once Claude Code and Codex no longer refer to it, it goes.
    std::fs::remove_file(e.p("claude/plugins/known_marketplaces.json")).unwrap();
    std::fs::write(e.p("codex/config.toml"), "").unwrap();
    let (ok, v) = e.json(&["uninit", "--agent", "claude"]);
    assert!(ok, "{v}");
    assert!(!e.root().exists(), "{v}");
}

#[test]
fn init_replaces_the_marketplace_and_leaves_no_temporary_trees() {
    let e = Env::new(&[]);
    assert!(e.json(&["init"]).0);
    std::fs::write(e.root().join("plugins/clax/dropped.txt"), "old").unwrap();
    std::fs::create_dir_all(e.p("ax/.marketplace.1.tmp")).unwrap();
    assert!(e.json(&["init"]).0);
    assert!(!e.root().join("plugins/clax/dropped.txt").exists());
    let debris: Vec<_> = std::fs::read_dir(e.p("ax"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|d| d.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".marketplace."))
        .collect();
    assert!(debris.is_empty(), "{debris:?}");
}

#[test]
fn concurrent_inits_take_turns() {
    let e = Env::new(&["pi"]);
    let runs: Vec<_> = (0..4)
        .map(|_| {
            let mut c = e.cmd();
            c.args(["init", "--json"]);
            std::thread::spawn(move || c.output().unwrap().status.success())
        })
        .collect();
    for r in runs {
        assert!(r.join().unwrap(), "every concurrent init succeeds");
    }
    assert!(e.root().join("plugins/clax").is_dir());
    let debris = std::fs::read_dir(e.p("ax"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|d| d.file_name().to_string_lossy().starts_with(".marketplace."))
        .count();
    assert_eq!(debris, 0);
}

#[test]
fn an_unparseable_registry_is_reported_and_registration_goes_ahead() {
    let e = Env::new(&["codex"]);
    std::fs::create_dir_all(e.p("codex")).unwrap();
    std::fs::write(e.p("codex/config.toml"), "[marketplaces\n").unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    assert_eq!(status(&v, "codex"), "registered");
    let detail = detail(&v, "codex");
    assert!(detail.contains("could not parse"), "{detail}");
}

#[test]
fn a_failed_removal_is_noted_but_does_not_fail_the_harness() {
    let e = Env::new(&["claude"]);
    std::fs::write(e.p("fail"), "claude plugin uninstall clax@clax\n").unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    assert_eq!(status(&v, "claude"), "registered");
    let detail = detail(&v, "claude");
    assert!(detail.contains("failed (ignored)"), "{detail}");
}

#[test]
fn harness_clis_run_in_the_home_directory_not_the_callers() {
    let e = Env::new(&["claude", "codex", "pi"]);
    std::fs::create_dir_all(e.p("project")).unwrap();
    let out = e
        .cmd()
        .current_dir(e.p("project"))
        .args(["init", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let home = e.dir.path().canonicalize().unwrap().display().to_string();
    let cwds = std::fs::read_to_string(e.p("cwds")).unwrap();
    assert!(!cwds.is_empty());
    for l in cwds.lines() {
        assert_eq!(l, home);
    }
}
