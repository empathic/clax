//! Which Clax MCP tools Codex asks the person about before running, and the
//! settings in Codex's `config.toml` that approve them.
//!
//! Codex decides per tool call from an approval mode: the tool's own
//! `approval_mode`, else its server's `default_tools_approval_mode`, else
//! `auto`. For the Clax plugin both live under
//! `[plugins."clax@clax".mcp_servers.clax]` (a `tools.<tool>` table for the
//! tool's own). The modes, as Codex 0.160 applies them:
//!
//! - `approve`: never asks;
//! - `prompt`: always asks;
//! - `writes`: asks unless the tool is annotated `readOnlyHint: true`;
//! - `auto`: asks when the tool is annotated `destructiveHint: true`, or is
//!   not read-only and not annotated `openWorldHint: false` (a tool without
//!   annotations asks).
//!
//! Every Clax tool is annotated `openWorldHint: false`, so under `auto` only
//! the destructive ones ask. Approving those is a grant the person makes:
//! it is offered only for tools the person has set nothing for, neither on
//! the tool nor on the server ([`Assessment::addable`]); a tool that asks
//! because of the person's own setting is theirs ([`Assessment::kept`]).
//! [`add_approvals`] never changes or removes a setting.
//!
//! `codex plugin remove` deletes the plugin's whole table, settings
//! included, so `clax init`, which removes and adds the plugin, carries them
//! across with [`plugin_settings`] and [`restore`].

use rmcp::model::{Tool, ToolAnnotations};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, value};

/// The plugin's name in Codex.
pub const PLUGIN: &str = "clax@clax";
/// The plugin's MCP server.
pub const SERVER: &str = "clax";

/// The command that shows the approval settings and adds them on
/// confirmation.
pub const SETUP_COMMAND: &str = "clax init --agent codex";

/// Where an effective approval mode came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// No setting: Codex's default, `auto`.
    Default,
    /// The server's `default_tools_approval_mode`.
    Server,
    /// The tool's own `approval_mode`.
    Tool,
}

/// A tool Codex asks about before each call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asking {
    pub tool: String,
    /// The effective approval mode, as written (`auto` when unset).
    pub mode: String,
    pub source: Source,
    /// Whether the tool is annotated `destructiveHint: true`.
    pub destructive: bool,
}

impl Asking {
    /// The person's setting that makes the tool ask, as written in
    /// `config.toml`; `None` when nothing is set.
    pub fn setting(&self) -> Option<String> {
        match self.source {
            Source::Default => None,
            Source::Server => Some(format!("default_tools_approval_mode = \"{}\"", self.mode)),
            Source::Tool => Some(format!(
                "approval_mode = \"{}\" on `{}`",
                self.mode, self.tool
            )),
        }
    }
}

/// The Clax tools Codex asks about under one `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assessment {
    /// Whether the config registers the plugin (`[plugins."clax@clax"]`).
    pub registered: bool,
    /// Each tool Codex asks about, in tool order.
    pub asking: Vec<Asking>,
}

impl Assessment {
    /// The asking tools the person has set nothing for, on the tool or
    /// the server: those setup offers to approve.
    pub fn addable_asking(&self) -> Vec<&Asking> {
        self.asking
            .iter()
            .filter(|a| a.source == Source::Default)
            .collect()
    }

    /// The names of [`Self::addable_asking`].
    pub fn addable(&self) -> Vec<&str> {
        self.addable_asking()
            .into_iter()
            .map(|a| a.tool.as_str())
            .collect()
    }

    /// The asking tools that ask because of a setting the person made, on
    /// the tool or on the server; setup leaves them alone.
    pub fn kept(&self) -> Vec<&Asking> {
        self.asking
            .iter()
            .filter(|a| a.source != Source::Default)
            .collect()
    }

