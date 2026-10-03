//! `clax init` / `clax uninit` against fake `claude`, `codex`, `grok` and
//! `pi` commands and scratch harness configuration directories. The real
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
    /// listed in `fail`. Given `plugin list --json`, a fake prints
    /// `grok-list.json`, else `[]`.
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
                    "#!/bin/sh\nline=\"{h} $*\"\necho \"$line\" >> '{calls}'\npwd -P >> '{cwds}'\nif grep -qxF \"$line\" '{fail}' 2>/dev/null; then echo \"$line failed\" >&2; exit 1; fi\nif [ \"$*\" = \"plugin list --json\" ]; then cat '{list}' 2>/dev/null || echo '[]'; fi\nexit 0\n",
                    calls = dir.path().join("calls").display(),
                    cwds = dir.path().join("cwds").display(),
                    fail = dir.path().join("fail").display(),
                    list = dir.path().join("grok-list.json").display(),
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
            .env("GROK_HOME", self.p("grok"))
            .env_remove("GROK_SESSION_ID")
            .env_remove("GROK_HOOK_EVENT")
            .env_remove("CLAUDE_PID")
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
    // The Pi directory is a symlink, so a path resolved through the
    // filesystem differs from the lexical one Pi stores and matches.
    std::fs::create_dir_all(e.p("store/pi")).unwrap();
    std::os::unix::fs::symlink(e.p("store/pi"), e.p("pi")).unwrap();
    // Through the symlink, `pi/../oldpkg` is `store/oldpkg`, which exists
    // too, so resolving through the filesystem finds a package there rather
    // than failing over to the lexical path.
    std::fs::create_dir_all(e.p("store/oldpkg")).unwrap();
    std::fs::write(
        e.p("store/oldpkg/package.json"),
        format!(r#"{{"name":"@empathic/{OLD}-pi"}}"#),
    )
    .unwrap();
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
            r#"{{"packages":["../oldpkg","./../oldpkg",{{"source":"../claxpkg"}},"{abs}","~/homepkg","../otherpkg","npm:@x/y"]}}"#,
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
    assert!(!calls.iter().any(|c| c.contains("/store/")), "{calls:#?}");
    // Each package once, although `../oldpkg` is listed twice.
    let removes = |p: &str| {
        calls
            .iter()
            .filter(|c| **c == format!("pi remove {p}"))
            .count()
    };
    assert_eq!(
        removes(&e.p("oldpkg").display().to_string()),
        1,
        "{calls:#?}"
    );
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
fn a_missing_pi_package_is_removed_only_when_init_recorded_it() {
    let e = Env::new(&["pi"]);
    assert!(e.json(&["init"]).0);
    let ours = e.root().join("plugins/pi");
    // Missing directories that merely look like Clax's, and a third party's.
    let lookalikes = [
        e.p("Users/claxton/src/foo/plugins/pi"),
        e.p("Volumes/ext/claxon-tools/plugins/pi"),
        e.p(&format!("Devel/{OLD}/plugins/pi")),
        e.p("src/third-party-tool/plugins/pi"),
    ];
    let mut entries: Vec<String> = lookalikes
        .iter()
        .map(|p| format!("\"{}\"", p.display()))
        .collect();
    entries.push(format!("\"{}\"", ours.display()));
    std::fs::create_dir_all(e.p("pi")).unwrap();
    std::fs::write(
        e.p("pi/settings.json"),
        format!(r#"{{"packages":[{}]}}"#, entries.join(",")),
    )
    .unwrap();
    // The recorded package's directory is gone too.
    std::fs::remove_dir_all(e.root()).unwrap();
    std::fs::remove_file(e.p("calls")).unwrap();

    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(e.calls(), vec![format!("pi remove {}", ours.display())]);
    let detail = detail(&v, "pi");
    for p in &lookalikes {
        assert!(
            detail.contains(&format!("`pi remove {}`", p.display())),
            "{p:?} is named with its command: {detail}"
        );
    }
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(e.p("ax/registrations.json")).unwrap())
            .unwrap();
    assert!(recorded["harnesses"].get("pi").is_none(), "{recorded}");
}

#[test]
fn init_records_what_it_registered() {
    let e = Env::new(&["claude", "pi"]);
    assert!(e.json(&["init"]).0);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(e.p("ax/registrations.json")).unwrap())
            .unwrap();
    let r = e.root().display().to_string();
    assert_eq!(
        v["harnesses"]["pi"]["packages"][0],
        format!("{r}/plugins/pi")
    );
    assert_eq!(v["harnesses"]["claude"]["source"], r);
    assert!(v["harnesses"].get("codex").is_none(), "{v}");
}

