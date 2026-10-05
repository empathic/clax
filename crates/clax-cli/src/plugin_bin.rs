//! Which clax the plugins run, and the `bin` setting that names one.
//!
//! The plugins' wrapper (`scripts/ensure-clax.sh`, copied into each plugin)
//! runs, in order: `$CLAX_BIN`; the `bin` setting in the home's
//! `config.toml`; the managed install of the release the plugin pins,
//! `<home>/bin/<version>/clax`, downloading it when it is missing. [`resolve`]
//! answers the same question without downloading, for `clax bin` and
//! `clax doctor --agent`. [`set`] and [`clear`] edit the setting, writing
//! only the one-line form the wrapper reads ([`clax_core::config::bin_line`]).

use clax_core::Home;
use clax_core::config::{FILE, bin_line, bin_path_ok};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The wrapper this binary was built with; the plugins carry copies.
pub const LAUNCHER: &str = include_str!("../../../scripts/ensure-clax.sh");

/// How long a `clax --version` may take.
pub const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The first line `exe --version` prints, or None when it cannot run or
/// takes longer than [`VERSION_TIMEOUT`] (it is then killed).
pub fn version_line(exe: &Path) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    out.lines().next().map(str::to_string)
}

/// `exe`'s version line when it names clax.
fn clax_version(exe: &Path) -> Option<String> {
    version_line(exe).filter(|l| l.starts_with("clax "))
}