    /// Names the tools in [`Self::kept`] with the setting that makes each
    /// ask, grouped by setting; `None` when there are none.
    pub fn kept_note(&self, config: &Path) -> Option<String> {
        let kept = self.kept();
        if kept.is_empty() {
            return None;
        }
        let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
        for k in kept {
            let s = k.setting().unwrap_or_default();
            match groups.iter_mut().find(|(g, _)| *g == s) {
                Some((_, tools)) => tools.push(&k.tool),
                None => groups.push((s, vec![&k.tool])),
            }
        }
        let parts: Vec<String> = groups
            .iter()
            .map(|(s, tools)| format!("{} (your {s})", tools.join(", ")))
            .collect();
        Some(format!(
            "Codex asks before {}, as set in {}; Clax leaves your settings as they are",
            parts.join("; "),
            config.display()
        ))
    }
}

/// Whether Codex asks before calling a tool annotated `a` under `mode`. An
/// unknown mode is taken to ask.
pub fn asks(mode: &str, a: Option<&ToolAnnotations>) -> bool {
    let read_only = a.and_then(|a| a.read_only_hint) == Some(true);
    match mode {
        "approve" => false,
        "writes" => !read_only,
        "auto" => {
            let destructive = a.and_then(|a| a.destructive_hint) == Some(true);
            let closed = a.and_then(|a| a.open_world_hint) == Some(false);
            destructive || (!read_only && !closed)
        }
        _ => true,
    }
}

/// `$CODEX_HOME/config.toml` (`codex_home` is `$CODEX_HOME`, else
/// `~/.codex`).
pub fn config_path(codex_home: &Path) -> PathBuf {
    codex_home.join("config.toml")
}

/// The config's text: `Ok(None)` when it does not exist.
pub fn read_config(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// `path` with symbolic links followed, including one whose target does
/// not exist yet; an error for a loop.
fn follow_links(path: &Path) -> std::io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                let to = std::fs::read_link(&p)?;
                p = match p.parent() {
                    Some(dir) if to.is_relative() => dir.join(to),
                    _ => to,
                };
            }
            _ => return Ok(p),
        }
    }
    Err(std::io::Error::other(format!(
        "{}: too many levels of symbolic links",
        path.display()
    )))
}

/// Replaces the config at `path` with `text` atomically: a sibling
/// temporary file, with the old file's permissions, renamed over it. A
/// symbolic link at `path` is followed, even when its target does not exist
/// yet, so the file it names is written and the link stays.
pub fn write_config(path: &Path, text: &str) -> std::io::Result<()> {
    let target = follow_links(path)?;
    let dir = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".config.toml.clax.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Ok(m) = std::fs::metadata(&target) {
        std::fs::set_permissions(&tmp, m.permissions())?;
    }
    std::fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn parse(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>().map_err(|e| e.to_string())
}

/// `[plugins."clax@clax"]`, when present.
fn plugin_table(doc: &DocumentMut) -> Option<&dyn toml_edit::TableLike> {
    doc.get("plugins")?
        .as_table_like()?
        .get(PLUGIN)?
        .as_table_like()
}

fn server_table(doc: &DocumentMut) -> Option<&dyn toml_edit::TableLike> {
    plugin_table(doc)?
        .get("mcp_servers")?
        .as_table_like()?
        .get(SERVER)?
        .as_table_like()
}

fn str_list(t: &dyn toml_edit::TableLike, key: &str) -> Option<Vec<String>> {
    Some(
        t.get(key)?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
    )
}

