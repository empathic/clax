//! `scripts/verify-harnesses.sh` against fake `claude`, `codex` and `pi`
//! commands that keep their registries the way the real ones do (as far as
//! the script reads them). HOME is a sentinel directory that must come out
//! unchanged: the script works only in its own scratch root.
//!
//! Each fake logs, per call, its working directory, the `clax` that PATH
//! finds, the scratch daemon port, every environment variable name it
//! received and the homes it saw. The build under test is a wrapper that
//! logs each Clax command and then runs the real binary, and a decoy `clax`
//! sits on the caller's PATH. Together they pin the script's safety lines:
//! the scratch port, `clax stop` on exit, the build first on PATH, the
//! scratch working directory and the environment allowlist.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
/// The previous product name, assembled.
const OLD: &str = concat!("arti", "fax");

/// One fake for all three CLIs, dispatching on its name. Registries are
/// regenerated from state files after every change, as valid JSON and TOML.
/// A file `hang-<name>` next to the fake's directory makes every call but
/// `--version` hang for 30 seconds, longer than the timeout the test sets,
/// and then exit 0 having done nothing.
const FAKE: &str = r#"#!/bin/sh
me="${0##*/}"
here="${0%/*}"
names="$(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p' | grep -vxE 'PWD|SHLVL|_|OLDPWD|__CF_USER_TEXT_ENCODING' | sort | tr '\n' ',')"
port="$(sed -n 's/^port = //p' "$CLAX_HOME/config.toml" 2>/dev/null)"
printf '%s %s\tPWD=%s\tWHICH=%s\tPORT=%s\tENV=%s\tHOME=%s\tCLAX_HOME=%s\tCODEX_HOME=%s\tCLAUDE_CONFIG_DIR=%s\tPI_CODING_AGENT_DIR=%s\tTMPDIR=%s\tCLAX_BIN=%s\n' \
    "$me" "$*" "$PWD" "$(command -v clax)" "$port" "$names" "$HOME" "$CLAX_HOME" "$CODEX_HOME" "$CLAUDE_CONFIG_DIR" "$PI_CODING_AGENT_DIR" "$TMPDIR" "$CLAX_BIN" >> "$here/../calls.log"
[ "$1" = "--version" ] && { echo "fake $me 1.0"; exit 0; }
if [ -e "$here/../hang-$me" ]; then echo $$ >> "$here/../hang-pids"; exec sleep 30; fi
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
    -*) echo "clax_status: the fake session ran"; exit 0 ;;
    *) echo "fake pi: unexpected $*" >&2; exit 3 ;;
    esac
    { printf '{"packages":['; sep=''; while read -r p; do printf '%s"%s"' "$sep" "$p"; sep=','; done < "$S/packages"; printf ']}\n'; } > "$PI_CODING_AGENT_DIR/settings.json" ;;
esac
exit 0
"#;

/// The variables every harness step may see, besides those the shell
/// (PWD, SHLVL, _, OLDPWD) and macOS (__CF_USER_TEXT_ENCODING, which
/// CoreFoundation sets in a process that uses it) add themselves.
const ALLOWED: &[&str] = &[
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "CLAUDE_CONFIG_DIR",
    "CLAX_BIN",
    "CLAX_HOME",
    "CLAX_NO_OPEN",
    "CODEX_HOME",
    "DISABLE_AUTOUPDATER",
    "HOME",
    "LANG",
    "PATH",
    "PI_CODING_AGENT_DIR",
    "PI_OFFLINE",
    "TERM",
    "TMPDIR",
];

/// Variables in the caller's environment that would point a harness back
/// at real state; none may reach a harness step.
const HOSTILE: &[(&str, &str)] = &[
    ("CLAUDE_SECURESTORAGE_CONFIG_DIR", ""),
    ("CLAUDE_CODE_PLUGIN_CACHE_DIR", "/nonexistent/real"),
    ("CLAUDE_CODE_SESSION_ID", "real-session"),
    ("CODEX_SQLITE_HOME", "/nonexistent/real"),
    ("PI_PACKAGE_DIR", "/nonexistent/real"),
    ("npm_config_prefix", "/nonexistent/real"),
    ("XDG_CONFIG_HOME", "/nonexistent/real"),
    ("ANTHROPIC_API_KEY", "sk-test"),
];