/// The release a wrapper's text pins (`PINNED_VERSION="X.Y.Z"`), or None
/// when it pins none.
pub fn pinned_version(launcher: &str) -> Option<String> {
    launcher
        .lines()
        .find_map(|l| l.strip_prefix("PINNED_VERSION=\"")?.strip_suffix('"'))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// True when the line assigns the top-level key `bin`, in any TOML spelling.
fn assigns_bin(line: &str) -> bool {
    let l = line.trim_start();
    ["bin", "\"bin\"", "'bin'"].iter().any(|k| {
        l.strip_prefix(k)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

/// True when the line starts a table (`[x]` or `[[x]]`).
fn starts_table(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// The `bin` setting as the wrapper reads it from config.toml's text: None
/// when no line before the first table assigns `bin`; the path when exactly
/// one does, as [`bin_line`] writes it; otherwise the offending line.
pub fn wrapper_setting(text: &str) -> Option<Result<String, String>> {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|l| !starts_table(l))
        .filter(|l| assigns_bin(l))
        .collect();
    let first = *lines.first()?;
    let path = first
        .strip_prefix("bin = \"")
        .and_then(|r| r.strip_suffix('"'))
        .filter(|p| bin_path_ok(p) && lines.len() == 1);
    Some(match path {
        Some(p) => Ok(p.to_string()),
        None if lines.len() > 1 => Err(format!("{first} (and {} more)", lines.len() - 1)),
        None => Err(first.to_string()),
    })
}

/// Where the managed install of `version` lives in `home`.
pub fn managed_dir(home: &Home, version: &str) -> PathBuf {
    home.root().join("bin").join(version)
}

/// The sha256 of the file at `p`, in lowercase hex.
fn sha256_hex(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(p).ok()?;
    Some(
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Why the managed install in `dir` is not valid for `version`, or None when
/// it is: as the wrapper checks it, the binary's sha256 against the record
/// beside it first, then its `--version`.
pub fn managed_problem(dir: &Path, version: &str) -> Option<String> {
    let bin = dir.join("clax");
    if !bin.is_file() {
        return Some("not installed".into());
    }
    let recorded = std::fs::read_to_string(dir.join("clax.sha256")).unwrap_or_default();
    let recorded = recorded.trim_end_matches('\n');
    if recorded.len() != 64 || !recorded.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Some("its sha256 record is missing or malformed".into());
    }
    if sha256_hex(&bin).as_deref() != Some(recorded) {
        return Some("its sha256 does not match the one recorded at install".into());
    }
    match version_line(&bin) {
        Some(l) if l == format!("clax {version}") => None,
        Some(l) => Some(format!("`--version` printed '{l}', not 'clax {version}'")),
        None => Some("`--version` did not run".into()),
    }
}

/// What the plugins' wrapper would run.
#[derive(Debug)]
pub struct Resolution {
    /// The binary, when the wrapper would run one (or install one: see
    /// `pending`).
    pub path: Option<PathBuf>,
    /// Its `--version` line, when it runs.
    pub version: Option<String>,
    /// `env` (CLAX_BIN), `config` (the `bin` setting), `managed` (the pinned
    /// release) or `none`.
    pub source: &'static str,
    /// Why this binary, or why none.
    pub why: String,
    /// The managed install is missing or invalid; the wrapper's next MCP
    /// start or CLI run installs it.
    pub pending: bool,
    /// False when the wrapper would fail: a CLAX_BIN or `bin` setting that
    /// is not a usable clax, or no binary named and no release pinned.
    pub ok: bool,
}

impl Resolution {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "version": self.version,
            "source": self.source,
            "why": self.why,
            "pending_install": self.pending,
            "ok": self.ok,
        })
    }

    /// One line: the binary, its version and why it is the one.
    pub fn line(&self) -> String {
        match (&self.path, self.ok) {
            (Some(p), true) => format!(
                "{} ({}), {}",
                p.display(),
                self.version.as_deref().unwrap_or("not installed yet"),
                self.why
            ),
            _ => format!("nothing: {}", self.why),
        }
    }
}

/// What the wrapper would run for `home`, given `CLAX_BIN` (`clax_bin`) and
/// the release its copy pins (`pin`). Never downloads.
pub fn resolve(home: &Home, clax_bin: Option<&str>, pin: Option<&str>) -> Resolution {
    let res = |path: Option<PathBuf>, version, source, why: String, ok| Resolution {
        path,
        version,
        source,
        why,
        pending: false,
        ok,
    };
    if let Some(b) = clax_bin.filter(|b| !b.is_empty()) {
        let p = PathBuf::from(b);
        return match clax_version(&p) {
            Some(v) => res(Some(p), Some(v), "env", "from CLAX_BIN".into(), true),
            None => res(
                None,
                None,
                "env",
                format!("CLAX_BIN is set to '{b}', which is not a usable clax binary; unset it, or point it at a clax binary"),
                false,
            ),
        };
    }
    let config = home.root().join(FILE);
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    match wrapper_setting(&text) {
        Some(Ok(b)) => {
            let p = PathBuf::from(&b);
            return match clax_version(&p) {
                Some(v) => res(
                    Some(p),
                    Some(v),
                    "config",
                    format!("from the bin setting in {}", config.display()),
                    true,
                ),
                None => res(
                    None,
                    None,
                    "config",
                    format!(
                        "{} sets bin = \"{b}\", which is not a usable clax binary; run `clax bin set <path>` with a clax binary, or `clax bin clear`",
                        config.display()
                    ),
                    false,
                ),
            };
        }
        Some(Err(line)) => {
            return res(
                None,
                None,
                "config",
                format!(
                    "{} sets bin in a form the plugins do not read: {line}; run `clax bin set <path>` or `clax bin clear`",
                    config.display()
                ),
                false,
            );
        }
        None => {}
    }
    let Some(pin) = pin else {
        return res(
            None,
            None,
            "none",
            "the plugin pins no Clax release, and neither CLAX_BIN nor the bin setting names a binary; run `clax bin set --this`".into(),
            false,
        );
    };
    let dir = managed_dir(home, pin);
    let bin = dir.join("clax");
    match managed_problem(&dir, pin) {
        None => res(
            Some(bin),
            Some(format!("clax {pin}")),
            "managed",
            format!("the managed install of clax {pin}, the release the plugin pins"),
            true,
        ),
        Some(problem) => Resolution {
            path: Some(bin),
            version: None,
            source: "managed",
            why: format!(
                "the managed install of clax {pin}, the release the plugin pins: {problem}; the plugin's MCP server downloads and installs it when it next starts"
            ),
            pending: true,
            ok: true,
        },
    }
}

/// config.toml's text with every top-level `bin` line removed, and with
/// `line` first when given. Refuses a file that does not parse, and an edit
/// whose result does not hold exactly the wanted `bin`.
fn edited(path: &Path, text: &str, line: Option<(&str, &str)>) -> anyhow::Result<String> {
    let parse = |t: &str| t.parse::<toml::Table>();
    if let Err(e) = parse(text) {
        anyhow::bail!("{}: {e}; fix it before changing the bin setting", path.display());
    }
    let mut out = String::new();
    if let Some((l, _)) = line {
        out.push_str(l);
        out.push('\n');
    }
    let mut in_tables = false;
    for l in text.split_inclusive('\n') {
        in_tables = in_tables || starts_table(l);
        if !in_tables && assigns_bin(l) {
            continue;
        }
        out.push_str(l);
    }
    let got = parse(&out).ok().map(|t| t.get("bin").cloned());
    let want = line.map(|(_, p)| toml::Value::String(p.to_string()));
    if got != Some(want) {
        anyhow::bail!(
            "{} sets bin in a form `clax bin` cannot rewrite (a value over several lines?); edit or remove that line by hand",
            path.display()
        );
    }
    Ok(out)
}

/// Writes `text` to `path` through a temporary file and a rename.
fn write_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Sets the `bin` setting to `path`, which must be absolute, writable as
/// [`bin_line`] ([`bin_path_ok`]) and a usable clax. Returns its version line.
pub fn set(home: &Home, path: &Path) -> anyhow::Result<String> {
    let Some(s) = path.to_str() else {
        anyhow::bail!("{} is not valid UTF-8, so it cannot be the bin setting", path.display());
    };
    if !s.starts_with('/') {
        anyhow::bail!("{s} is not an absolute path; give the absolute path of a clax binary");
    }
    if !bin_path_ok(s) {
        anyhow::bail!(
            "{s} holds a quote, backslash or control character, which the plugins cannot read from config.toml; use a path without them, or set CLAX_BIN"
        );
    }
    let Some(version) = clax_version(path) else {
        anyhow::bail!("{s} is not a usable clax binary (its --version does not name clax)");
    };
    home.ensure_dirs()?;
    let config = home.root().join(FILE);
    let text = match std::fs::read_to_string(&config) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => anyhow::bail!("{}: {e}", config.display()),
    };
    let out = edited(&config, &text, Some((&bin_line(s), s)))?;
    write_atomically(&config, &out)?;
    Ok(version)
}

/// Removes the `bin` setting. True when there was one.
pub fn clear(home: &Home) -> anyhow::Result<bool> {
    let config = home.root().join(FILE);
    let text = match std::fs::read_to_string(&config) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => anyhow::bail!("{}: {e}", config.display()),
    };
    let out = edited(&config, &text, None)?;
    if out == text {
        return Ok(false);
    }
    write_atomically(&config, &out)?;
    Ok(true)
}

