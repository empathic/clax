//! Every field of a home's configuration with its value and where it came
//! from ([`fields`]), what is wrong with each table ([`problems`]), and
//! writing file-backed fields back into `config.toml` ([`write`]) with
//! `toml_edit`, which keeps the file's comments, order and spelling.
//!
//! The fields are those that have defaults (`[serve]`, `[sample]`,
//! `[toolpath]`, `[questions]`, `[bot]`) and the top-level `bin`. `bin` is
//! listed but not editable here: `clax bin set` writes it, as the one line
//! the plugins' wrapper reads ([`super::bin_line`]). `[hosted]` is not
//! listed: a daemon with a `[hosted]` table serves no settings.

use super::{HomeConfig, Source, defaults};
use crate::{CoreError, Result};
use figment::value::Dict;
use serde::Serialize;
use std::path::Path;

/// One configuration field as the settings API shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field {
    /// Its key, dotted (`sample.models.quick`).
    pub key: String,
    /// Its value in effect: what the running daemon read when it started
    /// (JSON; null when unset and without a default).
    pub value: serde_json::Value,
    /// Where the value in effect came from.
    pub source: Source,
    /// The value the daemon takes when it next starts, when it differs from
    /// `value` (a write since it started, or a hand edit): absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<serde_json::Value>,
    /// Whether [`write`] may set it (it may not when the environment sets
    /// it, or for `bin`).
    pub editable: bool,
    /// Whether a change takes effect only when the daemon next starts.
    pub restart_required: bool,
    /// The variable that would set it (`CLAX_SAMPLE__MAX_TOKENS`); none for
    /// `bin`, which no variable sets.
    pub env: Option<String>,
    /// Why the value in effect is not the configuration's (the port the
    /// daemon listens on, chosen by `--port`, `CLAX_PORT` or the next free
    /// one): absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A table whose values do not check: what turns off until it is fixed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The table (`sample`), or `bin`.
    pub table: &'static str,
    /// The error, naming the file or the variable.
    pub message: String,
}

/// What [`write`] wrote.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Written {
    /// The keys set or removed, in the order of the changes given (a
    /// caller that collects them into a sorted map passes them sorted).
    pub keys: Vec<String>,
    /// Each key's value and source before and after (none of the fields
    /// holds a secret: `*_env` fields name a variable, never its value).
    pub changes: Vec<Change>,
    /// Whether any of them takes effect only when the daemon next starts.
    pub restart_required: bool,
}

/// One field [`write`] changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    pub key: String,
    /// The file's value before (or the default's), as JSON.
    pub old: serde_json::Value,
    pub old_source: Source,
    /// The value after, as JSON (the default's when the key was removed).
    pub new: serde_json::Value,
    pub new_source: Source,
}

/// The keys read on each use rather than when the daemon starts: a change
/// to one takes effect without a restart. Every reader today reads its
/// table when the daemon starts.
const LIVE_KEYS: &[&str] = &[];

/// The keys listed but never written here.
const READ_ONLY_KEYS: &[&str] = &["bin"];

/// The dotted keys of every leaf of `dict`, under `prefix`.
fn leaves(prefix: &str, dict: &Dict, out: &mut Vec<String>) {
    for (k, v) in dict {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v.as_dict() {
            Some(d) => leaves(&key, d, out),
            None => out.push(key),
        }
    }
}

/// Every field's key: `bin`, then the defaults' leaves in key order.
fn keys() -> Vec<String> {
    let mut out = vec!["bin".to_string()];
    leaves("", &defaults(), &mut out);
    out
}

