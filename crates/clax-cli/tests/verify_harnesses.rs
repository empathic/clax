//! `scripts/verify-harnesses.sh` against fake `claude`, `codex` and `pi`
//! commands that keep their registries the way the real ones do (as far as
//! the script reads them). HOME is a sentinel directory that must come out
//! unchanged: the script works only in its own scratch root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// One fake for all three CLIs, dispatching on its name. Each call is
/// logged with the homes it saw. Registries are regenerated from state
/// files after every change, as valid JSON and TOML.
const FAKE: &str = r#"#!/bin/sh
me="$(basename "$0")"
echo "$me $* | HOME=$HOME CLAX_HOME=$CLAX_HOME CODEX_HOME=$CODEX_HOME CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR PI_CODING_AGENT_DIR=$PI_CODING_AGENT_DIR" >> "$FAKE_LOG"
[ "$1" = "--version" ] && { echo "fake $me 1.0"; exit 0; }
first_name() { grep -m1 '"name"' "$1" | sed 's/.*"name": *"\([^"]*\)".*/\1/'; }
market_path() { awk -F'\t' -v n="$2" '$1 == n { print $2 }' "$1"; }
case "$me" in
claude)
    S="$CLAUDE_CONFIG_DIR/.fake"; mkdir -p "$S" "$CLAUDE_CONFIG_DIR/plugins"; touch "$S/markets" "$S/plugins"
    gen() {
        d="$CLAUDE_CONFIG_DIR/plugins"
        { printf '{'; sep=''; while IFS="$(printf '\t')" read -r n p; do printf '%s"%s":{"source":{"source":"directory","path":"%s"}}' "$sep" "$n" "$p"; sep=','; done < "$S/markets"; printf '}\n'; } > "$d/known_marketplaces.json"
        { printf '{"version":2,"plugins":{'; sep=''; while IFS="$(printf '\t')" read -r pl ip; do printf '%s"%s":[{"installPath":"%s"}]' "$sep" "$pl" "$ip"; sep=','; done < "$S/plugins"; printf '}}\n'; } > "$d/installed_plugins.json"
    }
    case "$2 $3" in
    "marketplace add") n="$(first_name "$4/.claude-plugin/marketplace.json")"; printf '%s\t%s\n' "$n" "$4" >> "$S/markets" ;;
    "marketplace remove") grep -q "^$4	" "$S/markets" || { echo "not found" >&2; exit 1; }; grep -v "^$4	" "$S/markets" > "$S/m.tmp"; mv "$S/m.tmp" "$S/markets" ;;
    "install "*) p="${3%@*}"; m="${3#*@}"; mp="$(market_path "$S/markets" "$m")"; [ -n "$mp" ] || { echo "no marketplace $m" >&2; exit 1; }
        c="$CLAUDE_CONFIG_DIR/plugins/cache/$m/$p/1"; rm -rf "$c"; mkdir -p "$(dirname "$c")"; cp -R "$mp/plugins/claude-code" "$c"
        grep -v "^$3	" "$S/plugins" > "$S/p.tmp"; mv "$S/p.tmp" "$S/plugins"; printf '%s\t%s\n' "$3" "$c" >> "$S/plugins" ;;
    "uninstall "*) grep -q "^$3	" "$S/plugins" || { echo "not installed" >&2; exit 1; }; grep -v "^$3	" "$S/plugins" > "$S/p.tmp"; mv "$S/p.tmp" "$S/plugins"
        rm -rf "$CLAUDE_CONFIG_DIR/plugins/cache/${3#*@}/${3%@*}" ;;
    *) echo "fake claude: unexpected $*" >&2; exit 3 ;;
    esac
    gen ;;
codex)
    S="$CODEX_HOME/.fake"; mkdir -p "$S"; touch "$S/markets" "$CODEX_HOME/config.toml"
    drop() { awk -v h="$1" '$0 == h { skip = 1; next } skip && /^\[/ { skip = 0 } !skip { print }' "$CODEX_HOME/config.toml" > "$S/c.tmp"; mv "$S/c.tmp" "$CODEX_HOME/config.toml"; }
    case "$2 $3" in
    "marketplace add") n="$(first_name "$4/.agents/plugins/marketplace.json")"; printf '%s\t%s\n' "$n" "$4" >> "$S/markets"
        printf '\n[marketplaces.%s]\nsource_type = "local"\nsource = "%s"\n' "$n" "$4" >> "$CODEX_HOME/config.toml" ;;
    "marketplace remove") grep -q "^\[marketplaces.$4\]$" "$CODEX_HOME/config.toml" || { echo "not found" >&2; exit 1; }
        drop "[marketplaces.$4]"; grep -v "^$4	" "$S/markets" > "$S/m.tmp"; mv "$S/m.tmp" "$S/markets" ;;
    "add "*) p="${3%@*}"; m="${3#*@}"; mp="$(market_path "$S/markets" "$m")"; [ -n "$mp" ] || { echo "no marketplace $m" >&2; exit 1; }
        c="$CODEX_HOME/plugins/cache/$m/$p/1"; rm -rf "$c"; mkdir -p "$(dirname "$c")"; cp -R "$mp/plugins/clax" "$c"
        drop "[plugins.\"$3\"]"; printf '\n[plugins."%s"]\nenabled = true\n' "$3" >> "$CODEX_HOME/config.toml" ;;
    "remove "*) grep -qF "[plugins.\"$3\"]" "$CODEX_HOME/config.toml" || { echo "not installed" >&2; exit 1; }
        drop "[plugins.\"$3\"]"; rm -rf "$CODEX_HOME/plugins/cache/${3#*@}/${3%@*}" ;;
    *) echo "fake codex: unexpected $*" >&2; exit 3 ;;
    esac ;;
