//! `clax publish`: builds the `publish` tool's arguments from the command
//! line and runs the tool, so the CLI and the tool share one set of
//! publish rules. Only what the tool has no counterpart for is here: the
//! `--dir` walk and its mirroring, the default `if_version` of the owner's
//! CLI, and the printing.

use crate::client::Client;
use anyhow::Context;
use clax_core::Home;
use clax_mcp::tools::{FileArg, PublishArgs};
use rmcp::handler::server::wrapper::Parameters;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// The page to publish as index.html.
    pub index: PathBuf,
    /// Extra file, optionally renamed: path or path=published/name. The text is
    /// split at the first `=` only when what precedes it is an existing file;
    /// otherwise the whole text is the path. Repeatable.
    #[arg(long = "file")]
    pub files: Vec<String>,
    /// Include every file under this directory (except index.html) at its relative path.
    /// Entries whose name starts with `.` and symlinks are skipped. When updating
    /// (--id/--url) the directory is mirrored: carried-forward files absent from it
    /// are removed.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Update this artifact instead of creating one.
    #[arg(long, conflicts_with = "url")]
    pub id: Option<String>,
    /// Update the artifact at this URL instead of creating one.
    #[arg(long)]
    pub url: Option<String>,
    /// Artifact title. When creating without it, the page's <title> is used.
    #[arg(long)]
    pub title: Option<String>,
    /// One-line description.
    #[arg(long)]
    pub description: Option<String>,
    /// Icon name for the artifact.
    #[arg(long)]
    pub icon: Option<String>,
    /// Label for this version.
    #[arg(long)]
    pub label: Option<String>,
    /// Expected current version; defaults to the artifact's current version.
    #[arg(long)]
    pub if_version: Option<u32>,
    /// A short change note shown to people as this version's changelog.
    #[arg(long)]
    pub note: Option<String>,
    /// IDs of comment threads this version addresses (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub addresses: Vec<String>,
    /// Existing tags to add to the artifact (names, comma-separated; see
    /// `clax tags`). An unknown tag fails before anything is published.
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
}

/// Printed after the URL when no agent session owns the published artifact.
const NO_SESSION_NOTE: &str = "note: published without an agent session; comments on this page will wait until an agent session watches it";

/// `path`, made absolute: the tool resolves no relative path without a
/// session.
fn absolute(path: &Path) -> anyhow::Result<String> {
    Ok(std::path::absolute(path)
        .with_context(|| format!("cannot resolve {}", path.display()))?
        .to_string_lossy()
        .into_owned())
}

/// A supporting file the tool reads from `path`.
fn file_arg(path: &Path) -> anyhow::Result<Option<FileArg>> {
    Ok(Some(FileArg {
        path: Some(absolute(path)?),
        ..Default::default()
    }))
}

fn collect_dir(
    dir: &Path,
    base: &Path,
    out: &mut BTreeMap<String, Option<FileArg>>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') || entry.file_type()?.is_symlink() {
            continue;
        }
        let p = entry.path();
        if p.is_dir() {
            collect_dir(&p, base, out)?;
            continue;
        }
        let rel = p.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
        if rel == clax_core::publish::INDEX {
            continue;
        }
        out.insert(rel, file_arg(&p)?);
    }
    Ok(())
}

