//! The Artifax MCP tool set: nine tools that call the daemon's REST API.

use crate::client::{ClientError, DaemonClient};
use crate::render;
use artifax_core::ArtifactId;
use artifax_core::model::Session;
use base64::Engine;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Default cap on the bytes `read` returns.
pub const DEFAULT_READ_MAX_BYTES: u64 = 200_000;

const INSTRUCTIONS: &str = "Artifax publishes HTML pages (artifacts) to a local server so the person \
can open them in a browser, and keeps every version. Publish a page with `publish` (an HTML string \
or a local file, plus optional supporting files); update it by passing its `id` or `url` with the \
`if_version` returned by the last `publish` or `read`, and on a conflict re-read and merge. Pages \
follow the Artifax page contract described in the artifax skill (a <title>, CSS tokens on :root \
with a dark mode, an explicit body background, phone-width layout). The URLs in results are for \
the person to open; call `open` to show one in their browser.";

/// File extensions published as UTF-8 text when they decode as UTF-8; any other
/// file is sent as base64.
const TEXT_EXT: &[&str] = &[
    "html", "htm", "css", "js", "mjs", "json", "svg", "md", "txt", "csv", "xml", "map",
];

/// How a file argument's `content` encodes its bytes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FileEncoding {
    #[default]
    Utf8,
    Base64,
}

/// A supporting file: exactly one of `path` (a local file) and `content`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileArg {
    /// Local file to read; text files are sent as UTF-8, others as base64.
    pub path: Option<String>,
    /// The file's content, encoded as `encoding` says.
    pub content: Option<String>,
    /// Encoding of `content`: `utf8` (default) or `base64`.
    pub encoding: Option<FileEncoding>,
    /// Content type to serve the file with; inferred from the extension when absent.
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishArgs {
    /// Local HTML file to publish as index.html. Exactly one of `file_path` and `html`.
    pub file_path: Option<String>,
    /// HTML to publish as index.html. Exactly one of `file_path` and `html`.
    pub html: Option<String>,
    /// Supporting files by published path; `null` removes a file carried forward
    /// from the previous version.
    pub files: Option<BTreeMap<String, Option<FileArg>>>,
    /// URL of an artifact to update (instead of creating one).
    pub url: Option<String>,
    /// ID of an artifact to update (instead of creating one).
    pub id: Option<String>,
    /// The version this update is based on; a stale value fails with the current
    /// version. Defaults to the artifact's current version.
    pub if_version: Option<u32>,
    /// Artifact title. Required to create an artifact unless the page has a
    /// non-empty `<title>`, which is used instead; optional on an update.
    pub title: Option<String>,
    /// One-line description.
    pub description: Option<String>,
    /// One generic word for the icon, such as chart or map.
    pub icon: Option<String>,
    /// Short label for this version.
    pub label: Option<String>,
    /// Runtime capabilities the page declares.
    pub capabilities: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    /// Artifact URL or ID. A URL naming a version (`/a/<id>/v/<n>`) selects that
    /// version unless `version` is given.
    pub url_or_id: String,
    /// Published path to read; defaults to index.html.
    pub path: Option<String>,
    /// Version to read; defaults to the URL's version, else the current version.
    pub version: Option<u32>,
    /// Most bytes of content to return (default 200000); longer files are cut
    /// and flagged `truncated`.
    pub max_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Most artifacts to return.
    pub limit: Option<u32>,
    /// `all` (default) lists every artifact; `mine` only those this session created.
    pub scope: Option<ListScope>,
}