struct Run {
    dir: tempfile::TempDir,
}

/// What one fake call logged.
struct Call {
    cmd: String,
    f: BTreeMap<String, String>,
}

fn write_exe(p: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Run {
    fn new() -> Run {
        let dir = tempfile::tempdir().unwrap();
        let r = Run { dir };
        for h in ["claude", "codex", "pi"] {
            write_exe(&r.p(&format!("fakebin/{h}")), FAKE);
        }
        // The build under test: logs each Clax command, then runs the real
        // one, naming it in CLAX_BIN (the binary the plugins run) so that
        // `doctor` sees the plugins run the clax it is.
        write_exe(
            &r.wrapper(),
            &format!(
                "#!/bin/sh\nprintf 'clax %s\\tCLAX_HOME=%s\\tHOME=%s\\n' \"$*\" \"$CLAX_HOME\" \"$HOME\" >> \"${{0%/*}}/../clax-calls.log\"\nCLAX_BIN='{0}' exec '{0}' \"$@\"\n",
                env!("CARGO_BIN_EXE_clax")
            ),
        );
        // Another clax on the caller's PATH, which must never be the one found.
        write_exe(&r.p("decoybin/clax"), "#!/bin/sh\necho decoy\nexit 1\n");
        // A cargo that reports the wrapper as the build it made.
        write_exe(
            &r.p("fakebin/cargo"),
            &format!(
                "#!/bin/sh\nprintf 'cargo %s\\tPWD=%s\\tHOME=%s\\tCARGO_TARGET_DIR=%s\\n' \"$*\" \"$PWD\" \"$HOME\" \"$CARGO_TARGET_DIR\" >> \"${{0%/*}}/../cargo.log\"\n\
                 echo '{{\"reason\":\"compiler-artifact\",\"target\":{{\"kind\":[\"lib\"],\"name\":\"clax_core\"}},\"executable\":null}}'\n\
                 echo '{{\"reason\":\"compiler-artifact\",\"target\":{{\"kind\":[\"bin\"],\"name\":\"clax\"}},\"executable\":\"{}\"}}'\n\
                 echo '{{\"reason\":\"build-finished\",\"success\":true}}'\n",
                r.wrapper().display()
            ),
        );
        let sentinel = r.p("sentinel");
        std::fs::create_dir_all(sentinel.join(".claude/plugins")).unwrap();
        std::fs::write(sentinel.join(".claude/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(sentinel.join(".codex")).unwrap();
        std::fs::write(sentinel.join(".codex/config.toml"), "# real\n").unwrap();
        std::fs::create_dir_all(sentinel.join(".clax")).unwrap();
        std::fs::write(sentinel.join(".clax/marker"), "real").unwrap();
        std::fs::create_dir_all(sentinel.join(".pi/agent")).unwrap();
        std::fs::write(sentinel.join(".pi/agent/settings.json"), "{}").unwrap();
        // The scratch base names the previous name, as a path may.
        std::fs::create_dir_all(r.tmp()).unwrap();
        r
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn wrapper(&self) -> PathBuf {
        self.p("claxbin/clax")
    }
    /// The wrapper's physical path, as the script names the build under test.
    fn wrapper_phys(&self) -> String {
        std::fs::canonicalize(self.wrapper())
            .unwrap()
            .display()
            .to_string()
    }
    fn tmp(&self) -> PathBuf {
        self.p(&format!("tmp/{OLD}-scratch"))
    }
    /// The script with HOME the sentinel, TMPDIR `tmpdir`, the hostile
    /// variables set, CLAX_BIN the wrapper, and `extra` on top (an empty
    /// value removes the variable).
    fn run(&self, tmpdir: &Path, extra: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = Command::new("bash");
        cmd.arg(format!("{REPO}/scripts/verify-harnesses.sh"))
            .env_clear()
            .env("HOME", self.p("sentinel"))
            .env("TMPDIR", tmpdir)
            .env("TERM", "xterm")
            .env("LANG", "en_US.UTF-8")
            .env("CLAX_BIN", self.wrapper())
            .env(
                "PATH",
                format!(
                    "{}:{}:/usr/bin:/bin",
                    self.p("fakebin").display(),
                    self.p("decoybin").display()
                ),
            );
        for (k, v) in HOSTILE {
            cmd.env(k, v);
        }
        for (k, v) in extra {
            if v.is_empty() {
                cmd.env_remove(k);
            } else {
                cmd.env(k, v);
            }
        }
        cmd.output().unwrap()
    }
    fn lines(&self, rel: &str) -> Vec<String> {
        std::fs::read_to_string(self.p(rel))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
    fn calls(&self) -> Vec<Call> {
        self.lines("calls.log")
            .iter()
            .map(|l| {
                let mut parts = l.split('\t');
                let cmd = parts.next().unwrap().to_string();
                let f = parts
                    .map(|kv| {
                        let (k, v) = kv.split_once('=').unwrap();
                        (k.to_string(), v.to_string())
                    })
                    .collect();
                Call { cmd, f }
            })
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

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// The scratch root the script printed.
fn root_of(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("scratch root: "))
        .and_then(|l| l.split(" (").next())
        .unwrap_or_else(|| panic!("no scratch root in:\n{stdout}"))
        .to_string()
}

fn has_line(stdout: &str, status: &str, want: &str) -> bool {
    stdout
        .lines()
        .any(|l| l.starts_with(status) && l.contains(want))
}

/// Every harness call ran in the scratch HOME with only the allowed
/// variables (plus `also`), the build under test first on PATH, the
/// scratch homes and TMPDIR, and a scratch port; and the last Clax command
/// was `clax stop` for the scratch home.
fn assert_scratch_only(r: &Run, root: &str, also: &[&str]) {
    let calls = r.calls();
    assert!(calls.len() > 10, "{} calls", calls.len());
    let sentinel = r.p("sentinel").display().to_string();
    let wrapper = r.wrapper_phys();
    let allowed: BTreeSet<&str> = ALLOWED.iter().chain(also).copied().collect();
    for c in &calls {
        let at = |k: &str| c.f.get(k).unwrap_or_else(|| panic!("no {k} in {}", c.cmd));
        assert_eq!(at("PWD"), &format!("{root}/home"), "cwd of {}", c.cmd);
        assert_eq!(at("WHICH"), &wrapper, "the clax PATH finds for {}", c.cmd);
        let port: u32 = at("PORT")
            .parse()
            .unwrap_or_else(|_| panic!("port {:?} for {}", at("PORT"), c.cmd));
        assert!(
            (20000..50000).contains(&port) && port != 7480 && port != 7481,
            "port {port} for {}",
            c.cmd
        );
        let names: BTreeSet<&str> = at("ENV").split(',').filter(|n| !n.is_empty()).collect();
        assert_eq!(names, allowed, "environment of {}", c.cmd);
        assert_eq!(at("TMPDIR"), &format!("{root}/tmp"), "TMPDIR of {}", c.cmd);
        assert!(
            [wrapper.as_str(), env!("CARGO_BIN_EXE_clax")].contains(&at("CLAX_BIN").as_str()),
            "CLAX_BIN={} in {}",
            at("CLAX_BIN"),
            c.cmd
        );
        for k in [
            "HOME",
            "CLAX_HOME",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
        ] {
            let v = at(k);
            assert!(
                v.starts_with(&format!("{root}/home")),
                "{k}={v} in {}",
                c.cmd
            );
            assert!(!v.starts_with(&sentinel), "{k}={v} in {}", c.cmd);
        }
    }
    let clax = r.lines("clax-calls.log");
    let want_home = format!("CLAX_HOME={root}/home/.clax");
    for l in &clax {
        assert!(l.contains(&want_home), "{l}");
    }
    assert!(
        clax.last().is_some_and(|l| l.starts_with("clax stop\t")),
        "the last Clax command is `clax stop`: {clax:#?}"
    );
}

#[test]
fn the_script_passes_against_fakes_and_never_touches_the_real_home() {
    let r = Run::new();
    let before = snapshot(&r.p("sentinel"));
    let out = r.run(&r.tmp(), &[]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(!stdout.contains("\nFAIL"), "{stdout}");
    for want in [
        "init exits 0",
        "claude: seed registers the previous name",
        "codex: seed registers the previous name",
        "pi: seed registers the previous name",
        "claude: init removes the previous name's registration",
        "codex: init removes the previous name's registration",
        "codex: init keeps other config.toml content",
        "pi: the deleted previous-name checkout is left and named",
        "pi: the named `pi remove` clears it",
        "claude: init again replaces the cached copy",
        "pi: one entry after init again",
        "codex: doctor plugin passes",
        "uninit --agent pi keeps the copy the others use",
        "uninit removes the marketplace copy",
    ] {
        assert!(
            has_line(&stdout, "PASS", want),
            "no PASS for {want:?}:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("SKIPPED  pi: a session loads the extension"),
        "{stdout}"
    );
    // The build under test is named at the top and above the table.
    let header = format!(
        "build under test: {} (clax {}",
        r.wrapper_phys(),
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(stdout.matches(&header).count(), 2, "{stdout}");

    let root = root_of(&stdout);
    assert_scratch_only(&r, &root, &[]);
    assert!(!Path::new(&root).exists(), "the scratch root is removed");
    assert_eq!(
        snapshot(&r.p("sentinel")),
        before,
        "the sentinel HOME is unchanged"
    );
}

#[test]
fn without_clax_bin_the_script_builds_first_and_tests_that_build() {
    let r = Run::new();
    // A stale build where a lookup by path would find it.
    let target = r.p("target");
    for profile in ["release", "debug"] {
        write_exe(
            &target.join(profile).join("clax"),
            "#!/bin/sh\necho stale\nexit 1\n",
        );
    }
    let target_s = target.display().to_string();
    let out = r.run(
        &r.tmp(),
        &[("CLAX_BIN", ""), ("CARGO_TARGET_DIR", &target_s)],
    );
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    let cargo = r.lines("cargo.log");
    assert_eq!(cargo.len(), 1, "{cargo:#?}");
    let repo = std::fs::canonicalize(REPO).unwrap();
    assert_eq!(
        cargo[0],
        format!(
            "cargo build -p clax-cli --bin clax --message-format=json-render-diagnostics\tPWD={}\tHOME={}\tCARGO_TARGET_DIR={target_s}",
            repo.display(),
            r.p("sentinel").display()
        ),
        "cargo builds before HOME moves, with the caller's target directory"
    );
    assert!(
        stdout.contains(&format!(
            "build under test: {} (clax {}; built from ",
            r.wrapper_phys(),
            env!("CARGO_PKG_VERSION")
        )),
        "{stdout}"
    );
    assert_scratch_only(&r, &root_of(&stdout), &[]);
}

#[test]
fn the_opt_in_pi_session_runs_without_built_in_tools_or_persistence() {
    let r = Run::new();
    let out = r.run(&r.tmp(), &[("VERIFY_PI_SESSION", "1")]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(
        has_line(&stdout, "PASS", "pi: a session loads the extension"),
        "{stdout}"
    );
    let calls = r.calls();
    let session: Vec<&Call> = calls.iter().filter(|c| c.cmd.contains(" -p ")).collect();
    assert_eq!(
        session.len(),
        1,
        "{:#?}",
        calls.iter().map(|c| &c.cmd).collect::<Vec<_>>()
    );
    let args: Vec<&str> = session[0].cmd.split(' ').collect();
    for flag in [
        "--no-builtin-tools",
        "--no-session",
        "--offline",
        "--no-context-files",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
    ] {
        assert!(args.contains(&flag), "{flag} missing: {}", session[0].cmd);
    }
    let tools = args.iter().position(|a| *a == "--tools").expect("--tools");
    assert_eq!(args[tools + 1], "clax_status");
    // Only the session gets the provider key.
    let root = root_of(&stdout);
    for c in &calls {
        let has_key = c.f["ENV"].split(',').any(|n| n == "ANTHROPIC_API_KEY");
        assert_eq!(has_key, c.cmd.contains(" -p "), "{}", c.cmd);
    }
    let others: Vec<String> = r
        .lines("calls.log")
        .into_iter()
        .filter(|l| !l.contains(" -p "))
        .collect();
    std::fs::write(r.p("calls.log"), others.join("\n") + "\n").unwrap();
    assert_scratch_only(&r, &root, &[]);
}

#[test]
fn a_hung_harness_cli_fails_by_timeout_and_cleanup_still_runs() {
    let r = Run::new();
    std::fs::write(r.p("hang-codex"), "").unwrap();
    let before = snapshot(&r.p("sentinel"));
    let out = r.run(&r.tmp(), &[("VERIFY_TIMEOUT", "1")]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{stderr}");
    for want in ["codex: seed registers the previous name", "init exits 0"] {
        assert!(
            stdout.lines().any(|l| l.starts_with("FAIL")
                && l.contains(want)
                && l.contains("(timeout after 1s)")),
            "no timeout FAIL for {want:?}:\n{stdout}"
        );
    }
    // Every hung call was killed.
    let pids = r.lines("hang-pids");
    assert!(!pids.is_empty());
    for pid in &pids {
        let alive = Command::new("kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success();
        assert!(!alive, "hung fake {pid} still running");
    }
    let root = root_of(&stdout);
    assert!(!Path::new(&root).exists(), "the scratch root is removed");
    assert!(
        r.lines("clax-calls.log")
            .last()
            .is_some_and(|l| l.starts_with("clax stop\t")),
        "clax stop still runs"
    );
    assert_eq!(snapshot(&r.p("sentinel")), before);
}

/// Exit 2 with `why` on stderr, no harness or Clax call, no cargo build,
/// and the sentinel unchanged.
fn assert_refused(
    r: &Run,
    out: &std::process::Output,
    why: &str,
    before: &BTreeMap<PathBuf, String>,
) {
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains(why), "{stderr}");
    assert!(
        r.lines("calls.log").is_empty(),
        "{:#?}",
        r.lines("calls.log")
    );
    assert!(r.lines("clax-calls.log").is_empty());
    assert!(r.lines("cargo.log").is_empty());
    assert_eq!(&snapshot(&r.p("sentinel")), before);
}

#[test]
fn the_script_refuses_a_scratch_root_inside_the_real_home() {
    let r = Run::new();
    std::fs::create_dir_all(r.p("sentinel/tmp")).unwrap();
    let before = snapshot(&r.p("sentinel"));
    let out = r.run(&r.p("sentinel/tmp"), &[("CLAX_BIN", "")]);
    assert_refused(&r, &out, "inside your real HOME", &before);

    // The same directory, named with other letter case, on a file system
    // that ignores case.
    let upper = r.p("SENTINEL/tmp");
    if upper.exists() {
        let out = r.run(&upper, &[]);
        assert_refused(&r, &out, "inside your real HOME", &before);
    }
}

#[test]
fn the_script_refuses_home_at_the_root() {
    let r = Run::new();
    let before = snapshot(&r.p("sentinel"));
    let out = r.run(&r.tmp(), &[("HOME", "/")]);
    assert_refused(&r, &out, "HOME is /", &before);
}