/// The path the `bin` setting names, as the wrapper reads it.
pub fn current(home: &Home) -> Option<String> {
    let text = std::fs::read_to_string(home.root().join(FILE)).ok()?;
    wrapper_setting(&text)?.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_clax(dir: &Path, line: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("clax");
        std::fs::write(&p, format!("#!/bin/sh\necho '{line}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn home(t: &tempfile::TempDir) -> Home {
        Home::at(t.path().join("ch"))
    }

    #[test]
    fn the_pin_is_read_from_the_wrapper() {
        assert_eq!(pinned_version("a\nPINNED_VERSION=\"\"\n"), None);
        assert_eq!(
            pinned_version("a\nPINNED_VERSION=\"0.3.0\"\n").as_deref(),
            Some("0.3.0")
        );
        assert_eq!(pinned_version("nothing"), None);
        // The wrapper this binary carries has the line.
        assert!(LAUNCHER.lines().any(|l| l.starts_with("PINNED_VERSION=\"")));
    }

    #[test]
    fn the_wrapper_reads_only_the_one_line_form_before_the_tables() {
        assert_eq!(
            wrapper_setting("# c\nbin = \"/a/clax\"\n[serve]\nport = 1\n"),
            Some(Ok("/a/clax".into()))
        );
        assert_eq!(wrapper_setting("[serve]\nbin = \"/a/clax\"\n"), None);
        assert_eq!(wrapper_setting("binary = 1\n"), None);
        for bad in [
            "bin='/a'\n",
            "bin=\"/a\"\n",
            "bin = \"a\"\n",
            "\"bin\" = \"/a\"\n",
            "bin = \"/a\" # x\n",
            "  bin = \"/a\"\n",
        ] {
            assert!(matches!(wrapper_setting(bad), Some(Err(_))), "{bad}");
        }
        assert_eq!(
            wrapper_setting("bin = \"/a\"\nbin = \"/b\"\n"),
            Some(Err("bin = \"/a\" (and 1 more)".into()))
        );
    }

    #[test]
    fn set_writes_the_line_first_and_keeps_the_rest_and_clear_removes_it() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let me = fake_clax(&t.path().join("me"), "clax 9.9.9");
        h.ensure_dirs().unwrap();
        let config = h.root().join(FILE);
        std::fs::write(&config, "# mine\nbin = \"/old\"\n\n[serve]\nport = 7481\n").unwrap();
        assert_eq!(set(&h, &me).unwrap(), "clax 9.9.9");
        let text = std::fs::read_to_string(&config).unwrap();
        assert_eq!(
            text,
            format!("bin = \"{}\"\n# mine\n\n[serve]\nport = 7481\n", me.display())
        );
        assert_eq!(current(&h), Some(me.display().to_string()));
        let c = clax_core::config::HomeConfig::load(h.root()).unwrap();
        assert_eq!(c.bin().unwrap(), Some(me.clone()));
        assert_eq!(c.serve_port().unwrap(), Some(7481));
        assert!(clear(&h).unwrap());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "# mine\n\n[serve]\nport = 7481\n"
        );
        assert!(!clear(&h).unwrap());
    }

    #[test]
    fn set_creates_the_home_and_refuses_what_the_wrapper_could_not_read() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let me = fake_clax(&t.path().join("me"), "clax 9.9.9");
        set(&h, &me).unwrap();
        assert!(h.root().join(FILE).is_file());
        let quoted = fake_clax(&t.path().join("a\"b"), "clax 9.9.9");
        let other = fake_clax(&t.path().join("other"), "other 1.0");
        for (p, why) in [
            (PathBuf::from("rel/clax"), "not an absolute path"),
            (quoted, "quote, backslash or control character"),
            (other, "not a usable clax"),
            (t.path().join("missing"), "not a usable clax"),
        ] {
            let e = set(&h, &p).unwrap_err().to_string();
            assert!(e.contains(why), "{}: {e}", p.display());
        }
        assert_eq!(current(&h), Some(me.display().to_string()));
    }

    #[test]
    fn set_refuses_a_config_that_does_not_parse_or_cannot_be_rewritten() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let me = fake_clax(&t.path().join("me"), "clax 9.9.9");
        h.ensure_dirs().unwrap();
        let config = h.root().join(FILE);
        std::fs::write(&config, "[serve\n").unwrap();
        assert!(set(&h, &me).unwrap_err().to_string().contains("fix it"));
        std::fs::write(&config, "bin = \"\"\"\n/x\n\"\"\"\n").unwrap();
        assert!(set(&h, &me).unwrap_err().to_string().contains("by hand"));
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "bin = \"\"\"\n/x\n\"\"\"\n"
        );
    }

    #[test]
    fn resolve_follows_the_wrappers_order() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let env = fake_clax(&t.path().join("env"), "clax 1.0.0");
        let cfg = fake_clax(&t.path().join("cfg"), "clax 2.0.0");
        let r = resolve(&h, None, None);
        assert!(!r.ok && r.source == "none" && r.why.contains("pins no Clax release"), "{r:?}");
        let r = resolve(&h, None, Some("3.0.0"));
        assert!(r.ok && r.pending && r.source == "managed", "{r:?}");
        assert!(r.why.contains("not installed"), "{r:?}");
        set(&h, &cfg).unwrap();
        let r = resolve(&h, None, Some("3.0.0"));
        assert_eq!((r.source, r.path.as_deref(), r.ok), ("config", Some(cfg.as_path()), true));
        let r = resolve(&h, Some(env.to_str().unwrap()), Some("3.0.0"));
        assert_eq!((r.source, r.version.as_deref()), ("env", Some("clax 1.0.0")));
        let r = resolve(&h, Some("/no/such/clax"), None);
        assert!(!r.ok && r.why.contains("CLAX_BIN is set to '/no/such/clax'"), "{r:?}");
        std::fs::remove_file(&cfg).unwrap();
        let r = resolve(&h, None, Some("3.0.0"));
        assert!(!r.ok && r.why.contains("not a usable clax binary"), "{r:?}");
    }

    #[test]
    fn a_managed_install_is_valid_only_with_a_matching_record_and_version() {
        let t = tempfile::tempdir().unwrap();
        let h = home(&t);
        let dir = managed_dir(&h, "3.0.0");
        let bin = fake_clax(&dir, "clax 3.0.0");
        assert!(managed_problem(&dir, "3.0.0").unwrap().contains("record"));
        std::fs::write(dir.join("clax.sha256"), format!("{}\n", sha256_hex(&bin).unwrap())).unwrap();
        assert_eq!(managed_problem(&dir, "3.0.0"), None);
        let r = resolve(&h, None, Some("3.0.0"));
        assert!(r.ok && !r.pending && r.path.as_deref() == Some(bin.as_path()), "{r:?}");
        assert!(managed_problem(&dir, "3.0.1").unwrap().contains("not 'clax 3.0.1'"));
        std::fs::write(&bin, "#!/bin/sh\necho 'clax 3.0.0'\n# changed\n").unwrap();
        assert!(managed_problem(&dir, "3.0.0").unwrap().contains("does not match"));
    }
}