/// The source path and published name of a `--file` spec: split at the first
/// `=` when the text before it names an existing file, else the whole spec is
/// the source and its file name is the published name.
fn split_file_spec(spec: &str) -> anyhow::Result<(PathBuf, String)> {
    if let Some((src, dest)) = spec.split_once('=')
        && Path::new(src).is_file()
    {
        return Ok((PathBuf::from(src), dest.to_string()));
    }
    let dest = Path::new(spec)
        .file_name()
        .context("--file needs a file name")?
        .to_string_lossy()
        .to_string();
    Ok((PathBuf::from(spec), dest))
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let mut files = BTreeMap::new();
    if let Some(dir) = &a.dir {
        collect_dir(dir, dir, &mut files)?;
    }
    for spec in &a.files {
        let (src, dest) = split_file_spec(spec)?;
        files.insert(dest, file_arg(&src)?);
    }
    let target = match (&a.id, &a.url) {
        (Some(id), _) => Some(super::open::id_from(id)?),
        (_, Some(u)) => Some(super::open::id_from(u)?),
        _ => None,
    };
    // The tool refuses a new artifact without a title too; this names the
    // CLI's flag and file.
    if target.is_none() && a.title.is_none() {
        let page = std::fs::read(&a.index)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", a.index.display()))?;
        if std::str::from_utf8(&page)
            .ok()
            .and_then(clax_core::html_title)
            .is_none()
        {
            anyhow::bail!(
                "a new artifact needs a title: pass --title, or give {} a non-empty <title>",
                a.index.display()
            );
        }
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    let mut if_version = a.if_version;
    if let Some(id) = &target {
        // The owner's CLI: an update without --if-version is based on the
        // current version.
        if if_version.is_none() {
            let current = c.get(&format!("/api/artifacts/{id}"))?["artifact"]["current_version"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .context("server response lacks artifact.current_version")?;
            if_version = Some(current);
        }
        if a.dir.is_some() {
            let current_files = c.get(&format!("/api/artifacts/{id}/files"))?;
            let existing = current_files["files"]
                .as_object()
                .context("server response lacks files")?;
            for path in existing.keys() {
                if path != clax_core::publish::INDEX && !files.contains_key(path) {
                    files.insert(path.clone(), None);
                }
            }
        }
    }
    let args = PublishArgs {
        file_path: Some(absolute(&a.index)?),
        files: (!files.is_empty()).then_some(files),
        id: target,
        if_version,
        title: a.title.clone(),
        description: a.description.clone(),
        icon: a.icon.clone(),
        label: a.label.clone(),
        note: a.note.clone(),
        addresses: (!a.addresses.is_empty()).then(|| a.addresses.clone()),
        tags: (!a.tags.is_empty()).then(|| a.tags.clone()),
        ..Default::default()
    };
    let res = super::tools::call_on(&c, home, move |t: clax_mcp::ClaxTools| async move {
        t.publish(Parameters(args)).await
    })?;
    let id = res["artifact_id"]
        .as_str()
        .context("the publish result lacks artifact_id")?
        .to_string();
    let url = res["url"].as_str().unwrap_or_default().to_string();
    // The session comments go to: the one this version is attributed to, else
    // the artifact's owner. The CLI attributes none of its own. The publish
    // has landed: when this read fails, the session is unknown (`None`),
    // and the command still succeeds.
    let session: Option<Option<String>> = c
        .get(&format!("/api/artifacts/{id}"))
        .ok()
        .map(|got| session_of(&got, &res["version"]));
    super::print(
        cli,
        serde_json::json!({
            "id": id,
            "url": url,
            "version": res["version"],
            "session": session.clone().flatten(),
            "note": res["note"],
            "addressed": res["addressed"],
            "tags": res["tags"],
            "tags_error": res.get("tags_error"),
        }),
        |j| {
            let mut text = format!(
                "published v{} at {}",
                j["version"],
                j["url"].as_str().unwrap_or_default()
            );
            let addressed = j["addressed"].as_array().map_or(0, Vec::len);
            if addressed > 0 {
                let s = if addressed == 1 { "" } else { "s" };
                text.push_str(&format!("\naddresses {addressed} comment{s}"));
            }
            if matches!(session, Some(None)) {
                text.push('\n');
                text.push_str(NO_SESSION_NOTE);
            }
            if let Some(tags) = j["tags"].as_array().filter(|t| !t.is_empty()) {
                let names: Vec<&str> = tags.iter().filter_map(|t| t.as_str()).collect();
                text.push_str(&format!("\ntags: {}", names.join(", ")));
            }
            text
        },
    );
    // The version landed; tags that could not be applied fail the command
    // after it says so.
    if let Some(e) = res.get("tags_error") {
        anyhow::bail!(
            "published, but the tags were not applied: {}",
            e["message"].as_str().unwrap_or("refused")
        );
    }
    Ok(())
}

/// The session version `n` of the artifact `got` (`GET
/// /api/artifacts/<id>`) sends comments to: the one the version is
/// attributed to, else the artifact's owner; `None` for neither.
fn session_of(got: &serde_json::Value, n: &serde_json::Value) -> Option<String> {
    got["versions"]
        .as_array()
        .and_then(|vs| vs.iter().find(|v| &v["n"] == n))
        .and_then(|v| v["session_id"].as_str())
        .or(got["artifact"]["owner_session_id"].as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_spec_splits_on_first_equals_only_for_an_existing_source() {
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("a=b.js");
        std::fs::write(&odd, "1").unwrap();
        let src = dir.path().join("src.js");
        std::fs::write(&src, "2").unwrap();

        let spec = odd.to_string_lossy().to_string();
        assert_eq!(
            split_file_spec(&spec).unwrap(),
            (odd.clone(), "a=b.js".into())
        );
        let spec = format!("{}=lib/dest.js", src.display());
        assert_eq!(split_file_spec(&spec).unwrap(), (src, "lib/dest.js".into()));
        assert_eq!(
            split_file_spec("nope=dest.js").unwrap(),
            (PathBuf::from("nope=dest.js"), "nope=dest.js".into())
        );
    }
}
