use crate::client::Client;
use anyhow::Context;
use base64::Engine;
use clax_core::Home;
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
}

/// Printed after the URL when no agent session owns the published artifact.
const NO_SESSION_NOTE: &str = "note: published without an agent session; comments on this page will wait until an agent session watches it";

const TEXT_EXT: &[&str] = &[
    "html", "htm", "css", "js", "mjs", "json", "svg", "md", "txt", "csv", "xml", "map",
];

pub fn file_entry(path: &Path) -> anyhow::Result<serde_json::Value> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(if TEXT_EXT.contains(&ext.as_str()) {
        match String::from_utf8(bytes) {
            Ok(s) => serde_json::json!({"content": s, "encoding": "utf8"}),
            Err(e) => {
                serde_json::json!({"content": base64::engine::general_purpose::STANDARD.encode(e.into_bytes()), "encoding": "base64"})
            }
        }
    } else {
        serde_json::json!({"content": base64::engine::general_purpose::STANDARD.encode(bytes), "encoding": "base64"})
    })
}

fn collect_dir(
    dir: &Path,
    base: &Path,
    out: &mut serde_json::Map<String, serde_json::Value>,
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
        if rel == "index.html" {
            continue;
        }
        out.insert(rel, file_entry(&p)?);
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
    let mut files = serde_json::Map::new();
    files.insert("index.html".into(), file_entry(&a.index)?);
    if let Some(dir) = &a.dir {
        collect_dir(dir, dir, &mut files)?;
    }
    for spec in &a.files {
        let (src, dest) = split_file_spec(spec)?;
        files.insert(dest, file_entry(&src)?);
    }
    let c = Client::connect(home, cli.port_for(home)?)?;
    let target = match (&a.id, &a.url) {
        (Some(id), _) => Some(super::open::id_from(id)?),
        (_, Some(u)) => Some(super::open::id_from(u)?),
        _ => None,
    };
    let title = match (&target, &a.title) {
        (None, None) => Some(
            files["index.html"]["content"]
                .as_str()
                .filter(|_| files["index.html"]["encoding"] == "utf8")
                .and_then(clax_core::html_title)
                .with_context(|| {
                    format!(
                        "a new artifact needs a title: pass --title, or give {} a non-empty <title>",
                        a.index.display()
                    )
                })?,
        ),
        (_, t) => t.clone(),
    };
    let mut body = serde_json::json!({"files": files});
    for (k, v) in [
        ("title", &title),
        ("description", &a.description),
        ("icon", &a.icon),
        ("label", &a.label),
        ("note", &a.note),
    ] {
        if let Some(v) = v {
            body[k] = serde_json::json!(v);
        }
    }
    if !a.addresses.is_empty() {
        body["addresses"] = serde_json::json!(a.addresses);
    }
    let res = match target {
        None => c.post("/api/artifacts", &body)?,
        Some(id) => {
            let current = match a.if_version {
                Some(v) => v,
                None => c.get(&format!("/api/artifacts/{id}"))?["artifact"]["current_version"]
                    .as_u64()
                    .context("server response lacks artifact.current_version")?
                    as u32,
            };
            if a.dir.is_some() {
                let current_files = c.get(&format!("/api/artifacts/{id}/files"))?;
                let existing = current_files["files"]
                    .as_object()
                    .context("server response lacks files")?;
                for path in existing.keys() {
                    if path != "index.html" && !files.contains_key(path) {
                        body["files"][path] = serde_json::Value::Null;
                    }
                }
            }
            body["if_version"] = serde_json::json!(current);
            c.post(&format!("/api/artifacts/{id}/versions"), &body)?
        }
    };
    let id = res["artifact"]["id"]
        .as_str()
        .context("server response lacks artifact.id")?
        .to_string();
    let url = c.browser_url(&format!("/a/{id}"));
    // The session comments go to: the one this version is attributed to, else
    // the artifact's owner. The CLI attributes none of its own.
    let session = res["version"]["session_id"]
        .as_str()
        .or(res["artifact"]["owner_session_id"].as_str());
    super::print(
        cli,
        serde_json::json!({
            "id": id,
            "url": url,
            "version": res["version"]["n"],
            "session": session,
            "note": res["version"]["note"],
            "addressed": res["version"]["addresses"],
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
            if j["session"].is_null() {
                text.push('\n');
                text.push_str(NO_SESSION_NOTE);
            }
            text
        },
    );
    Ok(())
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