/// Which artifacts `list` returns.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ListScope {
    /// Artifacts whose owner is this tool set's session (none without a session).
    Mine,
    /// Every artifact.
    #[default]
    All,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetArgs {
    /// Artifact URL or ID.
    pub url_or_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetUploadArgs {
    /// Artifact URL or ID the assets belong to.
    pub url_or_id: String,
    /// One local file to upload.
    pub file_path: Option<String>,
    /// Several local files to upload.
    pub file_paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusArgs {}

/// A tool's outcome before rendering: the success object, or a finished error result.
type Outcome = Result<Value, CallToolResult>;

fn invalid(message: impl Into<String>) -> CallToolResult {
    render::error("invalid_args", message, json!({}))
}

fn not_found(message: impl Into<String>) -> CallToolResult {
    render::error("not_found", message, json!({}))
}

/// The artifact ID, and the version when the reference names one, in a bare ID
/// or a URL of one of these forms (query and fragment ignored):
/// `.../a/<id>`, `.../a/<id>/v/<n>`, `.../c/<id>/v/<n>/...`, and the per-artifact
/// origin `http://<id>.localhost:<port>/v/<n>/...`.
fn artifact_ref(url_or_id: &str) -> Result<(String, Option<u32>), CallToolResult> {
    let s = url_or_id.trim();
    let s = s.split(['?', '#']).next().unwrap_or("");
    let parse = |c: &str| ArtifactId::parse(c).ok().map(|id| id.as_str().to_string());
    let version_in = |segs: &[&str]| match segs {
        ["v", n, ..] => n.parse::<u32>().ok(),
        _ => None,
    };
    if let Some(id) = parse(s) {
        return Ok((id, None));
    }
    let (host, path) = match s.split_once("://") {
        Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
        None => ("", s),
    };
    let segs: Vec<&str> = path.split('/').filter(|seg| !seg.is_empty()).collect();
    let hostname = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if let Some(id) = hostname.strip_suffix(".localhost").and_then(parse) {
        return Ok((id, version_in(&segs)));
    }
    for (i, seg) in segs.iter().enumerate() {
        if matches!(*seg, "a" | "c")
            && let Some(id) = segs.get(i + 1).and_then(|c| parse(c))
        {
            return Ok((id, version_in(&segs[i + 2..])));
        }
    }
    Err(render::error(
        "invalid_id",
        format!("'{url_or_id}' is not an artifact ID or URL"),
        json!({}),
    ))
}

/// The artifact ID in a reference accepted by [`artifact_ref`].
fn artifact_id(url_or_id: &str) -> Result<String, CallToolResult> {
    artifact_ref(url_or_id).map(|(id, _)| id)
}

fn read_local(path: &Path) -> Result<Vec<u8>, CallToolResult> {
    std::fs::read(path).map_err(|e| {
        render::error(
            "file_unreadable",
            format!("cannot read {}: {e}", path.display()),
            json!({"path": path.to_string_lossy()}),
        )
    })
}

/// A publish file entry for a local file: UTF-8 text for text extensions whose
/// bytes decode, base64 otherwise.
fn file_entry(path: &Path) -> Result<Value, CallToolResult> {
    let bytes = read_local(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    Ok(if TEXT_EXT.contains(&ext.as_str()) {
        match String::from_utf8(bytes) {
            Ok(s) => json!({"content": s, "encoding": "utf8"}),
            Err(e) => json!({"content": b64(e.as_bytes()), "encoding": "base64"}),
        }
    } else {
        json!({"content": b64(&bytes), "encoding": "base64"})
    })
}

/// True for content types `read` returns as text.
fn is_text(content_type: &str) -> bool {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    base.starts_with("text/")
        || matches!(
            base.as_str(),
            "application/json" | "application/javascript" | "image/svg+xml"
        )
}

/// The first `max` bytes of `bytes` as text. A cut through a multi-byte character
/// drops that character; other invalid UTF-8 is replaced with U+FFFD.
fn text_prefix(bytes: &[u8], max: usize) -> String {
    let mut slice = &bytes[..bytes.len().min(max)];
    if let Err(e) = std::str::from_utf8(slice)
        && e.error_len().is_none()
        && slice.len() < bytes.len()
    {
        slice = &slice[..e.valid_up_to()];
    }
    String::from_utf8_lossy(slice).into_owned()
}

/// The MCP tool set. Each instance talks to one daemon and attributes its
/// publishes to a session (the stdio shim) or to no session (the daemon's `/mcp`).
#[derive(Clone)]
pub struct ArtifaxTools {
    client: DaemonClient,
    browser_base: String,
    session: Option<Session>,
    log_path: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl ArtifaxTools {
    /// Tools calling the daemon through `client`. `browser_base` (`http://localhost:<port>`)
    /// prefixes the URLs in results; `log_path` is the daemon log named when the
    /// daemon cannot be reached. A managed client's current browser base and
    /// registered session take precedence over `browser_base` and `session`.
    pub fn new(
        client: DaemonClient,
        browser_base: String,
        session: Option<Session>,
        log_path: PathBuf,
    ) -> ArtifaxTools {
        ArtifaxTools {
            client,
            browser_base: browser_base.trim_end_matches('/').to_string(),
            session,
            log_path,
            tool_router: Self::tool_router(),
        }
    }

    fn session(&self) -> Option<Session> {
        self.client.session().or_else(|| self.session.clone())
    }

    fn browser_base(&self) -> String {
        self.client
            .browser_base()
            .map(|b| b.trim_end_matches('/').to_string())
            .unwrap_or_else(|| self.browser_base.clone())
    }

    fn fail(&self, e: ClientError) -> CallToolResult {
        render::client_error(e, &self.log_path)
    }

    fn artifact_url(&self, id: &str) -> String {
        format!("{}/a/{id}", self.browser_base())
    }

    /// `p` as given when absolute, else joined to the session's working
    /// directory. A relative path with no session (the daemon's `/mcp`), or a
    /// session whose working directory is not known yet, is an `invalid_args`
    /// error: resolving it against the process's own directory could publish
    /// the wrong file.
    fn local_path(&self, p: &str) -> Result<PathBuf, CallToolResult> {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            return Ok(path);
        }
        match self.session() {
            Some(s) if !s.cwd.is_empty() => Ok(Path::new(&s.cwd).join(path)),
            Some(s) => Err(invalid(format!(
                "file paths must be absolute: session {} has no working directory yet to resolve '{p}' against",
                s.id
            ))),
            None => Err(invalid(format!(
                "file paths must be absolute: there is no session working directory to resolve '{p}' against"
            ))),
        }
    }

    fn file_arg(&self, name: &str, f: FileArg) -> Result<Value, CallToolResult> {
        let mut entry = match (f.path, f.content) {
            (Some(p), None) => {
                if f.encoding.is_some() {
                    return Err(invalid(format!(
                        "files.{name}: encoding applies only to content"
                    )));
                }
                file_entry(&self.local_path(&p)?)?
            }
            (None, Some(c)) => json!({"content": c, "encoding": f.encoding.unwrap_or_default()}),
            _ => {
                return Err(invalid(format!(
                    "files.{name}: pass exactly one of path and content"
                )));
            }
        };
        if let Some(ct) = f.content_type {
            entry["content_type"] = json!(ct);
        }
        Ok(entry)
    }

    /// When any of `paths` is relative, registers a managed client's session
    /// first, so they resolve against its working directory. A failure is left
    /// for the daemon call to report.
    async fn prepare_session<'a>(&self, mut paths: impl Iterator<Item = &'a str>) {
        if paths.any(|p| Path::new(p).is_relative()) {
            let _ = self.client.ensure_session().await;
        }
    }

    async fn do_publish(&self, a: PublishArgs) -> Outcome {
        let file_paths = a
            .files
            .iter()
            .flatten()
            .filter_map(|(_, f)| f.as_ref()?.path.as_deref());
        self.prepare_session(a.file_path.as_deref().into_iter().chain(file_paths))
            .await;
        let page = match (&a.file_path, &a.html) {
            (Some(p), None) => file_entry(&self.local_path(p)?)?,
            (None, Some(h)) => json!({"content": h, "encoding": "utf8"}),
            _ => return Err(invalid("pass exactly one of file_path and html")),
        };
        let target = match (&a.id, &a.url) {
            (Some(_), Some(_)) => return Err(invalid("pass at most one of id and url")),
            (Some(t), None) | (None, Some(t)) => Some(artifact_id(t)?),
            (None, None) => None,
        };
        let mut files = Map::new();
        for (name, f) in a.files.unwrap_or_default() {
            if name == artifax_core::publish::INDEX {
                return Err(invalid(
                    "index.html comes from file_path or html, not files",
                ));
            }
            let entry = match f {
                Some(f) => self.file_arg(&name, f)?,
                None => Value::Null,
            };
            files.insert(name, entry);
        }
        let title = match (&target, a.title) {
            (None, None) => Some(
                page["content"]
                    .as_str()
                    .filter(|_| page["encoding"] == "utf8")
                    .and_then(artifax_core::html_title)
                    .ok_or_else(|| {
                        invalid(
                            "a new artifact needs a title: pass `title`, or give the page a non-empty <title>",
                        )
                    })?,
            ),
            (_, t) => t,
        };
        files.insert(artifax_core::publish::INDEX.to_string(), page);
        let mut body = json!({"files": files});
        for (k, v) in [
            ("title", title),
            ("description", a.description),
            ("icon", a.icon),
            ("label", a.label),
        ] {
            if let Some(v) = v {
                body[k] = json!(v);
            }
        }
        if let Some(c) = a.capabilities {
            body["capabilities"] = c;
        }
        let res = match target {
            None => self.client.create(&body).await,
            Some(id) => {
                let current = match a.if_version {
                    Some(v) => v,
                    None => {
                        let got = self.client.get(&id).await.map_err(|e| self.fail(e))?;
                        got["artifact"]["current_version"].as_u64().unwrap_or(0) as u32
                    }
                };
                body["if_version"] = json!(current);
                match self.client.publish_version(&id, &body).await {
                    Err(e @ ClientError::Api { status: 409, .. }) => {
                        return Err(self.conflict(&id, e).await);
                    }
                    r => r,
                }
            }
        }
        .map_err(|e| self.fail(e))?;
        let id = res["artifact"]["id"].as_str().unwrap_or_default();
        let files: Vec<&String> = res["version"]["files"]
            .as_object()
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        Ok(json!({
            "artifact_id": id,
            "url": self.artifact_url(id),
            "version": res["version"]["n"],
            "title": res["artifact"]["title"],
            "files": files,
        }))
    }

    /// The error result for a publish conflict: the daemon's error plus a summary
    /// of the current version and how to merge. When the summary cannot be
    /// fetched, the daemon's error alone.
    async fn conflict(&self, id: &str, e: ClientError) -> CallToolResult {
        let ClientError::Api { status, mut error } = e else {
            return self.fail(e);
        };
        if let Ok(got) = self.client.get(id).await {
            let n = got["artifact"]["current_version"].as_u64().unwrap_or(0);
            if let Some(v) = got["versions"]
                .as_array()
                .and_then(|vs| vs.iter().find(|v| v["n"].as_u64() == Some(n)))
            {
                let files: Vec<&String> = v["files"]
                    .as_object()
                    .map(|m| m.keys().collect())
                    .unwrap_or_default();
                error["current_version"] = json!({
                    "n": n,
                    "label": v["label"],
                    "created_at": v["created_at"],
                    "files": files,
                    "url": format!("{}/v/{n}", self.artifact_url(id)),
                });
                error["hint"] = json!(format!(
                    "read the current version, merge your change, and retry with if_version = {n}"
                ));
            }
        }
        self.fail(ClientError::Api { status, error })
    }

    async fn do_read(&self, a: ReadArgs) -> Outcome {
        let (id, url_version) = artifact_ref(&a.url_or_id)?;
        let got = self.client.get(&id).await.map_err(|e| self.fail(e))?;
        let n = match a.version.or(url_version) {
            Some(n) => n,
            None => got["artifact"]["current_version"].as_u64().unwrap_or(0) as u32,
        };
        let version = got["versions"]
            .as_array()
            .and_then(|vs| vs.iter().find(|v| v["n"].as_u64() == Some(n as u64)))
            .ok_or_else(|| not_found(format!("artifact {id} has no version {n}")))?;
        let path = a
            .path
            .unwrap_or_else(|| artifax_core::publish::INDEX.to_string());
        let meta = version["files"]
            .get(&path)
            .ok_or_else(|| not_found(format!("version {n} of {id} has no file '{path}'")))?;
        let content_type = meta["content_type"].as_str().unwrap_or("").to_string();
        let bytes = self
            .client
            .file_bytes(&id, n, &path)
            .await
            .map_err(|e| self.fail(e))?;
        let size = bytes.len();
        let max =
            usize::try_from(a.max_bytes.unwrap_or(DEFAULT_READ_MAX_BYTES)).unwrap_or(usize::MAX);
        let mut out = json!({
            "artifact_id": id,
            "version": n,
            "path": path,
            "content_type": content_type,
            "truncated": size > max,
            "size": size,
        });
        if is_text(&content_type) {
            out["content"] = json!(text_prefix(&bytes, max));
        } else if size <= max {
            out["content_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
        Ok(out)
    }

    async fn do_list(&self, a: ListArgs) -> Outcome {
        let res = self.client.list().await.map_err(|e| self.fail(e))?;
        let session_id = self.session().map(|s| s.id);
        let limit = a.limit.map_or(usize::MAX, |l| l as usize);
        let mine = |x: &&Value| match a.scope.unwrap_or_default() {
            ListScope::All => true,
            ListScope::Mine => session_id
                .as_deref()
                .is_some_and(|s| x["owner_session_id"].as_str() == Some(s)),
        };
        let artifacts: Vec<Value> = res["artifacts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(mine)
            .take(limit)
            .map(|x| {
                let id = x["id"].as_str().unwrap_or_default();
                json!({
                    "id": id,
                    "url": self.artifact_url(id),
                    "title": x["title"],
                    "version": x["current_version"],
                    "pinned": x["pinned"],
                    "updated_at": x["updated_at"],
                    "owner_session_id": x["owner_session_id"],
                })
            })
            .collect();
        Ok(json!({"artifacts": artifacts}))
    }

    async fn do_delete(&self, a: TargetArgs) -> Outcome {
        let id = artifact_id(&a.url_or_id)?;
        self.client.delete(&id).await.map_err(|e| self.fail(e))?;
        Ok(json!({"artifact_id": id, "deleted": true}))
    }

    async fn set_pinned(&self, a: TargetArgs, pinned: bool) -> Outcome {
        let id = artifact_id(&a.url_or_id)?;
        let res = self
            .client
            .patch(&id, &json!({"pinned": pinned}))
            .await
            .map_err(|e| self.fail(e))?;
        Ok(json!({"artifact_id": id, "pinned": res["artifact"]["pinned"]}))
    }

    async fn do_open(&self, a: TargetArgs) -> Outcome {
        let id = artifact_id(&a.url_or_id)?;
        self.client.get(&id).await.map_err(|e| self.fail(e))?;
        let url = self.artifact_url(&id);
        let opened = std::env::var_os("ARTIFAX_NO_OPEN").is_none() && {
            let url = url.clone();
            tokio::task::spawn_blocking(move || open_in_browser(&url))
                .await
                .unwrap_or(false)
        };
        Ok(json!({"url": url, "opened": opened}))
    }

    async fn do_asset_upload(&self, a: AssetUploadArgs) -> Outcome {
        let id = artifact_id(&a.url_or_id)?;
        self.prepare_session(
            a.file_path
                .iter()
                .chain(a.file_paths.iter().flatten())
                .map(String::as_str),
        )
        .await;
        let paths: Vec<String> = a
            .file_path
            .into_iter()
            .chain(a.file_paths.unwrap_or_default())
            .collect();
        if paths.is_empty() {
            return Err(invalid("pass file_path or file_paths"));
        }
        let mut files = Vec::with_capacity(paths.len());
        for p in &paths {
            let path = self.local_path(p)?;
            let bytes = read_local(&path)?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string());
            files.push((name, bytes));
        }
        let mut assets = Vec::with_capacity(files.len());
        for (name, bytes) in files {
            let content_type = artifax_core::publish::content_type_for(&name);
            let res = self
                .client
                .upload_asset(&id, &name, &content_type, bytes)
                .await
                .map_err(|e| self.fail(e))?;
            let asset = &res["asset"];
            assets.push(json!({
                "id": asset["id"],
                "url": format!("{}{}", self.browser_base(), res["url"].as_str().unwrap_or_default()),
                "content_type": asset["content_type"],
                "size": asset["size"],
            }));
        }
        Ok(json!({"assets": assets}))
    }

    async fn do_status(&self) -> Outcome {
        let h = self.client.healthz().await.map_err(|e| self.fail(e))?;
        let session = self.session();
        let mut out = json!({
            "daemon_url": self.browser_base(),
            "version": h["version"],
            "harness": session.as_ref().map(|s| &s.harness),
            "session": session,
            "watches": [],
        });
        // Version skew between these tools and the daemon they call.
        if h["version"].as_str() != Some(env!("CARGO_PKG_VERSION")) {
            out["daemon_version"] = h["version"].clone();
        }
        Ok(out)
    }
}

/// How long [`open_in_browser`] waits for the opener to exit.
pub const OPEN_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Runs the platform opener (`open` on macOS, `xdg-open` elsewhere) on `url`
/// with stdio detached and waits up to [`OPEN_WAIT`] for it. True when it
/// exits successfully in time, or is still running then (best effort: some
/// openers hand off and linger; it is reaped in the background); false when it
/// cannot start or exits unsuccessfully. Blocks the calling thread.
pub fn open_in_browser(url: &str) -> bool {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let Ok(mut child) = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + OPEN_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return true;
            }
            Err(_) => return false,
        }
    }
}

fn finish(o: Outcome) -> Result<CallToolResult, McpError> {
    Ok(match o {
        Ok(v) => render::success(v),
        Err(e) => e,
    })
}

#[tool_router]
impl ArtifaxTools {
    #[tool(
        description = "Publish an HTML page as a new artifact, or as a new version of an existing one (pass `id` or `url`, with `if_version`). Give the page as `html` or `file_path`, plus optional supporting `files`. Returns the artifact ID, its URL for the person, and the new version number."
    )]
    pub async fn publish(
        &self,
        Parameters(args): Parameters<PublishArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_publish(args).await)
    }

    #[tool(
        description = "Read a published file (index.html by default) of an artifact's current or given version, as stored, before serve-time wrapping. Text is cut at `max_bytes` (default 200000) with `truncated: true`; binary files come back as `content_base64` when under the cap."
    )]
    pub async fn read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_read(args).await)
    }

    #[tool(
        description = "List artifacts, pinned first and then most recently updated, with their URLs and current versions. `scope: mine` lists only those this session created."
    )]
    pub async fn list(
        &self,
        Parameters(args): Parameters<ListArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_list(args).await)
    }

    #[tool(description = "Delete an artifact and all its versions.")]
    pub async fn delete(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_delete(args).await)
    }

    #[tool(description = "Open an artifact in the person's browser on this machine.")]
    pub async fn open(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_open(args).await)
    }

    #[tool(description = "Pin an artifact to the top of the gallery.")]
    pub async fn pin(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.set_pinned(args, true).await)
    }

    #[tool(description = "Unpin an artifact.")]
    pub async fn unpin(
        &self,
        Parameters(args): Parameters<TargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.set_pinned(args, false).await)
    }

    #[tool(
        description = "Upload local files (images, video, fonts, data) as assets of an artifact. Returns each asset's URL for the page to reference."
    )]
    pub async fn asset_upload(
        &self,
        Parameters(args): Parameters<AssetUploadArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_asset_upload(args).await)
    }

    #[tool(
        description = "Report the Artifax daemon's URL and version and the session publishes are attributed to."
    )]
    pub async fn status(
        &self,
        Parameters(_args): Parameters<StatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.do_status().await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ArtifaxTools {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS);
        config.server_info = Implementation::new("artifax", env!("CARGO_PKG_VERSION"));
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_ref_reads_versions_from_every_url_form() {
        let id = "7q3k9mzx2b4t";
        for (s, v) in [
            (format!("http://localhost:7480/a/{id}"), None),
            (format!("/a/{id}"), None),
            (format!("http://localhost:7480/a/{id}/v/3"), Some(3)),
            (format!("http://127.0.0.1:7480/c/{id}/v/2/"), Some(2)),
            (
                format!("http://127.0.0.1:7480/c/{id}/v/2/img/a.png"),
                Some(2),
            ),
            (format!("http://{id}.localhost:7480/v/4/"), Some(4)),
            (format!("http://{id}.localhost/v/5/app.js?x#y"), Some(5)),
            (format!("http://{id}.localhost:7480/"), None),
        ] {
            assert_eq!(artifact_ref(&s).unwrap(), (id.to_string(), v), "{s}");
        }
        for bad in [
            "http://localhost:7480/c/nope/v/1/",
            "http://evil.localhost:7480/v/1/",
            "http://localhost:7480/x/7q3k9mzx2b4t",
        ] {
            assert!(artifact_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn artifact_id_accepts_ids_and_urls() {
        let id = "7q3k9mzx2b4t";
        for s in [
            id.to_string(),
            format!("http://localhost:7480/a/{id}"),
            format!("http://localhost:7480/a/{id}/v/2?x=1"),
            format!(" {id} "),
        ] {
            assert_eq!(artifact_id(&s).unwrap(), id, "{s}");
        }
        assert!(artifact_id("http://localhost/a/").is_err());
        assert!(artifact_id("nope").is_err());
    }

    #[test]
    fn text_prefix_never_splits_a_character() {
        let s = "aé".as_bytes();
        assert_eq!(text_prefix(s, 2), "a");
        assert_eq!(text_prefix(s, 3), "aé");
        assert_eq!(text_prefix(&[b'a', 0xff, b'b'], 10), "a\u{FFFD}b");
    }

    #[test]
    fn text_types() {
        for t in [
            "text/html; charset=utf-8",
            "application/json",
            "application/javascript",
            "image/svg+xml",
        ] {
            assert!(is_text(t), "{t}");
        }
        for t in ["image/png", "application/octet-stream", "font/woff2"] {
            assert!(!is_text(t), "{t}");
        }
    }
}