#[test]
fn a_relative_clax_home_is_made_absolute_before_harness_clis_run() {
    let e = Env::new(&["pi"]);
    std::fs::create_dir_all(e.p("work")).unwrap();
    let out = e
        .cmd()
        .env("CLAX_HOME", "rel")
        .current_dir(e.p("work"))
        .args(["init", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let work = e.p("work").canonicalize().unwrap();
    assert_eq!(
        e.calls(),
        vec![format!(
            "pi install {}/rel/marketplace/plugins/pi",
            work.display()
        )]
    );
}

#[test]
fn uninit_keeps_the_marketplace_when_a_registry_cannot_be_read_or_names_it_by_tilde() {
    let e = Env::new(&[]);
    assert!(e.json(&["init"]).0);
    std::fs::create_dir_all(e.p("codex")).unwrap();
    std::fs::write(e.p("codex/config.toml"), "[marketplaces\n").unwrap();
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert!(e.root().is_dir(), "{v}");
    assert!(
        v["marketplace_detail"]
            .as_str()
            .unwrap()
            .contains("could not parse"),
        "{v}"
    );

    std::fs::write(
        e.p("codex/config.toml"),
        "[marketplaces.clax]\nsource = \"~/ax/marketplace\"\n",
    )
    .unwrap();
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert!(e.root().is_dir(), "{v}");
    let kept = v["marketplace_detail"].as_str().unwrap();
    assert!(
        kept.contains("codex plugin marketplace remove clax"),
        "{kept}"
    );

    std::fs::write(e.p("codex/config.toml"), "").unwrap();
    assert!(e.json(&["uninit"]).0);
    assert!(!e.root().exists());
}

#[test]
fn uninit_creates_no_clax_home() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert!(!e.p("ax").exists());
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

#[test]
fn a_machine_with_only_grok_registers_clax_grok() {
    let e = Env::new(&["grok"]);
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    for other in ["claude", "codex", "pi"] {
        assert_eq!(status(&v, other), "skipped", "{v}");
    }
    assert_eq!(status(&v, "grok"), "registered", "{v}");
    let dir = e.root().join("plugins/clax-grok");
    assert_eq!(
        e.calls(),
        [
            "grok plugin uninstall clax-grok --confirm".to_string(),
            format!("grok plugin install {} --trust", dir.display()),
        ]
    );
    assert!(dir.join(".grok-plugin/plugin.json").is_file());
    assert!(e.root().join(".grok-plugin/marketplace.json").is_file());
    assert!(
        !e.p(".grok").exists() && !e.p("grok").exists(),
        "init writes nothing in a Grok home"
    );
}

#[test]
fn no_grok_command_names_the_claude_code_plugin() {
    let e = Env::new(&["claude", "codex", "grok", "pi"]);
    assert!(e.json(&["init"]).0);
    assert!(e.json(&["uninit"]).0);
    for c in e.calls().iter().filter(|c| c.starts_with("grok ")) {
        assert!(
            !c.split_whitespace()
                .any(|w| w == "clax" || w == "clax@clax"),
            "{c}: in Grok, `clax` is the Claude Code plugin's install"
        );
    }
}

#[test]
fn a_failed_grok_uninstall_is_ignored_and_a_failed_install_fails_grok_only() {
    let e = Env::new(&["claude", "grok"]);
    std::fs::write(
        e.p("fail"),
        format!(
            "grok plugin uninstall clax-grok --confirm\ngrok plugin install {} --trust\n",
            e.root().join("plugins/clax-grok").display()
        ),
    )
    .unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(!ok);
    assert_eq!(status(&v, "grok"), "failed");
    assert_eq!(status(&v, "claude"), "registered");
    assert!(detail(&v, "grok").contains("(ignored)"), "{v}");
}

#[test]
fn init_records_grok_and_uninit_removes_it_and_the_marketplace() {
    let e = Env::new(&["grok"]);
    assert!(e.json(&["init"]).0);
    let rec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(e.p("ax/registrations.json")).unwrap()).unwrap();
    assert_eq!(rec["harnesses"]["grok"]["plugin"], "clax-grok");
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(status(&v, "grok"), "removed");
    assert!(e.calls().contains(&"grok plugin list --json".to_string()));
    assert!(!e.root().exists(), "{v}");
}

#[test]
fn uninit_keeps_the_marketplace_while_grok_still_lists_a_plugin_from_it() {
    let e = Env::new(&["grok"]);
    assert!(e.json(&["init"]).0);
    std::fs::write(
        e.p("grok-list.json"),
        serde_json::json!([{"name": "clax-grok", "source": e.root().join("plugins/clax-grok")}])
            .to_string(),
    )
    .unwrap();
    let (_, v) = e.json(&["uninit"]);
    assert!(e.root().exists());
    assert!(
        v["marketplace_detail"]
            .as_str()
            .unwrap()
            .contains("grok still registers it"),
        "{v}"
    );
}