/// The tools Codex asks about under the config `text` (`None`: no config),
/// for the server's `tools`. Tools the server's `enabled_tools` leaves out
/// or its `disabled_tools` names are not offered, so never asked about; a
/// disabled plugin asks about nothing.
pub fn assess(text: Option<&str>, tools: &[Tool]) -> Result<Assessment, String> {
    let doc = parse(text.unwrap_or_default())?;
    let plugin = plugin_table(&doc);
    let registered = plugin.is_some();
    if plugin
        .and_then(|p| p.get("enabled"))
        .and_then(Item::as_bool)
        == Some(false)
    {
        return Ok(Assessment {
            registered,
            asking: Vec::new(),
        });
    }
    let server = server_table(&doc);
    let enabled = server.and_then(|s| str_list(s, "enabled_tools"));
    let disabled = server
        .and_then(|s| str_list(s, "disabled_tools"))
        .unwrap_or_default();
    let server_mode = server
        .and_then(|s| s.get("default_tools_approval_mode"))
        .and_then(Item::as_str);
    let mut asking = Vec::new();
    for t in tools {
        let name = t.name.as_ref();
        if disabled.iter().any(|d| d == name)
            || enabled
                .as_ref()
                .is_some_and(|e| !e.iter().any(|x| x == name))
        {
            continue;
        }
        let own = server
            .and_then(|s| s.get("tools"))
            .and_then(Item::as_table_like)
            .and_then(|ts| ts.get(name))
            .and_then(Item::as_table_like)
            .and_then(|t| t.get("approval_mode"))
            .and_then(Item::as_str);
        let (mode, source) = match (own, server_mode) {
            (Some(m), _) => (m, Source::Tool),
            (None, Some(m)) => (m, Source::Server),
            (None, None) => ("auto", Source::Default),
        };
        if asks(mode, t.annotations.as_ref()) {
            asking.push(Asking {
                tool: name.to_string(),
                mode: mode.to_string(),
                source,
                destructive: t.annotations.as_ref().and_then(|a| a.destructive_hint) == Some(true),
            });
        }
    }
    Ok(Assessment { registered, asking })
}