/// The value at `key` in `cfg`, as JSON (null when unset).
fn json_at(cfg: &HomeConfig, key: &str) -> serde_json::Value {
    cfg.value(key)
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Every field of `cfg`, with its value and source.
pub fn fields(cfg: &HomeConfig) -> Vec<Field> {
    fields_in_effect(cfg, cfg)
}

/// Every field as the daemon that read `running` when it started has it,
/// with `pending` the value `current` (the configuration as it reads now)
/// holds when that differs. Whether a field is editable follows `current`.
pub fn fields_in_effect(running: &HomeConfig, current: &HomeConfig) -> Vec<Field> {
    keys()
        .into_iter()
        .map(|key| {
            let value = json_at(running, &key);
            let now = json_at(current, &key);
            let read_only = READ_ONLY_KEYS.contains(&key.as_str());
            Field {
                editable: !read_only && !matches!(current.source(&key), Source::Env(_)),
                restart_required: !LIVE_KEYS.contains(&key.as_str()),
                env: (!read_only).then(|| super::env_name(&key)),
                pending: (now != value).then_some(now),
                note: None,
                source: running.source(&key),
                key,
                value,
            }
        })
        .collect()
}

/// What is wrong with each table of `cfg` that does not check, as its
/// reader reports it.
pub fn problems(cfg: &HomeConfig) -> Vec<Problem> {
    let checks: [(&'static str, Result<()>); 6] = [
        ("bin", cfg.bin().map(drop)),
        ("serve", cfg.serve_port().map(drop)),
        ("sample", cfg.sample().map(drop)),
        ("toolpath", cfg.toolpath().map(drop)),
        ("questions", questions_check(cfg)),
        ("bot", cfg.bot().map(drop)),
    ];
    checks
        .into_iter()
        .filter_map(|(table, r)| {
            r.err().map(|e| Problem {
                table,
                message: e.to_string(),
            })
        })
        .collect()
}

/// `[questions]` as a write must leave it: a table whose
/// `terminal_after_s` is an integer from 0 to
/// [`MAX_TERMINAL_AFTER_S`](super::MAX_TERMINAL_AFTER_S). (The reader
/// itself clamps and logs instead.)
fn questions_check(cfg: &HomeConfig) -> Result<()> {
    let bad = |what: String| cfg.bad("questions", "[questions]", what);
    match cfg.value("questions") {
        Some(q) if q.as_dict().is_none() => {
            return Err(bad(format!("must be a table, not {}", super::kind_of(&q))));
        }
        _ => {}
    }
    let Some(v) = cfg.value("questions.terminal_after_s") else {
        return Ok(());
    };
    match super::as_int(&v) {
        Some(n) if (0..=super::MAX_TERMINAL_AFTER_S as i128).contains(&n) => Ok(()),
        _ => Err(bad(format!(
            "terminal_after_s must be an integer from 0 to {}, not {}",
            super::MAX_TERMINAL_AFTER_S,
            super::shown(&v)
        ))),
    }
}

/// The check a write to `table` must pass.
fn check_table(cfg: &HomeConfig, table: &str) -> Result<()> {
    match table {
        "serve" => cfg.serve_port().map(drop),
        "sample" => cfg.sample().map(drop),
        "toolpath" => cfg.toolpath().map(drop),
        "questions" => questions_check(cfg),
        "bot" => cfg.bot().map(drop),
        _ => Ok(()),
    }
}

/// Serializes writes from this process, so two never interleave their
/// read and rename.
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `value` as a TOML value: a string, boolean, integer or float.
fn toml_value(key: &str, value: &serde_json::Value) -> Result<toml_edit::Value> {
    use serde_json::Value as J;
    let bad = || {
        CoreError::invalid(
            "invalid_value",
            format!("{key}: a setting is a string, a boolean, an integer or a number, not {value}"),
        )
    };
    Ok(match value {
        J::String(s) => toml_edit::Value::from(s.as_str()),
        J::Bool(b) => toml_edit::Value::from(*b),
        J::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => toml_edit::Value::from(i),
            (None, Some(f)) if n.as_u64().is_none() => toml_edit::Value::from(f),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    })
}

/// Sets `key` (dotted) in `doc` to `value`, or removes it when `None`,
/// making the tables on the way (standard tables under standard ones,
/// inline under inline). A value replacing another keeps its comments.
fn apply(
    doc: &mut toml_edit::DocumentMut,
    key: &str,
    value: Option<toml_edit::Value>,
) -> Result<()> {
    use toml_edit::{InlineTable, Item, Table};
    let parts: Vec<&str> = key.split('.').collect();
    let (leaf, parents) = parts.split_last().expect("a key has a part");
    let not_table = |at: &str| {
        CoreError::invalid(
            "invalid_value",
            format!("{key}: config.toml's {at} is not a table, so it cannot hold {key}"),
        )
    };
    let mut item: &mut Item = doc.as_item_mut();
    for (i, seg) in parents.iter().enumerate() {
        let at = parts[..=i].join(".");
        let inline = item.is_inline_table();
        let t = item.as_table_like_mut().ok_or_else(|| not_table(&at))?;
        if t.get(seg).is_none() {
            if value.is_none() {
                return Ok(());
            }
            let new = if inline {
                Item::Value(toml_edit::Value::InlineTable(InlineTable::new()))
            } else {
                let mut tb = Table::new();
                tb.set_implicit(true);
                Item::Table(tb)
            };
            t.insert(seg, new);
        }
        item = t.get_mut(seg).expect("inserted above");
        if !item.is_table_like() {
            return Err(not_table(&at));
        }
    }
    let t = item
        .as_table_like_mut()
        .ok_or_else(|| not_table(&parents.join(".")))?;
    match value {
        None => {
            t.remove(leaf);
        }
        Some(mut v) => {
            if let Some(old) = t.get(leaf).and_then(|i| i.as_value()) {
                *v.decor_mut() = old.decor().clone();
            }
            t.insert(leaf, Item::Value(v));
        }
    }
    Ok(())
}

/// Sets each `(key, value)` of `changes` in `<home_root>/config.toml` (a
/// null value removes the key, so its default applies), keeping the rest
/// of the file as it is: comments, order, the `bin` line. `env` is the
/// environment the configuration is read under. Nothing is written unless
/// every change is allowed and every table it touches still checks; the
/// file is replaced in one rename.
///
/// # Errors
/// - `Invalid { code: "unknown_setting" }` for a key [`fields`] does not list;
/// - `not_editable` for `bin` (`clax bin set` writes it);
/// - `set_by_env` for a key the environment sets, naming the variable;
/// - `invalid_value` for a value that is not a string, boolean or number,
///   or that leaves its table failing its check (the message is the
///   check's);
/// - `bad_config` when the file does not read or parse;
/// - `Io` when the file cannot be written.
pub fn write(
    home_root: &Path,
    env: &[(String, String)],
    changes: &[(String, serde_json::Value)],
) -> Result<Written> {
    let _held = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = home_root.join(super::FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(super::bad_file(&path, &e)),
    };
    let cfg = HomeConfig::from_text(path.clone(), &text, env.iter().cloned())?;
    let known = fields(&cfg);
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| super::bad_file(&path, &e))?;
    let mut tables: Vec<&str> = Vec::new();
    let mut restart_required = false;
    for (key, value) in changes {
        let Some(field) = known.iter().find(|f| &f.key == key) else {
            return Err(CoreError::invalid(
                "unknown_setting",
                format!("{key} is not a setting"),
            ));
        };
        if let Source::Env(name) = &field.source {
            return Err(CoreError::invalid(
                "set_by_env",
                format!(
                    "{key} is set by the environment variable {name}; unset it to change {key} here"
                ),
            ));
        }
        if !field.editable {
            return Err(CoreError::invalid(
                "not_editable",
                format!("{key} is set with `clax bin set`"),
            ));
        }
        let v = match value {
            serde_json::Value::Null => None,
            v => Some(toml_value(key, v)?),
        };
        apply(&mut doc, key, v)?;
        tables.push(key.split('.').next().unwrap_or(key));
        restart_required |= field.restart_required;
    }
    let new_text = doc.to_string();
    let after = HomeConfig::from_text(path.clone(), &new_text, env.iter().cloned())?;
    for table in tables {
        check_table(&after, table).map_err(|e| match e {
            CoreError::Invalid { message, .. } => CoreError::invalid("invalid_value", message),
            e => e,
        })?;
    }
    if new_text != text {
        write_file(&path, &new_text)?;
    }
    Ok(Written {
        keys: changes.iter().map(|(k, _)| k.clone()).collect(),
        changes: changes
            .iter()
            .map(|(key, _)| Change {
                key: key.clone(),
                old: json_at(&cfg, key),
                old_source: cfg.source(key),
                new: json_at(&after, key),
                new_source: after.source(key),
            })
            .collect(),
        restart_required,
    })
}

/// Writes `text` to `path` (a home's `config.toml`; `clax bin set` and
/// [`write`] both write through here) through a file beside it and one
/// rename. When `path` is a symbolic link, the file it points to is
/// written (the link stays), so a config.toml kept elsewhere and linked
/// into the home keeps working. The new file is made with the old one's
/// mode (0600 when there was none) before any byte is written, and synced
/// before the rename.
///
/// # Errors
/// `Invalid { code: "config_symlink_dangling" }` when `path` is a link to
/// nothing: writing a new file there would leave the link's target
/// unmade, or replace the link. `Io` otherwise.
pub fn write_file(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => match std::fs::canonicalize(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::invalid(
                    "config_symlink_dangling",
                    format!(
                        "{} is a symbolic link to {}, which does not exist; make that file or remove the link",
                        path.display(),
                        std::fs::read_link(path)
                            .map(|t| t.display().to_string())
                            .unwrap_or_default()
                    ),
                ));
            }
            Err(e) => return Err(e.into()),
        },
        _ => path.to_path_buf(),
    };
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| super::FILE.to_string());
    let tmp = dir.join(format!(".{name}.{}.tmp", ulid::Ulid::new()));
    let done = (|| {
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let mode = std::fs::metadata(&target).map_or(0o600, |m| m.mode() & 0o7777);
            open.mode(mode);
        }
        let mut f = open.open(&tmp)?;
        // The mode given at creation is masked by the umask: set it exactly.
        if let Ok(meta) = std::fs::metadata(&target) {
            f.set_permissions(meta.permissions())?;
        }
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target)
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done.map_err(CoreError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn home(text: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(super::super::FILE), text).unwrap();
        dir
    }

    fn read(dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(dir.path().join(super::super::FILE)).unwrap()
    }

    fn field<'a>(fs: &'a [Field], key: &str) -> &'a Field {
        fs.iter().find(|f| f.key == key).unwrap()
    }

    #[test]
    fn every_field_is_listed_with_its_value_and_source() {
        let dir = home("bin = \"/opt/clax\"\n[sample]\nmax_tokens = 9\n");
        let c =
            HomeConfig::load_with_env(dir.path(), env(&[("CLAX_BOT__MAX_STEPS", "7")])).unwrap();
        let fs = fields(&c);
        let port = field(&fs, "serve.port");
        assert_eq!(
            (
                &port.value,
                &port.source,
                port.editable,
                port.restart_required
            ),
            (&json!(7480), &Source::Default, true, true)
        );
        assert_eq!(port.env.as_deref(), Some("CLAX_SERVE__PORT"));
        let mt = field(&fs, "sample.max_tokens");
        assert_eq!(
            (&mt.value, &mt.source, mt.editable),
            (&json!(9), &Source::File, true)
        );
        let steps = field(&fs, "bot.max_steps");
        assert_eq!(
            (&steps.value, &steps.source, steps.editable),
            (&json!(7), &Source::Env("CLAX_BOT__MAX_STEPS".into()), false)
        );
        assert_eq!(
            serde_json::to_value(steps).unwrap()["source"],
            json!("env:CLAX_BOT__MAX_STEPS")
        );
        let bin = field(&fs, "bin");
        assert_eq!(
            (&bin.value, &bin.source, bin.editable, &bin.env),
            (&json!("/opt/clax"), &Source::File, false, &None)
        );
        assert_eq!(field(&fs, "sample.daily_call_cap").value, json!(null));
        for key in [
            "bot.models.default",
            "bot.script.cpu_ms",
            "toolpath.journal",
            "questions.terminal_after_s",
            "sample.models.quick",
        ] {
            assert!(fs.iter().any(|f| f.key == key), "{key}");
        }
        assert!(!fs.iter().any(|f| f.key.starts_with("hosted")));
    }

    #[test]
    fn problems_name_each_table_that_does_not_check() {
        let c = HomeConfig::from_text(
            "/h/config.toml".into(),
            "[sample]\nprovider = \"x\"\n[bot]\nmax_steps = 0\n",
            [],
        )
        .unwrap();
        let ps = problems(&c);
        assert_eq!(
            ps.iter().map(|p| p.table).collect::<Vec<_>>(),
            ["sample", "bot"]
        );
        assert!(
            ps[0].message.contains("/h/config.toml"),
            "{}",
            ps[0].message
        );
    }

    #[test]
    fn a_write_keeps_comments_order_and_the_bin_line() {
        let text = "bin = \"/opt/clax/bin/clax\"\n\n# The dev daemon.\n[serve]\nport = 7481 # dev\n\n[sample]\n# Which provider.\nprovider = \"stub\"\n";
        let dir = home(text);
        let w = write(
            dir.path(),
            &[],
            &[
                ("serve.port".into(), json!(7490)),
                ("sample.max_tokens".into(), json!(8000)),
                ("bot.models.default".into(), json!("claude-x")),
                ("toolpath.journal".into(), json!(false)),
            ],
        )
        .unwrap();
        assert!(w.restart_required);
        assert_eq!(w.keys.len(), 4);
        let out = read(&dir);
        assert!(
            out.starts_with(
                "bin = \"/opt/clax/bin/clax\"\n\n# The dev daemon.\n[serve]\nport = 7490 # dev\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("# Which provider.\nprovider = \"stub\"\nmax_tokens = 8000\n"),
            "{out}"
        );
        assert!(
            out.contains("[bot.models]\ndefault = \"claude-x\"\n"),
            "{out}"
        );
        assert!(!out.contains("[bot]\n"), "{out}");
        let c = HomeConfig::load_with_env(dir.path(), []).unwrap();
        assert_eq!(c.serve_port().unwrap(), Some(7490));
        assert_eq!(c.bin().unwrap(), Some("/opt/clax/bin/clax".into()));
        assert_eq!(c.bot().unwrap().models.default, "claude-x");
        assert!(!c.toolpath().unwrap().journal);
        // Null removes the key: its default applies again.
        write(dir.path(), &[], &[("serve.port".into(), json!(null))]).unwrap();
        let c = HomeConfig::load_with_env(dir.path(), []).unwrap();
        assert_eq!(c.serve_port().unwrap(), None);
        assert!(read(&dir).contains("# The dev daemon.\n[serve]\n"));
    }

    #[test]
    fn a_write_makes_the_file_and_fills_inline_tables() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &[],
            &[("sample.models.quick".into(), json!("q"))],
        )
        .unwrap();
        assert_eq!(read(&dir), "[sample.models]\nquick = \"q\"\n");
        let dir = home("bot = { models = { quick = \"a\" } }\n");
        write(
            dir.path(),
            &[],
            &[("bot.models.default".into(), json!("d"))],
        )
        .unwrap();
        let c = HomeConfig::load_with_env(dir.path(), []).unwrap();
        let b = c.bot().unwrap();
        assert_eq!(
            (b.models.quick.as_str(), b.models.default.as_str()),
            ("a", "d")
        );
    }

    #[test]
    fn a_refused_write_changes_nothing() {
        let text = "# keep\n[serve]\nport = 7481\n";
        let dir = home(text);
        let e = |changes: &[(String, serde_json::Value)], vars: &[(&str, &str)]| match write(
            dir.path(),
            &env(vars),
            changes,
        )
        .unwrap_err()
        {
            CoreError::Invalid { code, message } => (code, message),
            e => panic!("{e}"),
        };
        let (code, m) = e(
            &[("serve.port".into(), json!(1))],
            &[("CLAX_SERVE__PORT", "7490")],
        );
        assert_eq!(code, "set_by_env");
        assert!(m.contains("CLAX_SERVE__PORT"), "{m}");
        assert_eq!(e(&[("nope".into(), json!(1))], &[]).0, "unknown_setting");
        assert_eq!(
            e(&[("hosted.public_origin".into(), json!("https://x"))], &[]).0,
            "unknown_setting"
        );
        assert_eq!(e(&[("bin".into(), json!("/x"))], &[]).0, "not_editable");
        let (code, m) = e(&[("serve.port".into(), json!(0))], &[]);
        assert_eq!(code, "invalid_value");
        assert!(m.contains("[serve]"), "{m}");
        assert_eq!(
            e(&[("sample.max_tokens".into(), json!("many"))], &[]).0,
            "invalid_value"
        );
        assert_eq!(
            e(
                &[("bot.base_url".into(), json!("http://evil.example"))],
                &[]
            )
            .0,
            "invalid_value"
        );
        assert_eq!(
            e(&[("questions.terminal_after_s".into(), json!(9999))], &[]).0,
            "invalid_value"
        );
        assert_eq!(
            e(&[("bot.max_steps".into(), json!([1]))], &[]).0,
            "invalid_value"
        );
        // One bad change refuses the batch.
        assert_eq!(
            e(
                &[
                    ("sample.max_tokens".into(), json!(5)),
                    ("bot.concurrency".into(), json!(0)),
                ],
                &[]
            )
            .0,
            "invalid_value"
        );
        assert_eq!(read(&dir), text);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(super::super::FILE)]);
    }

    #[test]
    fn a_write_reports_each_change_and_pending_values_until_a_restart() {
        let dir = home("[bot]\nmax_steps = 7\n");
        let running = HomeConfig::load_with_env(dir.path(), []).unwrap();
        let w = write(
            dir.path(),
            &[],
            &[
                ("bot.max_steps".into(), json!(9)),
                ("bot.concurrency".into(), json!(2)),
            ],
        )
        .unwrap();
        assert_eq!(
            w.changes[0],
            Change {
                key: "bot.max_steps".into(),
                old: json!(7),
                old_source: Source::File,
                new: json!(9),
                new_source: Source::File,
            }
        );
        assert_eq!(
            (
                &w.changes[1].old,
                &w.changes[1].old_source,
                &w.changes[1].new
            ),
            (&json!(4), &Source::Default, &json!(2))
        );
        let current = HomeConfig::load_with_env(dir.path(), []).unwrap();
        let fs = fields_in_effect(&running, &current);
        let steps = field(&fs, "bot.max_steps");
        assert_eq!((&steps.value, &steps.pending), (&json!(7), &Some(json!(9))));
        assert_eq!(field(&fs, "bot.max_tokens").pending, None);
        let v = serde_json::to_value(field(&fs, "bot.max_tokens")).unwrap();
        assert!(v.get("pending").is_none(), "{v}");
    }

    #[test]
    fn a_write_through_a_symlink_writes_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("elsewhere.toml");
        std::fs::write(&real, "# kept\n[serve]\nport = 7481\n").unwrap();
        let link = dir.path().join(super::super::FILE);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(dir.path(), &[], &[("bot.max_steps".into(), json!(3))]).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "# kept\n[serve]\nport = 7481\n\n[bot]\nmax_steps = 3\n"
        );
    }

    #[test]
    fn a_dangling_symlink_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join(super::super::FILE);
        std::os::unix::fs::symlink(dir.path().join("gone.toml"), &link).unwrap();
        let e = write(dir.path(), &[], &[("bot.max_steps".into(), json!(3))]).unwrap_err();
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "config_symlink_dangling",
                    ..
                }
            ),
            "{e}"
        );
        assert!(e.to_string().contains("gone.toml"), "{e}");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!dir.path().join("gone.toml").exists());
    }

    #[test]
    fn a_new_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &[], &[("bot.max_steps".into(), json!(3))]).unwrap();
        let p = dir.path().join(super::super::FILE);
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_write_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = home("[serve]\nport = 7481\n");
        let p = dir.path().join(super::super::FILE);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        write(dir.path(), &[], &[("serve.port".into(), json!(7482))]).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
