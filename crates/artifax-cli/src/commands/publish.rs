use crate::client::Client;
use artifax_core::Home;
use base64::Engine;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// The page to publish as index.html.
    pub index: PathBuf,
    /// Extra file, optionally renamed: path or path=published/name. Repeatable.
    #[arg(long = "file")]
    pub files: Vec<String>,
    /// Include every file under this directory (except index.html) at its relative path.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Update this artifact instead of creating one.
    #[arg(long, conflicts_with = "url")]
    pub id: Option<String>,
    /// Update the artifact at this URL instead of creating one.
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub icon: Option<String>,
    #[arg(long)]
    pub label: Option<String>,
    /// Expected current version; defaults to the artifact's current version.
    #[arg(long)]
    pub if_version: Option<u32>,
}

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
        let p = entry?.path();
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

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let mut files = serde_json::Map::new();
    files.insert("index.html".into(), file_entry(&a.index)?);
    if let Some(dir) = &a.dir {
        collect_dir(dir, dir, &mut files)?;
    }
    for spec in &a.files {
        let (src, dest) = match spec.split_once('=') {
            Some((s, d)) => (PathBuf::from(s), d.to_string()),
            None => (
                PathBuf::from(spec),
                Path::new(spec)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            ),
        };
        files.insert(dest, file_entry(&src)?);
    }
    let c = Client::connect(home, cli.port)?;
    let target = match (&a.id, &a.url) {
        (Some(id), _) => Some(super::open::id_from(id)?),
        (_, Some(u)) => Some(super::open::id_from(u)?),
        _ => None,
    };
    let mut body = serde_json::json!({"files": files});
    for (k, v) in [
        ("title", &a.title),
        ("description", &a.description),
        ("icon", &a.icon),
        ("label", &a.label),
    ] {
        if let Some(v) = v {
            body[k] = serde_json::json!(v);
        }
    }
    let res = match target {
        None => c.post("/api/artifacts", &body)?,
        Some(id) => {
            let current = match a.if_version {
                Some(v) => v,
                None => c.get(&format!("/api/artifacts/{id}"))?["artifact"]["current_version"]
                    .as_u64()
                    .unwrap() as u32,
            };
            body["if_version"] = serde_json::json!(current);
            c.post(&format!("/api/artifacts/{id}/versions"), &body)?
        }
    };
    let id = res["artifact"]["id"].as_str().unwrap().to_string();
    let url = c.browser_url(&format!("/a/{id}"));
    super::print(
        cli,
        serde_json::json!({"id": id, "url": url, "version": res["version"]["n"]}),
        |j| {
            format!(
                "published v{} at {}",
                j["version"],
                j["url"].as_str().unwrap()
            )
        },
    );
    Ok(())
}