/// The `config.toml` lines that approve `tools`, as [`add_approvals`]
/// writes them.
pub fn lines(tools: &[&str]) -> String {
    tools
        .iter()
        .map(|t| {
            format!(
                "[plugins.\"{PLUGIN}\".mcp_servers.{SERVER}.tools.{t}]\napproval_mode = \"approve\"\n"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The table at `key` under `parent`, created implicit (no header of its
/// own) when absent; an error when `key` holds something else.
fn child<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table, String> {
    let item = parent.entry(key).or_insert_with(|| {
        let mut t = Table::new();
        t.set_implicit(true);
        Item::Table(t)
    });
    item.as_table_mut()
        .ok_or_else(|| format!("`{key}` in config.toml is not a table"))
}

/// `text` with `approval_mode = "approve"` for each of `tools` that has no
/// `approval_mode`, everything else (comments and layout included) as it
/// was. Adding what is already there changes nothing.
pub fn add_approvals(text: &str, tools: &[&str]) -> Result<String, String> {
    let mut doc = parse(text)?;
    let root = doc.as_table_mut();
    let plugins = child(root, "plugins")?;
    let plugin = child(plugins, PLUGIN)?;
    let servers = child(plugin, "mcp_servers")?;
    let server = child(servers, SERVER)?;
    let tool_tables = child(server, "tools")?;
    for t in tools {
        let tool = tool_tables
            .entry(t)
            .or_insert_with(|| Item::Table(Table::new()));
        let tool = tool
            .as_table_mut()
            .ok_or_else(|| format!("the `{t}` tool's entry in config.toml is not a table"))?;
        if !tool.contains_key("approval_mode") {
            tool.insert("approval_mode", value("approve"));
        }
    }
    Ok(doc.to_string())
}

/// The person's settings for the plugin besides `enabled` (`mcp_servers`
/// and anything else under `[plugins."clax@clax"]`), to put back with
/// [`restore`] after the plugin is removed and added again.
pub fn plugin_settings(text: &str) -> Option<Table> {
    let doc = parse(text).ok()?;
    let mut t = doc
        .get("plugins")?
        .get(PLUGIN)?
        .as_table_like()?
        .iter()
        .filter(|(k, _)| *k != "enabled")
        .fold(Table::new(), |mut t, (k, v)| {
            t.insert(k, v.clone());
            t
        });
    t.set_implicit(true);
    (!t.is_empty()).then_some(t)
}

/// Every key of `from` that `into` lacks, copied in, recursing into tables
/// both have.
pub fn merge_missing(into: &mut Table, from: &Table) {
    for (k, v) in from.iter() {
        match (into.get_mut(k), v) {
            (None, _) => {
                into.insert(k, v.clone());
            }
            (Some(Item::Table(a)), Item::Table(b)) => merge_missing(a, b),
            _ => {}
        }
    }
}

/// `text` with the `saved` plugin settings put back where it lacks them;
/// what `text` has is kept.
pub fn restore(text: &str, saved: &Table) -> Result<String, String> {
    let mut doc = parse(text)?;
    let plugins = child(doc.as_table_mut(), "plugins")?;
    let plugin = child(plugins, PLUGIN)?;
    merge_missing(plugin, saved);
    Ok(doc.to_string())
}

/// The notice a Codex session start shows the person when Codex would stop
/// to ask before Clax tools: the tools and how to approve them once.
/// `command` is how to run `clax`, `None` when no `clax` is on `PATH`, in
/// which case the notice names the settings instead. `None` when no tool
/// would be added.
pub fn notice(a: &Assessment, config: &Path, command: Option<&str>) -> Option<String> {
    let asking = a.addable_asking();
    if asking.is_empty() {
        return None;
    }
    let tools: Vec<&str> = asking.iter().map(|a| a.tool.as_str()).collect();
    let how = match command {
        Some(c) => format!(
            "run `{c} init --agent codex` in a terminal: it shows the lines it adds to {} and adds them once you confirm",
            config.display()
        ),
        None => format!(
            "add `approval_mode = \"approve\"` under `[plugins.\"{PLUGIN}\".mcp_servers.{SERVER}.tools.<tool>]` in {} for each",
            config.display()
        ),
    };
    Some(format!(
        "Clax: Codex will stop to ask before each call of {}. {} To approve them once, {how}.",
        tools.join(", "),
        consequences(&asking)
    ))
}

/// What the tools in `asking` do, from their annotations and names, and
/// what approving them means; the text the setup prompt and the notice show.
pub fn consequences(asking: &[&Asking]) -> String {
    let mut parts = Vec::new();
    if asking.iter().any(|a| a.tool == "delete" && a.destructive) {
        parts.push("`delete` removes an artifact and all its versions".to_string());
    }
    let db: Vec<&str> = asking
        .iter()
        .filter(|a| a.destructive && a.tool.starts_with("db_"))
        .map(|a| a.tool.as_str())
        .collect();
    if !db.is_empty() {
        parts.push(format!(
            "{} replace or remove page data, which keeps no history",
            db.join(", ")
        ));
    }
    let other: Vec<&str> = asking
        .iter()
        .filter(|a| a.destructive && a.tool != "delete" && !a.tool.starts_with("db_"))
        .map(|a| a.tool.as_str())
        .collect();
    if !other.is_empty() {
        parts.push(format!("{} may replace or remove data", other.join(", ")));
    }
    let mut out = String::new();
    if !parts.is_empty() {
        out = format!("Of these, {}. ", parts.join("; "));
    }
    out.push_str("Once approved, Codex runs them without asking, including when it acts on comments people leave on a page.");
    out
}

/// The marker recording the last notice for the Codex home `codex_home`,
/// under the Clax home `clax_root`: one per Codex home.
pub fn notice_marker(clax_root: &Path, codex_home: &Path) -> PathBuf {
    // FNV-1a, so the name is stable across builds.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in codex_home.as_os_str().as_encoded_bytes() {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    clax_root.join(format!("run/codex-approvals-notice-{h:016x}"))
}

/// Whether the notice for `tools` is due: when the marker records another
/// set, or none. The notice is shown once per set of tools; `clax doctor`
/// reports the tools any time after. With no tools, the marker is removed,
/// so the next set that asks is shown.
pub fn notice_due(marker: &Path, tools: &[&str]) -> bool {
    if tools.is_empty() {
        let _ = std::fs::remove_file(marker);
        return false;
    }
    std::fs::read_to_string(marker).map_or(true, |r| r.trim_end() != tools.join(","))
}

/// Records that the notice for `tools` was shown.
pub fn record_notice(marker: &Path, tools: &[&str]) {
    if let Some(d) = marker.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(marker, format!("{}\n", tools.join(",")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_mcp::tools::ClaxTools;

    fn ann(ro: Option<bool>, d: Option<bool>, ow: Option<bool>) -> ToolAnnotations {
        let mut a = ToolAnnotations::default();
        a.read_only_hint = ro;
        a.destructive_hint = d;
        a.open_world_hint = ow;
        a
    }

    /// The decisions observed from Codex 0.160 for each mode and each kind
    /// of annotation (docs/superpowers/specs/2026-10-07-codex-permissions-design.md, §2.1).
    #[test]
    fn asks_matches_what_codex_did() {
        let none = None;
        let ro = Some(ann(Some(true), None, Some(false)));
        let ro_only = Some(ann(Some(true), None, None));
        let local = Some(ann(Some(false), Some(false), Some(false)));
        let destructive = Some(ann(Some(false), Some(true), Some(false)));
        let open = Some(ann(Some(false), Some(false), Some(true)));
        let kinds = [&none, &ro, &ro_only, &local, &destructive, &open];
        let row =
            |mode: &str| -> Vec<bool> { kinds.iter().map(|a| asks(mode, a.as_ref())).collect() };
        assert_eq!(row("auto"), [true, false, false, false, true, true]);
        assert_eq!(row("writes"), [true, false, false, true, true, true]);
        assert_eq!(row("prompt"), [true; 6]);
        assert_eq!(row("approve"), [false; 6]);
        assert_eq!(row("someday"), [true; 6]);
    }

    fn names(a: &Assessment) -> Vec<&str> {
        a.asking.iter().map(|x| x.tool.as_str()).collect()
    }

    const DESTRUCTIVE: [&str; 6] = [
        "db_batch",
        "db_delete",
        "db_set",
        "db_str_replace",
        "db_update",
        "delete",
    ];

    #[test]
    fn with_no_settings_only_the_destructive_tools_ask() {
        let a = assess(
            Some("[plugins.\"clax@clax\"]\nenabled = true\n"),
            &ClaxTools::tools(),
        )
        .unwrap();
        assert!(a.registered);
        assert_eq!(names(&a), DESTRUCTIVE);
        assert_eq!(a.addable(), DESTRUCTIVE);
        assert!(a.asking.iter().all(|x| x.source == Source::Default));
        let none = assess(None, &ClaxTools::tools()).unwrap();
        assert!(!none.registered);
        assert_eq!(names(&none), DESTRUCTIVE);
    }

    #[test]
    fn tool_settings_override_the_server_default() {
        let cfg = r#"
[plugins."clax@clax"]
enabled = true

[plugins."clax@clax".mcp_servers.clax]
default_tools_approval_mode = "writes"
disabled_tools = ["db_batch"]

[plugins."clax@clax".mcp_servers.clax.tools.publish]
approval_mode = "approve"

[plugins."clax@clax".mcp_servers.clax.tools.delete]
approval_mode = "prompt"
"#;
        let a = assess(Some(cfg), &ClaxTools::tools()).unwrap();
        let asking = names(&a);
        assert!(!asking.contains(&"publish"), "{asking:?}");
        assert!(!asking.contains(&"db_batch"), "disabled: {asking:?}");
        assert!(!asking.contains(&"read"), "read-only under writes");
        assert!(asking.contains(&"pin"), "writes asks for pin");
        // Every tool that asks does so by the person's own settings.
        assert!(a.addable().is_empty(), "{:?}", a.addable());
        assert_eq!(a.kept().len(), a.asking.len());
        let delete = a.kept().into_iter().find(|k| k.tool == "delete").unwrap();
        assert_eq!(
            (delete.source, delete.setting().unwrap()),
            (
                Source::Tool,
                "approval_mode = \"prompt\" on `delete`".into()
            )
        );
        let pin = a.kept().into_iter().find(|k| k.tool == "pin").unwrap();
        assert_eq!(
            pin.setting().unwrap(),
            "default_tools_approval_mode = \"writes\""
        );
        let note = a.kept_note(Path::new("/cx/config.toml")).unwrap();
        assert!(
            note.contains("delete (your approval_mode = \"prompt\" on `delete`)"),
            "{note}"
        );
        assert!(
            note.contains("(your default_tools_approval_mode = \"writes\")"),
            "{note}"
        );
    }

    /// A server default the person set is theirs: nothing is offered over
    /// it, and every tool it makes ask is named with it.
    #[test]
    fn a_server_default_of_the_persons_is_never_offered_over() {
        let tools = ClaxTools::tools();
        let server = |mode: &str| {
            assess(
                Some(&format!(
                    "[plugins.\"clax@clax\".mcp_servers.clax]\ndefault_tools_approval_mode = \"{mode}\"\n"
                )),
                &tools,
            )
            .unwrap()
        };
        let prompt = server("prompt");
        assert!(prompt.addable().is_empty());
        assert_eq!(prompt.kept().len(), tools.len());
        assert!(prompt.kept().iter().all(|k| k.source == Source::Server));
        let writes = server("writes");
        assert!(writes.addable().is_empty());
        let read_only = tools
            .iter()
            .filter(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
            .count();
        assert_eq!(writes.kept().len(), tools.len() - read_only);
        assert!(!names(&writes).contains(&"read"));
        assert!(server("approve").asking.is_empty());
        assert_eq!(notice(&prompt, Path::new("/c"), Some("clax")), None);
        let p = add_approvals("", &prompt.addable()).unwrap();
        assert!(!p.contains("approval_mode"), "{p}");
    }

    #[test]
    fn approve_on_the_server_or_a_disabled_plugin_asks_nothing() {
        let cfg =
            "[plugins.\"clax@clax\".mcp_servers.clax]\ndefault_tools_approval_mode = \"approve\"\n";
        assert!(
            assess(Some(cfg), &ClaxTools::tools())
                .unwrap()
                .asking
                .is_empty()
        );
        let off = "[plugins.\"clax@clax\"]\nenabled = false\n";
        assert!(
            assess(Some(off), &ClaxTools::tools())
                .unwrap()
                .asking
                .is_empty()
        );
        let only =
            "[plugins.\"clax@clax\".mcp_servers.clax]\nenabled_tools = [\"publish\", \"delete\"]\n";
        assert_eq!(
            names(&assess(Some(only), &ClaxTools::tools()).unwrap()),
            ["delete"]
        );
    }

    #[test]
    fn a_config_that_does_not_parse_is_an_error() {
        assert!(assess(Some("[plugins\n"), &ClaxTools::tools()).is_err());
    }

    const SEED: &str = r#"# my settings
model = "gpt-6-astra"

[plugins."clax@clax"]
enabled = true # keep me

[plugins."clax@clax".mcp_servers.clax.tools.publish]
approval_mode = "approve"

[plugins."clax@clax".mcp_servers.clax.tools.delete]
approval_mode = "prompt"

[tui]
screen_reader_detection_done = true
"#;

    #[test]
    fn adding_approvals_keeps_everything_and_overrides_nothing() {
        let out = add_approvals(SEED, &["delete", "db_set"]).unwrap();
        // The new table lands beside the plugin's other tool tables.
        let want = SEED.replace("\n[tui]", &format!("\n{}\n[tui]", lines(&["db_set"])));
        assert_eq!(out, want);
        let a = assess(Some(&out), &ClaxTools::tools()).unwrap();
        assert_eq!(a.kept()[0].tool, "delete");
        assert!(!names(&a).contains(&"db_set"));
        assert_eq!(add_approvals(&out, &["delete", "db_set"]).unwrap(), out);
    }

    #[test]
    fn adding_to_an_empty_config_writes_exactly_the_lines() {
        let tools = ["delete", "db_set"];
        let out = add_approvals("", &tools).unwrap();
        assert_eq!(out, lines(&tools));
        let a = assess(Some(&out), &ClaxTools::tools()).unwrap();
        assert!(!names(&a).contains(&"delete"));
    }

    #[test]
    fn adding_where_a_key_is_not_a_table_fails() {
        assert!(add_approvals("plugins = 1\n", &["delete"]).is_err());
    }

    #[test]
    fn settings_survive_removal_and_registration() {
        let saved = plugin_settings(SEED).expect("settings");
        assert!(!saved.contains_key("enabled"));
        // What `codex plugin remove` then `codex plugin add` leave.
        let readded = "# my settings\nmodel = \"gpt-6-astra\"\n\n[tui]\nscreen_reader_detection_done = true\n\n[plugins.\"clax@clax\"]\nenabled = true\n";
        let out = restore(readded, &saved).unwrap();
        assert!(out.starts_with(readded), "{out}");
        let a = assess(Some(&out), &ClaxTools::tools()).unwrap();
        assert!(!names(&a).contains(&"publish"));
        assert_eq!(a.kept()[0].tool, "delete");
        assert_eq!(restore(&out, &saved).unwrap(), out);
        assert!(plugin_settings("[plugins.\"clax@clax\"]\nenabled = true\n").is_none());
        assert!(plugin_settings("").is_none());
    }

    #[test]
    fn restoring_keeps_newer_settings() {
        let saved = plugin_settings(SEED).unwrap();
        let now =
            "[plugins.\"clax@clax\".mcp_servers.clax.tools.delete]\napproval_mode = \"approve\"\n";
        let out = restore(now, &saved).unwrap();
        let a = assess(Some(&out), &ClaxTools::tools()).unwrap();
        assert!(!names(&a).contains(&"delete"), "{out}");
    }

    #[test]
    fn the_notice_names_the_command_or_the_settings() {
        let a = assess(None, &ClaxTools::tools()).unwrap();
        let p = Path::new("/cx/config.toml");
        let with = notice(&a, p, Some("clax")).unwrap();
        assert!(with.contains("delete"), "{with}");
        assert!(with.contains("`clax init --agent codex`"), "{with}");
        let without = notice(&a, p, None).unwrap();
        assert!(
            without.contains("[plugins.\"clax@clax\".mcp_servers.clax.tools.<tool>]"),
            "{without}"
        );
        assert!(
            with.contains("removes an artifact and all its versions"),
            "{with}"
        );
        assert!(with.contains("keeps no history"), "{with}");
        assert!(
            with.contains("including when it acts on comments"),
            "{with}"
        );
        let approved = add_approvals("", &a.addable()).unwrap();
        let none = assess(Some(&approved), &ClaxTools::tools()).unwrap();
        assert_eq!(notice(&none, p, Some("clax")), None);
    }

    #[test]
    fn consequences_claim_only_what_the_annotations_say() {
        let a = |tool: &str, destructive: bool| Asking {
            tool: tool.into(),
            mode: "auto".into(),
            source: Source::Default,
            destructive,
        };
        let (read, pin) = (a("read", false), a("pin", false));
        let text = consequences(&[&read, &pin]);
        assert!(!text.contains("remove"), "{text}");
        assert!(!text.contains("replace"), "{text}");
        let (del, set) = (a("delete", true), a("db_set", true));
        let text = consequences(&[&del, &set]);
        assert!(text.starts_with("Of these, `delete` removes"), "{text}");
        assert!(
            text.contains("db_set replace or remove page data"),
            "{text}"
        );
    }

    #[test]
    fn the_notice_is_due_once_per_set_of_tools() {
        let d = tempfile::tempdir().unwrap();
        let m = notice_marker(d.path(), Path::new("/home/u/.codex"));
        assert_ne!(m, notice_marker(d.path(), Path::new("/home/u/.codex-2")));
        let set = ["delete", "db_set"];
        assert!(notice_due(&m, &set), "never shown");
        assert!(notice_due(&m, &set), "due until recorded");
        record_notice(&m, &set);
        assert!(!notice_due(&m, &set));
        assert!(notice_due(&m, &["delete"]), "another set");
        assert!(!notice_due(&m, &[]));
        assert!(!m.exists());
        assert!(notice_due(&m, &set));
    }

    #[test]
    fn writing_follows_a_symlink_and_keeps_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("dotfiles.toml");
        std::fs::write(&real, "a = 1\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = d.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_config(&link, "a = 2\n").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "a = 2\n");
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let dangling = d.path().join("dangling.toml");
        std::os::unix::fs::symlink("missing/target.toml", &dangling).unwrap();
        write_config(&dangling, "c = 1\n").unwrap();
        assert!(
            std::fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("missing/target.toml")).unwrap(),
            "c = 1\n"
        );
        let fresh = d.path().join("new/config.toml");
        write_config(&fresh, "b = 1\n").unwrap();
        assert_eq!(read_config(&fresh).unwrap().as_deref(), Some("b = 1\n"));
        assert_eq!(read_config(&d.path().join("none")).unwrap(), None);
    }
}