pi)
    S="$PI_CODING_AGENT_DIR/.fake"; mkdir -p "$S"; touch "$S/packages"
    case "$1" in
    install) [ -d "$2" ] || { echo "Path does not exist" >&2; exit 1; }; grep -vxF "$2" "$S/packages" > "$S/p.tmp"; mv "$S/p.tmp" "$S/packages"; echo "$2" >> "$S/packages" ;;
    remove) grep -qxF "$2" "$S/packages" || { echo "No matching package found" >&2; exit 1; }; grep -vxF "$2" "$S/packages" > "$S/p.tmp"; mv "$S/p.tmp" "$S/packages" ;;
    *) echo "fake pi: unexpected $*" >&2; exit 3 ;;
    esac
    { printf '{"packages":['; sep=''; while read -r p; do printf '%s"%s"' "$sep" "$p"; sep=','; done < "$S/packages"; printf ']}\n'; } > "$PI_CODING_AGENT_DIR/settings.json" ;;
esac
exit 0
"#;

struct Run {
    dir: tempfile::TempDir,
}

impl Run {
    fn new() -> Run {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakebin");
        std::fs::create_dir_all(&bin).unwrap();
        for h in ["claude", "codex", "pi"] {
            let p = bin.join(h);
            std::fs::write(&p, FAKE).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let sentinel = dir.path().join("sentinel");
        std::fs::create_dir_all(sentinel.join(".claude/plugins")).unwrap();
        std::fs::write(sentinel.join(".claude/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(sentinel.join(".codex")).unwrap();
        std::fs::write(sentinel.join(".codex/config.toml"), "# real\n").unwrap();
        std::fs::create_dir_all(sentinel.join(".clax")).unwrap();
        std::fs::write(sentinel.join(".clax/marker"), "real").unwrap();
        std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
        Run { dir }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn run(&self, tmpdir: &Path) -> std::process::Output {
        Command::new("bash")
            .arg(format!("{REPO}/scripts/verify-harnesses.sh"))
            .env_clear()
            .env("HOME", self.p("sentinel"))
            .env("TMPDIR", tmpdir)
            .env("CLAX_BIN", env!("CARGO_BIN_EXE_clax"))
            .env("FAKE_LOG", self.p("calls.log"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.p("fakebin").display()),
            )
            .output()
            .unwrap()
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.p("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Every file under `dir` with its contents and modification time, and
/// every directory.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                out.insert(p.clone(), "dir".into());
                stack.push(p);
            } else {
                let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).to_string();
                out.insert(p, format!("{:?} {text}", m.modified().unwrap()));
            }
        }
    }
    out
}

#[test]
fn the_script_passes_against_fakes_and_never_touches_the_real_home() {
    let r = Run::new();
    let before = snapshot(&r.p("sentinel"));
    let out = r.run(&r.p("tmp"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(!stdout.contains("\nFAIL"), "{stdout}");
    for want in [
        "init exits 0",
        "claude: init removes the previous name's registration",
        "codex: init keeps other config.toml content",
        "pi: the deleted previous-name checkout is left and named",
        "claude: init again replaces the cached copy",
        "pi: one entry after init again",
        "codex: doctor plugin passes",
        "uninit --agent pi keeps the copy the others use",
        "uninit removes the marketplace copy",
    ] {
        assert!(
            stdout
                .lines()
                .any(|l| l.starts_with("PASS") && l.contains(want)),
            "no PASS for {want:?}:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("SKIPPED  pi: a session loads the extension"),
        "{stdout}"
    );

    // Every harness call saw the scratch homes, never the sentinel.
    let root = stdout
        .lines()
        .find_map(|l| l.strip_prefix("scratch root: "))
        .and_then(|l| l.split(" (").next())
        .unwrap()
        .to_string();
    let calls = r.calls();
    assert!(calls.len() > 10, "{calls:#?}");
    let sentinel = r.p("sentinel").display().to_string();
    for c in &calls {
        let homes = c.split(" | ").nth(1).unwrap();
        for kv in homes.split(' ') {
            let v = kv.split_once('=').unwrap().1;
            assert!(v.starts_with(&format!("{root}/")), "{kv} in {c}");
            assert!(!v.starts_with(&sentinel), "{kv} in {c}");
        }
    }
    assert!(!Path::new(&root).exists(), "the scratch root is removed");
    assert_eq!(
        snapshot(&r.p("sentinel")),
        before,
        "the sentinel HOME is unchanged"
    );
}

#[test]
fn the_script_refuses_a_scratch_root_inside_the_real_home() {
    let r = Run::new();
    std::fs::create_dir_all(r.p("sentinel/tmp")).unwrap();
    let before = snapshot(&r.p("sentinel"));
    let out = r.run(&r.p("sentinel/tmp"));
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside your real HOME"));
    assert!(r.calls().is_empty(), "{:#?}", r.calls());
    assert_eq!(snapshot(&r.p("sentinel")), before);
}
