# Artifax Phase 1: Daemon, Publish, Versions, Gallery, Viewer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single `artifax` binary that runs a local daemon, publishes versioned HTML artifacts with supporting files and assets from the CLI, and shows them in a browser gallery and viewer with live "new version" banners.

**Architecture:** Rust workspace with three crates: `artifax-core` (IDs, paths, SQLite store, publish validation, document wrapping, events), `artifax-server` (axum HTTP + SSE, daemon lifecycle, embedded web UI), `artifax-cli` (the `artifax` binary and its HTTP client). The web UI is a Preact shell plus a vanilla bridge script, built by Vite into `web/dist` and embedded with rust-embed. Content is served per artifact at `<id>.localhost:<port>` when the browser resolves it, otherwise in an opaque-origin sandboxed iframe.

**Tech Stack:** Rust 1.85+ edition 2024, axum 0.8, tokio, rusqlite (bundled), rust-embed, clap 4, serde/serde_json, chrono, ulid, rand, mime_guess, reqwest (blocking, CLI), libc, assert_cmd; TypeScript, Vite 6, Preact 10, vitest, Playwright.

**Spec:** `docs/superpowers/specs/2026-09-28-artifax-design.md` (sections 4–8, 9 "recognition rule", 14, 15, 17 "Phase 1")

## Global Constraints

- Rust edition 2024; `cargo clippy --workspace -- -D warnings` must pass.
- Storage root is `~/.artifax/`, overridable with `ARTIFAX_HOME`. Every test sets `ARTIFAX_HOME` to a temp dir; no test touches the real home.
- Default bind `127.0.0.1`, default port `7480`, then the next 20 ports if busy.
- `daemon.json` is written atomically with mode `0600` and contains `{port, pid, token, started_at, bind, version}`.
- Write routes require `Authorization: Bearer <token>`; `/api/token` answers only to loopback peers.
- Artifact IDs are 12 lowercase Crockford base32 characters (alphabet `0123456789abcdefghjkmnpqrstvwxyz`) from 60 random bits. Other IDs are ULIDs. Versions are integers from 1.
- `index.html` is required on every publish and never carried forward; other files carry forward unless given or `null`. Single file cap 16 MiB, body cap 64 MiB, `label` at most 60 characters.
- Recognition rule: a page beginning (after whitespace) with `<!doctype html>` case-insensitively is a complete document and gets the bridge script inserted after the first `<body ...>` tag; anything else is wrapped in the skeleton.
- Bridge script tag: `<script src="/_artifax/bridge.js" data-artifact="<aid>" data-version="<n>" data-contract="0.2.61"></script>`.
- Phase 1 has no comments, sessions, capabilities, room, or sample. `window.claude.use(name)` resolves `null` for every name. The shell has no comment affordances.
- JSON errors have the shape `{"error": {"code": "<snake_case>", "message": "<text>"}}`.
- Doc comments and commit messages describe the contract or the change, never the conversation.
- "ID" is the spelling in prose and UI copy; `id` only as a code symbol.

## Review Focus

1. Publish file paths like `../x`, `/etc/passwd`, `a\\b`, or an empty segment must be rejected with HTTP 400 `invalid_path` and write nothing. Pinned in Task 3.
2. A Host header like `evil.localhost`, `7q3k9mzx2b4t.localhost.attacker.com`, or `7Q3K9MZX2B4T.localhost` must not route to content; only an anchored 12-char lowercase ID plus `.localhost` with optional port does. Pinned in Task 10.
3. A republish whose `if_version` is stale must return 409 `conflict` naming the current version and must not create a version directory. Pinned in Task 3 and Task 8.
4. Two processes auto-starting the daemon at once must end with exactly one daemon and one valid `daemon.json`; the loser exits 0 having found the winner. Pinned in Task 12.
5. A published page reading `<!DOCTYPE HTML><html><body class="x">…` must be served with the bridge tag directly after `<body class="x">`, and a page with no `<body` tag but a doctype must still get the tag inserted right after the doctype. Pinned in Task 5.
6. A `utf8`-encoded file whose content is not valid UTF-8 after JSON decoding cannot occur (JSON strings are UTF-8), but a `base64` file with invalid base64 must return 400 `invalid_encoding`, not 500. Pinned in Task 3.

---

## File structure

```
Cargo.toml                                  workspace
rust-toolchain.toml                         channel 1.94.0 (matches toolpath)
clippy.toml, .gitignore, justfile, README.md
scripts/quality_gates.sh
.github/workflows/ci.yml
crates/artifax-core/
  Cargo.toml
  src/lib.rs            re-exports
  src/error.rs          CoreError, Result
  src/ids.rs            ArtifactId, new_ulid
  src/home.rs           Home: every path under ~/.artifax
  src/model.rs          Artifact, Version, FileMeta, Asset (serde)
  src/publish.rs        PublishRequest → ValidatedPublish (paths, sizes, encodings)
  src/store/mod.rs      Store (rusqlite + Mutex), open + migrate
  src/store/migrations.rs
  src/store/artifacts.rs create, publish_version (copy-forward), get, list, pin, delete, file_path
  src/store/assets.rs   add, get, list, delete
  src/wrap.rs           wrap_document + recognition rule
  src/events.rs         Event enum + broadcast
crates/artifax-server/
  Cargo.toml
  src/lib.rs            build_router, run
  src/state.rs          AppState
  src/error.rs          ApiError → JSON response
  src/auth.rs           RequireToken extractor, is_loopback
  src/host.rs           per-artifact Host rewrite middleware
  src/routes/mod.rs     router assembly
  src/routes/health.rs
  src/routes/token.rs
  src/routes/artifacts.rs
  src/routes/assets.rs
  src/routes/content.rs
  src/routes/events.rs
  src/routes/shell.rs   embedded UI
  src/daemon.rs         DaemonInfo, write/read, pid_alive, stale watcher, shutdown
  tests/common/mod.rs   spawn a test server on port 0 with temp home
  tests/api_artifacts.rs, tests/api_assets.rs, tests/api_content.rs, tests/api_events.rs, tests/api_auth.rs
crates/artifax-cli/
  Cargo.toml
  src/main.rs           clap tree
  src/client.rs         Client: discovery, auto-start, REST calls
  src/commands/{serve,stop,status,publish,list,open,delete,pin,doctor}.rs
  tests/cli.rs          assert_cmd against a temp home
web/
  package.json, tsconfig.json, vite.shell.config.ts, vite.bridge.config.ts, playwright.config.ts
  shell/index.html
  shell/src/main.tsx, api.ts, events.ts, gallery.tsx, artifact.tsx, frame.tsx, theme.css
  bridge/src/bridge.ts
  bridge/test/bridge.test.ts
  e2e/viewer.spec.ts
  dist/.gitkeep                             build output, embedded by the server
```

---

### Task 1: Workspace scaffold, IDs, and home paths

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `clippy.toml`, `.gitignore`, `crates/artifax-core/Cargo.toml`, `crates/artifax-core/src/lib.rs`, `crates/artifax-core/src/error.rs`, `crates/artifax-core/src/ids.rs`, `crates/artifax-core/src/home.rs`

**Interfaces:**
- Produces: `ArtifactId::generate() -> ArtifactId`, `ArtifactId::parse(&str) -> Result<ArtifactId>`, `ArtifactId::as_str(&self) -> &str`, `new_ulid() -> String`, `Home::from_env() -> Home`, `Home::at(PathBuf) -> Home`, `Home::root/db_path/daemon_json/daemon_lock/log_path/artifact_dir/version_dir/assets_dir/ensure_dirs`, `CoreError` with variants `NotFound`, `Conflict { current: u32 }`, `Invalid { code: &'static str, message: String }`, `Io(std::io::Error)`, `Db(rusqlite::Error)`, and `pub type Result<T> = std::result::Result<T, CoreError>`.

- [ ] **Step 1: Write the workspace manifests**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/artifax-core", "crates/artifax-server", "crates/artifax-cli"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
repository = "https://github.com/empathic/artifax"

[workspace.dependencies]
anyhow = "1"
axum = { version = "0.8", features = ["multipart", "macros"] }
base64 = "0.22"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive", "env"] }
libc = "0.2"
mime_guess = "2"
rand = "0.9"
reqwest = { version = "0.12", features = ["blocking", "json", "multipart"], default-features = false }
rusqlite = { version = "0.32", features = ["bundled"] }
rust-embed = { version = "8", features = ["include-exclude"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tokio-stream = { version = "0.1", features = ["sync"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
ulid = "1"
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.94.0"
components = ["rustfmt", "clippy"]
```

`clippy.toml`:
```toml
too-many-arguments-threshold = 8
```

`.gitignore`:
```
target/
node_modules/
web/dist/*
!web/dist/.gitkeep
web/playwright-report/
web/test-results/
.DS_Store
```

`crates/artifax-core/Cargo.toml`:
```toml
[package]
name = "artifax-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
base64.workspace = true
chrono.workspace = true
mime_guess.workspace = true
rand.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["sync"] }
ulid.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Write the failing tests for IDs and home paths**

`crates/artifax-core/src/ids.rs` (tests at the bottom of the same file):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_12_crockford_chars() {
        for _ in 0..100 {
            let id = ArtifactId::generate();
            assert_eq!(id.as_str().len(), 12);
            assert!(id.as_str().bytes().all(|b| ALPHABET.contains(&b)), "{id:?}");
        }
    }

    #[test]
    fn parse_rejects_bad_ids() {
        assert!(ArtifactId::parse("7q3k9mzx2b4t").is_ok());
        assert!(ArtifactId::parse("7Q3K9MZX2B4T").is_err(), "uppercase");
        assert!(ArtifactId::parse("7q3k9mzx2b4").is_err(), "short");
        assert!(ArtifactId::parse("7q3k9mzx2b4tu").is_err(), "long");
        assert!(ArtifactId::parse("7q3k9mzx2b4i").is_err(), "i not in alphabet");
    }

    #[test]
    fn ulids_are_26_chars_and_unique() {
        let a = new_ulid();
        let b = new_ulid();
        assert_eq!(a.len(), 26);
        assert_ne!(a, b);
    }
}
```

`crates/artifax-core/src/home.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ArtifactId;

    #[test]
    fn paths_hang_off_root() {
        let home = Home::at("/tmp/ax".into());
        let id = ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        assert_eq!(home.db_path(), PathBuf::from("/tmp/ax/artifax.db"));
        assert_eq!(home.daemon_json(), PathBuf::from("/tmp/ax/daemon.json"));
        assert_eq!(home.daemon_lock(), PathBuf::from("/tmp/ax/daemon.lock"));
        assert_eq!(home.log_path(), PathBuf::from("/tmp/ax/logs/daemon.log"));
        assert_eq!(home.artifact_dir(&id), PathBuf::from("/tmp/ax/artifacts/7q3k9mzx2b4t"));
        assert_eq!(home.version_dir(&id, 3), PathBuf::from("/tmp/ax/artifacts/7q3k9mzx2b4t/versions/3"));
        assert_eq!(home.assets_dir(&id), PathBuf::from("/tmp/ax/artifacts/7q3k9mzx2b4t/assets"));
    }

    #[test]
    fn from_env_prefers_artifax_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_env_with(Some(dir.path().to_str().unwrap()), "/never");
        assert_eq!(home.root(), dir.path());
        let home = Home::from_env_with(None, "/home/x");
        assert_eq!(home.root(), Path::new("/home/x/.artifax"));
    }

    #[test]
    fn ensure_dirs_creates_root_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        assert!(home.root().is_dir());
        assert!(home.root().join("logs").is_dir());
        assert!(home.root().join("artifacts").is_dir());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p artifax-core`
Expected: compile error, `ArtifactId` and `Home` not defined.

- [ ] **Step 4: Implement error, ids, home, lib**

`crates/artifax-core/src/error.rs`:
```rust
//! Error type shared by every core operation.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("not found")]
    NotFound,
    #[error("version conflict: current version is {current}")]
    Conflict { current: u32 },
    #[error("{message}")]
    Invalid { code: &'static str, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

impl CoreError {
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        CoreError::Invalid { code, message: message.into() }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
```

`crates/artifax-core/src/ids.rs`:
```rust
//! Identifiers: 12-character Crockford base32 artifact IDs and ULIDs for everything else.

use crate::error::{CoreError, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Lowercase Crockford base32 without i, l, o, u.
pub const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
pub const ARTIFACT_ID_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ArtifactId(String);

impl ArtifactId {
    /// 60 random bits rendered as 12 base32 characters.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 8];
        rand::rng().fill_bytes(&mut bytes);
        let mut n = u64::from_le_bytes(bytes);
        let mut out = String::with_capacity(ARTIFACT_ID_LEN);
        for _ in 0..ARTIFACT_ID_LEN {
            out.push(ALPHABET[(n & 31) as usize] as char);
            n >>= 5;
        }
        ArtifactId(out)
    }

    pub fn parse(s: &str) -> Result<Self> {
        let ok = s.len() == ARTIFACT_ID_LEN && s.bytes().all(|b| ALPHABET.contains(&b));
        if ok {
            Ok(ArtifactId(s.to_string()))
        } else {
            Err(CoreError::invalid("invalid_id", format!("'{s}' is not an artifact ID")))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ArtifactId {
    type Error = CoreError;
    fn try_from(s: String) -> Result<Self> {
        ArtifactId::parse(&s)
    }
}

impl From<ArtifactId> for String {
    fn from(id: ArtifactId) -> String {
        id.0
    }
}

pub fn new_ulid() -> String {
    ulid::Ulid::new().to_string()
}
```

`crates/artifax-core/src/home.rs`:
```rust
//! Layout of the `~/.artifax` directory.

use crate::ids::ArtifactId;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn at(root: PathBuf) -> Self {
        Home { root }
    }

    /// `$ARTIFAX_HOME`, else `$HOME/.artifax`.
    pub fn from_env() -> Self {
        let ax = std::env::var("ARTIFAX_HOME").ok();
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self::from_env_with(ax.as_deref(), &home)
    }

    pub fn from_env_with(artifax_home: Option<&str>, home: &str) -> Self {
        match artifax_home.filter(|s| !s.is_empty()) {
            Some(p) => Home::at(PathBuf::from(p)),
            None => Home::at(Path::new(home).join(".artifax")),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join("artifax.db")
    }
    pub fn daemon_json(&self) -> PathBuf {
        self.root.join("daemon.json")
    }
    pub fn daemon_lock(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }
    pub fn log_path(&self) -> PathBuf {
        self.root.join("logs").join("daemon.log")
    }
    pub fn artifact_dir(&self, id: &ArtifactId) -> PathBuf {
        self.root.join("artifacts").join(id.as_str())
    }
    pub fn version_dir(&self, id: &ArtifactId, n: u32) -> PathBuf {
        self.artifact_dir(id).join("versions").join(n.to_string())
    }
    pub fn assets_dir(&self, id: &ArtifactId) -> PathBuf {
        self.artifact_dir(id).join("assets")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.root.join("logs"))?;
        std::fs::create_dir_all(self.root.join("artifacts"))?;
        Ok(())
    }
}
```

`crates/artifax-core/src/lib.rs`:
```rust
//! Core types and storage for Artifax.

pub mod error;
pub mod home;
pub mod ids;

pub use error::{CoreError, Result};
pub use home::Home;
pub use ids::{ArtifactId, new_ulid};
```

Create empty crates so the workspace compiles: `crates/artifax-server/Cargo.toml` and `crates/artifax-cli/Cargo.toml` with only `[package]` sections (name, `version.workspace = true`, `edition.workspace = true`, `license.workspace = true`) and `src/lib.rs` / `src/main.rs` containing a doc comment and, for the CLI, `fn main() {}`. Task 7 and Task 14 fill them in.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p artifax-core && cargo clippy --workspace -- -D warnings`
Expected: 6 tests pass, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Scaffold workspace with core IDs and home layout"
```

---

### Task 2: Store with migrations and artifact metadata

**Files:**
- Create: `crates/artifax-core/src/model.rs`, `crates/artifax-core/src/store/mod.rs`, `crates/artifax-core/src/store/migrations.rs`, `crates/artifax-core/src/store/artifacts.rs`
- Modify: `crates/artifax-core/src/lib.rs`

**Interfaces:**
- Produces: `Store::open(&Home) -> Result<Store>`, `Store::get_artifact(&ArtifactId) -> Result<Option<Artifact>>`, `Store::list_artifacts() -> Result<Vec<Artifact>>` (pinned first, then `updated_at` descending, excluding deleted), `Store::set_pinned(&ArtifactId, bool) -> Result<Artifact>`, `Store::update_meta(&ArtifactId, MetaPatch) -> Result<Artifact>`, `Store::delete_artifact(&ArtifactId) -> Result<()>` (soft-deletes the row, removes `artifact_dir`), `Store::now() -> String` (RFC 3339 UTC, milliseconds). Structs `Artifact`, `Version`, `FileMeta`, `Asset`, `MetaPatch { title, description, icon, pinned }` all `Option`s.
- Creation and versions land in Task 3, since they need publish validation. This task inserts rows through a test-only helper `Store::insert_artifact_for_test`.

- [ ] **Step 1: Write the failing tests**

`crates/artifax-core/src/store/artifacts.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use crate::{Home, Store};
    use crate::store::artifacts::MetaPatch;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = Store::open(&home).unwrap();
        (dir, store)
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        Store::open(&home).unwrap();
        Store::open(&home).unwrap();
        assert!(home.db_path().exists());
    }

    #[test]
    fn list_orders_pinned_first_then_recent() {
        let (_d, store) = store();
        let a = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let b = store.insert_artifact_for_test("B", "2026-01-02T00:00:00.000Z");
        let c = store.insert_artifact_for_test("C", "2026-01-03T00:00:00.000Z");
        store.set_pinned(&a, true).unwrap();
        let titles: Vec<String> = store.list_artifacts().unwrap().into_iter().map(|x| x.title).collect();
        assert_eq!(titles, vec!["A", "C", "B"]);
        let _ = (b, c);
    }

    #[test]
    fn update_meta_patches_only_given_fields() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let a = store.update_meta(&id, MetaPatch { description: Some("d".into()), ..Default::default() }).unwrap();
        assert_eq!(a.title, "A");
        assert_eq!(a.description.as_deref(), Some("d"));
        assert!(!a.pinned);
    }

    #[test]
    fn delete_hides_from_get_and_list_and_removes_dir() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let dir = store.home().artifact_dir(&id);
        std::fs::create_dir_all(&dir).unwrap();
        store.delete_artifact(&id).unwrap();
        assert!(store.get_artifact(&id).unwrap().is_none());
        assert!(store.list_artifacts().unwrap().is_empty());
        assert!(!dir.exists());
        assert!(matches!(store.delete_artifact(&id), Err(crate::CoreError::NotFound)));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p artifax-core store`
Expected: compile error, `Store` not defined.

- [ ] **Step 3: Implement model, migrations, store, artifacts**

`crates/artifax-core/src/model.rs`:
```rust
//! Serialisable records returned by the store and the HTTP API.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub current_version: u32,
    pub pinned: bool,
    pub capabilities: serde_json::Value,
    pub contract_version: String,
    pub owner_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub content_type: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub artifact_id: String,
    pub n: u32,
    pub label: Option<String>,
    pub created_at: String,
    pub session_id: Option<String>,
    pub files: BTreeMap<String, FileMeta>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub artifact_id: String,
    pub content_type: String,
    pub size: u64,
    pub ext: String,
    pub created_at: String,
}

pub const CONTRACT_VERSION: &str = "0.2.61";
```

`crates/artifax-core/src/store/migrations.rs`:
```rust
//! Schema versions applied in order on `Store::open`.

pub const MIGRATIONS: &[&str] = &[
    // 1: phase 1 tables
    "CREATE TABLE artifacts (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        description TEXT,
        icon TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        current_version INTEGER NOT NULL DEFAULT 0,
        owner_session_id TEXT,
        pinned INTEGER NOT NULL DEFAULT 0,
        capabilities_json TEXT NOT NULL DEFAULT '{}',
        contract_version TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE TABLE versions (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        n INTEGER NOT NULL,
        label TEXT,
        created_at TEXT NOT NULL,
        session_id TEXT,
        files_json TEXT NOT NULL,
        PRIMARY KEY (artifact_id, n)
    );
    CREATE TABLE assets (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        content_type TEXT NOT NULL,
        size INTEGER NOT NULL,
        ext TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX assets_by_artifact ON assets(artifact_id);",
];
```

`crates/artifax-core/src/store/mod.rs`:
```rust
//! SQLite-backed store. One connection behind a mutex; every public method is a
//! single transaction.

pub mod artifacts;
pub mod migrations;

use crate::{Home, Result};
use rusqlite::Connection;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
    home: Home,
}

impl Store {
    pub fn open(home: &Home) -> Result<Store> {
        home.ensure_dirs()?;
        let conn = Connection::open(home.db_path())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let store = Store { conn: Mutex::new(conn), home: home.clone() };
        store.migrate()?;
        Ok(store)
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn now() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in migrations::MIGRATIONS.iter().enumerate() {
            let target = i as u32 + 1;
            if target > version {
                conn.execute_batch(sql)?;
                conn.pragma_update(None, "user_version", target)?;
            }
        }
        Ok(())
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }

    pub(crate) fn with_tx<T>(&self, f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}
```

`crates/artifax-core/src/store/artifacts.rs`:
```rust
//! Artifact metadata: read, patch, pin, delete. Creation and versions live in
//! `publish.rs` because they need validated input.

use super::Store;
use crate::model::{Artifact, CONTRACT_VERSION};
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, Row, params};

#[derive(Debug, Default, Clone)]
pub struct MetaPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub pinned: Option<bool>,
}

pub(crate) fn row_to_artifact(r: &Row<'_>) -> rusqlite::Result<Artifact> {
    let caps: String = r.get("capabilities_json")?;
    Ok(Artifact {
        id: r.get("id")?,
        title: r.get("title")?,
        description: r.get("description")?,
        icon: r.get("icon")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        current_version: r.get("current_version")?,
        pinned: r.get::<_, i64>("pinned")? != 0,
        capabilities: serde_json::from_str(&caps).unwrap_or(serde_json::json!({})),
        contract_version: r.get("contract_version")?,
        owner_session_id: r.get("owner_session_id")?,
    })
}

const SELECT: &str = "SELECT id, title, description, icon, created_at, updated_at, current_version,
    owner_session_id, pinned, capabilities_json, contract_version FROM artifacts";

impl Store {
    pub fn get_artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                &format!("{SELECT} WHERE id = ?1 AND deleted_at IS NULL"),
                params![id.as_str()],
                row_to_artifact,
            )
            .optional()?)
        })
    }

    pub fn list_artifacts(&self) -> Result<Vec<Artifact>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT} WHERE deleted_at IS NULL ORDER BY pinned DESC, updated_at DESC"
            ))?;
            let rows = stmt.query_map([], row_to_artifact)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn set_pinned(&self, id: &ArtifactId, pinned: bool) -> Result<Artifact> {
        self.update_meta(id, MetaPatch { pinned: Some(pinned), ..Default::default() })
    }

    pub fn update_meta(&self, id: &ArtifactId, patch: MetaPatch) -> Result<Artifact> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE artifacts SET
                    title = COALESCE(?2, title),
                    description = COALESCE(?3, description),
                    icon = COALESCE(?4, icon),
                    pinned = COALESCE(?5, pinned)
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![id.as_str(), patch.title, patch.description, patch.icon, patch.pinned.map(|b| b as i64)],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(tx.query_row(&format!("{SELECT} WHERE id = ?1"), params![id.as_str()], row_to_artifact)?)
        })
    }

    pub fn delete_artifact(&self, id: &ArtifactId) -> Result<()> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE artifacts SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id.as_str(), Store::now()],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(())
        })?;
        let dir = self.home.artifact_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn insert_artifact_for_test(&self, title: &str, at: &str) -> ArtifactId {
        let id = ArtifactId::generate();
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                 VALUES (?1, ?2, ?3, ?3, 1, ?4)",
                params![id.as_str(), title, at, CONTRACT_VERSION],
            )?;
            Ok(())
        })
        .unwrap();
        id
    }
}
```

Add to `lib.rs`:
```rust
pub mod model;
pub mod store;
pub use store::Store;
pub use store::artifacts::MetaPatch;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p artifax-core && cargo clippy --workspace -- -D warnings`
Expected: all pass. (`insert_artifact_for_test` is `#[doc(hidden)]` but public so the server's integration tests can use it too.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Add SQLite store with artifact metadata and migrations"
```

---

### Task 3: Publish validation, artifact creation, and versions with copy-forward

**Files:**
- Create: `crates/artifax-core/src/publish.rs`
- Modify: `crates/artifax-core/src/store/artifacts.rs`, `crates/artifax-core/src/lib.rs`

**Interfaces:**
- Consumes: `Store`, `Home`, `ArtifactId`, `Artifact`, `Version`, `FileMeta`, `CoreError`.
- Produces:
  - `PublishRequest { title: Option<String>, description: Option<String>, icon: Option<String>, label: Option<String>, if_version: Option<u32>, capabilities: Option<serde_json::Value>, files: BTreeMap<String, Option<FileInput>> }` (Deserialize)
  - `FileInput { content: String, encoding: Encoding (default Utf8), content_type: Option<String> }`, `Encoding::{Utf8, Base64}` with serde lowercase.
  - `ValidatedPublish { title, description, icon, label, if_version, capabilities, files: BTreeMap<String, FileChange> }`, `FileChange::{Put(DecodedFile), Remove}`, `DecodedFile { bytes: Vec<u8>, content_type: String }`.
  - `validate(req: PublishRequest) -> Result<ValidatedPublish>`; `pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024; pub const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;`
  - `Store::create_artifact(&self, p: ValidatedPublish) -> Result<(Artifact, Version)>`
  - `Store::publish_version(&self, id: &ArtifactId, p: ValidatedPublish) -> Result<(Artifact, Version)>` (requires `if_version == current_version` else `Conflict`)
  - `Store::get_version(&self, id, n) -> Result<Option<Version>>`, `Store::list_versions(&self, id) -> Result<Vec<Version>>`
  - `Store::file_path(&self, id, n, path) -> Result<Option<(PathBuf, FileMeta)>>` returning the on-disk path only for files recorded in that version's `files_json`.
- On-disk layout per spec §5: `versions/<n>/index.html`, `versions/<n>/files/<path>`.

- [ ] **Step 1: Write the failing validation tests**

`crates/artifax-core/src/publish.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn req(files: &[(&str, Option<FileInput>)]) -> PublishRequest {
        PublishRequest {
            title: Some("T".into()), description: None, icon: None, label: None,
            if_version: None, capabilities: None,
            files: files.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>(),
        }
    }
    fn utf8(s: &str) -> Option<FileInput> {
        Some(FileInput { content: s.into(), encoding: Encoding::Utf8, content_type: None })
    }

    #[test]
    fn index_html_is_required_and_may_not_be_removed() {
        let e = validate(req(&[("app.js", utf8("1"))])).unwrap_err();
        assert!(matches!(e, CoreError::Invalid { code: "missing_index", .. }));
        let e = validate(req(&[("index.html", None)])).unwrap_err();
        assert!(matches!(e, CoreError::Invalid { code: "missing_index", .. }));
    }

    #[test]
    fn rejects_unsafe_paths() {
        for bad in ["../x.js", "/etc/passwd", "a\\b.js", "a//b.js", "a/./b.js", "", "a/", "a/../b"] {
            let e = validate(req(&[("index.html", utf8("<p>")), (bad, utf8("x"))])).unwrap_err();
            assert!(matches!(e, CoreError::Invalid { code: "invalid_path", .. }), "{bad}");
        }
        assert!(validate(req(&[("index.html", utf8("<p>")), ("css/a-b_c.v2.css", utf8("x"))])).is_ok());
    }

    #[test]
    fn decodes_base64_and_rejects_bad_base64() {
        let v = validate(req(&[("index.html", utf8("<p>")),
            ("a.bin", Some(FileInput { content: "AQID".into(), encoding: Encoding::Base64, content_type: None }))])).unwrap();
        match &v.files["a.bin"] {
            FileChange::Put(f) => { assert_eq!(f.bytes, vec![1, 2, 3]); assert_eq!(f.content_type, "application/octet-stream"); }
            _ => panic!(),
        }
        let e = validate(req(&[("index.html", utf8("<p>")),
            ("a.bin", Some(FileInput { content: "!!!".into(), encoding: Encoding::Base64, content_type: None }))])).unwrap_err();
        assert!(matches!(e, CoreError::Invalid { code: "invalid_encoding", .. }));
    }

    #[test]
    fn content_types_come_from_extension_or_override() {
        let v = validate(req(&[("index.html", utf8("<p>")), ("app.js", utf8("1")),
            ("data.csv", Some(FileInput { content: "a".into(), encoding: Encoding::Utf8, content_type: Some("text/plain".into()) }))])).unwrap();
        let ct = |k: &str| match &v.files[k] { FileChange::Put(f) => f.content_type.clone(), _ => panic!() };
        assert_eq!(ct("index.html"), "text/html");
        assert_eq!(ct("app.js"), "text/javascript");
        assert_eq!(ct("data.csv"), "text/plain");
    }

    #[test]
    fn enforces_size_caps_and_label_length() {
        let big = "x".repeat(MAX_FILE_BYTES as usize + 1);
        let e = validate(req(&[("index.html", utf8("<p>")), ("big.txt", utf8(&big))])).unwrap_err();
        assert!(matches!(e, CoreError::Invalid { code: "file_too_large", .. }));
        let mut r = req(&[("index.html", utf8("<p>"))]);
        r.label = Some("x".repeat(61));
        assert!(matches!(validate(r).unwrap_err(), CoreError::Invalid { code: "label_too_long", .. }));
    }

    #[test]
    fn removals_pass_through() {
        let v = validate(req(&[("index.html", utf8("<p>")), ("old.css", None)])).unwrap();
        assert!(matches!(v.files["old.css"], FileChange::Remove));
    }
}
```

- [ ] **Step 2: Write the failing store tests for create and publish**

Append to `crates/artifax-core/src/store/artifacts.rs` tests module:
```rust
    use crate::publish::{Encoding, FileInput, PublishRequest, validate};
    use std::collections::BTreeMap;

    fn publish(files: &[(&str, Option<&str>)], if_version: Option<u32>) -> crate::publish::ValidatedPublish {
        let files = files.iter().map(|(k, v)| (k.to_string(), v.map(|s| FileInput {
            content: s.to_string(), encoding: Encoding::Utf8, content_type: None }))).collect::<BTreeMap<_, _>>();
        validate(PublishRequest { title: Some("T".into()), description: None, icon: None, label: None,
            if_version, capabilities: None, files }).unwrap()
    }

    #[test]
    fn create_writes_version_1_and_files() {
        let (_d, store) = store();
        let (a, v) = store.create_artifact(publish(&[("index.html", Some("<p>hi")), ("app.js", Some("1"))], None)).unwrap();
        assert_eq!(a.current_version, 1);
        assert_eq!(v.n, 1);
        assert_eq!(v.files["index.html"].size, 5);
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let vdir = store.home().version_dir(&id, 1);
        assert_eq!(std::fs::read_to_string(vdir.join("index.html")).unwrap(), "<p>hi");
        assert_eq!(std::fs::read_to_string(vdir.join("files/app.js")).unwrap(), "1");
        let (p, meta) = store.file_path(&id, 1, "app.js").unwrap().unwrap();
        assert!(p.ends_with("files/app.js"));
        assert_eq!(meta.content_type, "text/javascript");
        assert!(store.file_path(&id, 1, "nope.js").unwrap().is_none());
    }

    #[test]
    fn publish_version_carries_files_forward_and_honours_removals() {
        let (_d, store) = store();
        let (a, _) = store.create_artifact(publish(&[("index.html", Some("v1")), ("a.js", Some("a")), ("b.css", Some("b"))], None)).unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let (a2, v2) = store.publish_version(&id, publish(&[("index.html", Some("v2")), ("b.css", None), ("c.txt", Some("c"))], Some(1))).unwrap();
        assert_eq!(a2.current_version, 2);
        assert_eq!(v2.files.keys().cloned().collect::<Vec<_>>(), vec!["a.js", "c.txt", "index.html"]);
        let vdir = store.home().version_dir(&id, 2);
        assert_eq!(std::fs::read_to_string(vdir.join("files/a.js")).unwrap(), "a");
        assert!(!vdir.join("files/b.css").exists());
        assert_eq!(store.list_versions(&id).unwrap().len(), 2);
        assert_eq!(store.get_version(&id, 1).unwrap().unwrap().files.len(), 3);
    }

    #[test]
    fn stale_if_version_conflicts_and_writes_nothing() {
        let (_d, store) = store();
        let (a, _) = store.create_artifact(publish(&[("index.html", Some("v1"))], None)).unwrap();
        let id = crate::ArtifactId::parse(&a.id).unwrap();
        let e = store.publish_version(&id, publish(&[("index.html", Some("v2"))], Some(7))).unwrap_err();
        assert!(matches!(e, crate::CoreError::Conflict { current: 1 }));
        let e = store.publish_version(&id, publish(&[("index.html", Some("v2"))], None)).unwrap_err();
        assert!(matches!(e, crate::CoreError::Invalid { code: "if_version_required", .. }));
        assert!(!store.home().version_dir(&id, 2).exists());
        assert_eq!(store.get_artifact(&id).unwrap().unwrap().current_version, 1);
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p artifax-core`
Expected: compile errors for `publish` module and `create_artifact`.

- [ ] **Step 4: Implement publish.rs**

```rust
//! Publish request validation: path safety, encodings, size caps, content types.

use crate::{CoreError, Result};
use base64::Engine;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_LABEL_CHARS: usize = 60;
pub const INDEX: &str = "index.html";

#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    #[default]
    Utf8,
    Base64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FileInput {
    pub content: String,
    #[serde(default)]
    pub encoding: Encoding,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct PublishRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub label: Option<String>,
    pub if_version: Option<u32>,
    pub capabilities: Option<serde_json::Value>,
    #[serde(default)]
    pub files: BTreeMap<String, Option<FileInput>>,
}

#[derive(Clone, Debug)]
pub struct DecodedFile {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[derive(Clone, Debug)]
pub enum FileChange {
    Put(DecodedFile),
    Remove,
}

#[derive(Clone, Debug)]
pub struct ValidatedPublish {
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub label: Option<String>,
    pub if_version: Option<u32>,
    pub capabilities: Option<serde_json::Value>,
    pub files: BTreeMap<String, FileChange>,
}

/// A relative path of one or more non-empty segments, no `.`/`..`, forward slashes only.
pub fn check_path(path: &str) -> Result<()> {
    let bad = || CoreError::invalid("invalid_path", format!("'{path}' is not a safe relative path"));
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') || path.contains('\\') {
        return Err(bad());
    }
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.chars().any(|c| c.is_control()) {
            return Err(bad());
        }
    }
    Ok(())
}

pub fn content_type_for(path: &str) -> String {
    match path.rsplit('.').next() {
        Some("js") | Some("mjs") => "text/javascript".to_string(),
        _ => mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream").to_string(),
    }
}

pub fn validate(req: PublishRequest) -> Result<ValidatedPublish> {
    if let Some(label) = &req.label {
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(CoreError::invalid("label_too_long", format!("label exceeds {MAX_LABEL_CHARS} characters")));
        }
    }
    match req.files.get(INDEX) {
        Some(Some(_)) => {}
        _ => return Err(CoreError::invalid("missing_index", "index.html is required on every publish")),
    }
    let mut total: u64 = 0;
    let mut files = BTreeMap::new();
    for (path, input) in req.files {
        check_path(&path)?;
        match input {
            None => {
                files.insert(path, FileChange::Remove);
            }
            Some(f) => {
                let bytes = match f.encoding {
                    Encoding::Utf8 => f.content.into_bytes(),
                    Encoding::Base64 => base64::engine::general_purpose::STANDARD
                        .decode(f.content.as_bytes())
                        .map_err(|_| CoreError::invalid("invalid_encoding", format!("'{path}' is not valid base64")))?,
                };
                if bytes.len() as u64 > MAX_FILE_BYTES {
                    return Err(CoreError::invalid("file_too_large", format!("'{path}' exceeds {MAX_FILE_BYTES} bytes")));
                }
                total += bytes.len() as u64;
                if total > MAX_BODY_BYTES {
                    return Err(CoreError::invalid("body_too_large", format!("publish exceeds {MAX_BODY_BYTES} bytes")));
                }
                let content_type = f.content_type.unwrap_or_else(|| content_type_for(&path));
                files.insert(path, FileChange::Put(DecodedFile { bytes, content_type }));
            }
        }
    }
    Ok(ValidatedPublish {
        title: req.title, description: req.description, icon: req.icon, label: req.label,
        if_version: req.if_version, capabilities: req.capabilities, files,
    })
}
```

- [ ] **Step 5: Implement create_artifact, publish_version, versions, file_path**

Append to `crates/artifax-core/src/store/artifacts.rs` (add `use crate::model::{FileMeta, Version}; use crate::publish::{FileChange, INDEX, ValidatedPublish}; use std::collections::BTreeMap; use std::path::PathBuf;`):
```rust
fn row_to_version(r: &Row<'_>) -> rusqlite::Result<Version> {
    let files: String = r.get("files_json")?;
    Ok(Version {
        artifact_id: r.get("artifact_id")?,
        n: r.get("n")?,
        label: r.get("label")?,
        created_at: r.get("created_at")?,
        session_id: r.get("session_id")?,
        files: serde_json::from_str(&files).unwrap_or_default(),
    })
}

const SELECT_VERSION: &str = "SELECT artifact_id, n, label, created_at, session_id, files_json FROM versions";

impl Store {
    pub fn create_artifact(&self, p: ValidatedPublish) -> Result<(Artifact, Version)> {
        let id = ArtifactId::generate();
        let now = Store::now();
        let title = p.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "Untitled".to_string());
        let caps = p.capabilities.clone().unwrap_or(serde_json::json!({}));
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO artifacts (id, title, description, icon, created_at, updated_at, current_version,
                    pinned, capabilities_json, contract_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, ?6, ?7)",
                params![id.as_str(), title, p.description, p.icon, now, caps.to_string(), CONTRACT_VERSION],
            )?;
            Ok(())
        })?;
        self.write_version(&id, 1, &p, &BTreeMap::new())
    }

    pub fn publish_version(&self, id: &ArtifactId, p: ValidatedPublish) -> Result<(Artifact, Version)> {
        let current = self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
        let Some(expected) = p.if_version else {
            return Err(CoreError::invalid("if_version_required", "if_version is required when updating an artifact"));
        };
        if expected != current.current_version {
            return Err(CoreError::Conflict { current: current.current_version });
        }
        let prev = self.get_version(id, current.current_version)?.map(|v| v.files).unwrap_or_default();
        self.write_version(id, current.current_version + 1, &p, &prev)
    }

    /// Writes files for version `n`, carrying forward `prev` entries not named in `p.files`,
    /// then records the version and bumps the artifact in one transaction.
    fn write_version(&self, id: &ArtifactId, n: u32, p: &ValidatedPublish, prev: &BTreeMap<String, FileMeta>)
        -> Result<(Artifact, Version)> {
        let vdir = self.home.version_dir(id, n);
        let files_dir = vdir.join("files");
        std::fs::create_dir_all(&files_dir)?;
        let write = |path: &str, bytes: &[u8]| -> Result<()> {
            let dest = if path == INDEX { vdir.join(INDEX) } else { files_dir.join(path) };
            if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(dest, bytes)?;
            Ok(())
        };
        let mut files: BTreeMap<String, FileMeta> = BTreeMap::new();
        for (path, meta) in prev {
            if path == INDEX || p.files.contains_key(path) { continue; }
            let src = self.home.version_dir(id, n - 1).join("files").join(path);
            let bytes = std::fs::read(&src)?;
            write(path, &bytes)?;
            files.insert(path.clone(), meta.clone());
        }
        for (path, change) in &p.files {
            if let FileChange::Put(f) = change {
                write(path, &f.bytes)?;
                files.insert(path.clone(), FileMeta { content_type: f.content_type.clone(), size: f.bytes.len() as u64 });
            }
        }
        let now = Store::now();
        let files_json = serde_json::to_string(&files).expect("serialisable map");
        let result = self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO versions (artifact_id, n, label, created_at, session_id, files_json)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                params![id.as_str(), n, p.label, now, files_json],
            )?;
            tx.execute(
                "UPDATE artifacts SET current_version = ?2, updated_at = ?3,
                    title = COALESCE(?4, title), description = COALESCE(?5, description), icon = COALESCE(?6, icon),
                    capabilities_json = COALESCE(?7, capabilities_json)
                 WHERE id = ?1",
                params![id.as_str(), n, now, p.title, p.description, p.icon, p.capabilities.as_ref().map(|c| c.to_string())],
            )?;
            let a = tx.query_row(&format!("{SELECT} WHERE id = ?1"), params![id.as_str()], row_to_artifact)?;
            let v = tx.query_row(&format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"), params![id.as_str(), n], row_to_version)?;
            Ok((a, v))
        });
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&vdir);
        }
        result
    }

    pub fn get_version(&self, id: &ArtifactId, n: u32) -> Result<Option<Version>> {
        self.with_conn(|c| Ok(c.query_row(
            &format!("{SELECT_VERSION} WHERE artifact_id = ?1 AND n = ?2"), params![id.as_str(), n], row_to_version).optional()?))
    }

    pub fn list_versions(&self, id: &ArtifactId) -> Result<Vec<Version>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!("{SELECT_VERSION} WHERE artifact_id = ?1 ORDER BY n"))?;
            Ok(stmt.query_map(params![id.as_str()], row_to_version)?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn file_path(&self, id: &ArtifactId, n: u32, path: &str) -> Result<Option<(PathBuf, FileMeta)>> {
        let Some(v) = self.get_version(id, n)? else { return Ok(None) };
        let Some(meta) = v.files.get(path) else { return Ok(None) };
        let vdir = self.home.version_dir(id, n);
        let p = if path == INDEX { vdir.join(INDEX) } else { vdir.join("files").join(path) };
        Ok(Some((p, meta.clone())))
    }
}
```

Note: in `create_artifact` the artifact row is inserted with `current_version = 0` and `write_version` bumps it to 1, so a crash between the two leaves a zero-version row that `list_artifacts` should hide: add `AND current_version > 0` to the `WHERE` clauses of `get_artifact` and `list_artifacts`.

Add to `lib.rs`: `pub mod publish;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p artifax-core && cargo clippy --workspace -- -D warnings`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "Validate publish requests and write versions with copy-forward"
```

---

### Task 4: Asset store

**Files:**
- Create: `crates/artifax-core/src/store/assets.rs`
- Modify: `crates/artifax-core/src/store/mod.rs`

**Interfaces:**
- Produces: `Store::add_asset(&self, id: &ArtifactId, content_type: &str, bytes: &[u8]) -> Result<Asset>`, `Store::get_asset(&self, asset_id: &str) -> Result<Option<(Asset, PathBuf)>>`, `Store::list_assets(&self, id) -> Result<Vec<Asset>>`, `Store::delete_asset(&self, asset_id) -> Result<()>`. Files live at `assets_dir(id)/<asset_id>.<ext>`; `ext` comes from `mime_guess::get_mime_extensions_str(content_type)` first entry, else `bin`. Accepted content types: `image/*`, `video/*`, `application/pdf`, `font/*`, `text/css`, `text/javascript`, `text/csv`, `text/markdown`, `application/json`, `text/plain`; anything else is `Invalid { code: "unsupported_type" }`. Cap 20 MiB (`MAX_ASSET_BYTES`).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use crate::{Home, Store, CoreError};

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, store)
    }

    #[test]
    fn add_get_list_delete() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        let a = store.add_asset(&id, "image/png", &[1, 2, 3]).unwrap();
        assert_eq!(a.ext, "png");
        assert_eq!(a.size, 3);
        let (got, path) = store.get_asset(&a.id).unwrap().unwrap();
        assert_eq!(got, a);
        assert_eq!(std::fs::read(&path).unwrap(), vec![1, 2, 3]);
        assert_eq!(store.list_assets(&id).unwrap().len(), 1);
        store.delete_asset(&a.id).unwrap();
        assert!(store.get_asset(&a.id).unwrap().is_none());
        assert!(!path.exists());
        assert!(matches!(store.delete_asset(&a.id), Err(CoreError::NotFound)));
    }

    #[test]
    fn rejects_unsupported_types_and_oversize() {
        let (_d, store) = store();
        let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
        assert!(matches!(store.add_asset(&id, "application/x-msdownload", &[0]).unwrap_err(),
            CoreError::Invalid { code: "unsupported_type", .. }));
        let big = vec![0u8; super::MAX_ASSET_BYTES as usize + 1];
        assert!(matches!(store.add_asset(&id, "image/png", &big).unwrap_err(),
            CoreError::Invalid { code: "asset_too_large", .. }));
    }

    #[test]
    fn unknown_artifact_is_not_found() {
        let (_d, store) = store();
        let id = crate::ArtifactId::generate();
        assert!(matches!(store.add_asset(&id, "image/png", &[0]).unwrap_err(), CoreError::NotFound));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p artifax-core assets`
Expected: compile error, `add_asset` not defined.

- [ ] **Step 3: Implement**

`crates/artifax-core/src/store/assets.rs`:
```rust
//! Per-artifact asset store: uploaded images, media, fonts, and data files served at /_blob/<id>.

use super::Store;
use crate::model::Asset;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, Row, params};
use std::path::PathBuf;

pub const MAX_ASSET_BYTES: u64 = 20 * 1024 * 1024;

fn row_to_asset(r: &Row<'_>) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: r.get("id")?, artifact_id: r.get("artifact_id")?, content_type: r.get("content_type")?,
        size: r.get::<_, i64>("size")? as u64, ext: r.get("ext")?, created_at: r.get("created_at")?,
    })
}

pub fn is_supported(content_type: &str) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    ct.starts_with("image/") || ct.starts_with("video/") || ct.starts_with("font/")
        || matches!(ct, "application/pdf" | "text/css" | "text/javascript" | "text/csv"
            | "text/markdown" | "application/json" | "text/plain")
}

fn ext_for(content_type: &str) -> String {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    match ct {
        "text/javascript" => "js".into(),
        "text/markdown" => "md".into(),
        _ => mime_guess::get_mime_extensions_str(ct).and_then(|e| e.first()).map(|s| s.to_string()).unwrap_or("bin".into()),
    }
}

const SELECT: &str = "SELECT id, artifact_id, content_type, size, ext, created_at FROM assets";

impl Store {
    pub fn add_asset(&self, id: &ArtifactId, content_type: &str, bytes: &[u8]) -> Result<Asset> {
        if !is_supported(content_type) {
            return Err(CoreError::invalid("unsupported_type", format!("'{content_type}' is not an accepted asset type")));
        }
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(CoreError::invalid("asset_too_large", format!("asset exceeds {MAX_ASSET_BYTES} bytes")));
        }
        self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
        let asset = Asset {
            id: new_ulid(), artifact_id: id.as_str().to_string(), content_type: content_type.to_string(),
            size: bytes.len() as u64, ext: ext_for(content_type), created_at: Store::now(),
        };
        let dir = self.home.assets_dir(id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{}.{}", asset.id, asset.ext)), bytes)?;
        self.with_conn(|c| {
            c.execute("INSERT INTO assets (id, artifact_id, content_type, size, ext, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![asset.id, asset.artifact_id, asset.content_type, asset.size as i64, asset.ext, asset.created_at])?;
            Ok(())
        })?;
        Ok(asset)
    }

    pub fn get_asset(&self, asset_id: &str) -> Result<Option<(Asset, PathBuf)>> {
        let a = self.with_conn(|c| Ok(c.query_row(&format!("{SELECT} WHERE id = ?1"), params![asset_id], row_to_asset).optional()?))?;
        Ok(a.map(|a| {
            let id = ArtifactId::parse(&a.artifact_id).expect("stored id is valid");
            let path = self.home.assets_dir(&id).join(format!("{}.{}", a.id, a.ext));
            (a, path)
        }))
    }

    pub fn list_assets(&self, id: &ArtifactId) -> Result<Vec<Asset>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(&format!("{SELECT} WHERE artifact_id = ?1 ORDER BY created_at"))?;
            Ok(stmt.query_map(params![id.as_str()], row_to_asset)?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn delete_asset(&self, asset_id: &str) -> Result<()> {
        let Some((_, path)) = self.get_asset(asset_id)? else { return Err(CoreError::NotFound) };
        self.with_conn(|c| { c.execute("DELETE FROM assets WHERE id = ?1", params![asset_id])?; Ok(()) })?;
        if path.exists() { std::fs::remove_file(path)?; }
        Ok(())
    }
}
```

Add `pub mod assets;` to `store/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p artifax-core && cargo clippy --workspace -- -D warnings`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Add per-artifact asset store"
```

---

### Task 5: Document wrapping and the recognition rule

**Files:**
- Create: `crates/artifax-core/src/wrap.rs`
- Modify: `crates/artifax-core/src/lib.rs`

**Interfaces:**
- Produces: `wrap_document(page: &str, artifact_id: &str, version: u32, contract: &str) -> String`, `bridge_tag(artifact_id, version, contract) -> String`, `is_full_document(page: &str) -> bool`, `pub const RESET_CSS: &str`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_is_wrapped_with_skeleton_and_bridge_first_in_body() {
        let out = wrap_document("<title>T</title><style>p{}</style><p>hi</p>", "7q3k9mzx2b4t", 2, "0.2.61");
        assert!(out.starts_with("<!doctype html><html><head><meta charset=utf8><meta name=viewport"));
        let body = out.find("<body>").unwrap();
        let tag = out.find("<script src=\"/_artifax/bridge.js\"").unwrap();
        assert!(tag > body && tag < out.find("<title>").unwrap());
        assert!(out.contains("data-artifact=\"7q3k9mzx2b4t\" data-version=\"2\" data-contract=\"0.2.61\""));
        assert!(out.ends_with("</body></html>"));
    }

    #[test]
    fn full_document_is_recognised_case_insensitively_and_bridge_goes_after_body_tag() {
        let page = "\n  <!DOCTYPE HTML><html><head><title>x</title></head><body class=\"x\" data-a=\"1\"><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        assert!(out.starts_with("\n  <!DOCTYPE HTML>"), "served as-is");
        let body_end = out.find("<body class=\"x\" data-a=\"1\">").unwrap() + "<body class=\"x\" data-a=\"1\">".len();
        assert!(out[body_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")));
        assert_eq!(out.matches("<!DOCTYPE").count() + out.matches("<!doctype").count(), 1, "not double wrapped");
    }

    #[test]
    fn full_document_without_body_tag_gets_bridge_after_doctype() {
        let out = wrap_document("<!doctype html><p>no body tag</p>", "7q3k9mzx2b4t", 1, "0.2.61");
        assert!(out.starts_with(&format!("<!doctype html>{}", bridge_tag("7q3k9mzx2b4t", 1, "0.2.61"))));
    }

    #[test]
    fn body_inside_a_comment_or_attribute_is_not_the_body_tag() {
        let page = "<!doctype html><html><head><!-- <body> --></head><body><p></p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        let real = out.find("<body><script").expect("bridge after real body tag");
        assert!(real > out.find("-->").unwrap());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p artifax-core wrap`
Expected: compile error.

- [ ] **Step 3: Implement**

```rust
//! Serve-time wrapping of a published page into the document skeleton, and the
//! recognition rule that keeps a full document from being wrapped twice.

pub const RESET_CSS: &str = "*,*::before,*::after{box-sizing:border-box}html,body{margin:0;padding:0;min-height:100%}img,video,svg{max-width:100%;display:block}";

pub fn bridge_tag(artifact_id: &str, version: u32, contract: &str) -> String {
    format!(
        "<script src=\"/_artifax/bridge.js\" data-artifact=\"{artifact_id}\" data-version=\"{version}\" data-contract=\"{contract}\"></script>"
    )
}

/// A page that begins, after whitespace, with `<!doctype html>` (any case) is complete.
pub fn is_full_document(page: &str) -> bool {
    let head: String = page.trim_start().chars().take(15).collect();
    head.eq_ignore_ascii_case("<!doctype html>")
}

/// Byte offset just past the first real `<body ...>` tag, skipping HTML comments.
fn body_tag_end(doc: &str) -> Option<usize> {
    let lower = doc.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if lower[i..].starts_with("<!--") {
            match lower[i..].find("-->") { Some(e) => { i += e + 3; continue; } None => return None }
        }
        if lower[i..].starts_with("<body") {
            let after = i + 5;
            let next = bytes.get(after).copied();
            if matches!(next, Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')) {
                return lower[after..].find('>').map(|e| after + e + 1);
            }
        }
        i += 1;
    }
    None
}

pub fn wrap_document(page: &str, artifact_id: &str, version: u32, contract: &str) -> String {
    let tag = bridge_tag(artifact_id, version, contract);
    if is_full_document(page) {
        if let Some(pos) = body_tag_end(page) {
            return format!("{}{}{}", &page[..pos], tag, &page[pos..]);
        }
        let trimmed = page.len() - page.trim_start().len();
        let doctype_end = trimmed + "<!doctype html>".len();
        return format!("{}{}{}", &page[..doctype_end], tag, &page[doctype_end..]);
    }
    format!(
        "<!doctype html><html><head><meta charset=utf8><meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><style>{RESET_CSS}</style></head><body>{tag}{page}</body></html>"
    )
}
```

Add `pub mod wrap;` to `lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p artifax-core && cargo clippy --workspace -- -D warnings`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Wrap published pages with the document skeleton and bridge tag"
```

---

### Task 6: Event bus

**Files:**
- Create: `crates/artifax-core/src/events.rs`
- Modify: `crates/artifax-core/src/lib.rs`

**Interfaces:**
- Produces: `#[derive(Clone, Serialize)] #[serde(tag = "type", rename_all = "snake_case")] pub enum Event { Version { artifact_id: String, n: u32 }, ArtifactDeleted { artifact_id: String } }`, `Event::artifact_id(&self) -> &str`, `pub struct EventBus(broadcast::Sender<Event>)` with `EventBus::new() -> EventBus`, `publish(&self, Event)`, `subscribe(&self) -> broadcast::Receiver<Event>`. Capacity 256.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_events_as_json() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(Event::Version { artifact_id: "7q3k9mzx2b4t".into(), n: 2 });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.artifact_id(), "7q3k9mzx2b4t");
        assert_eq!(serde_json::to_value(&ev).unwrap(), serde_json::json!({"type": "version", "artifact_id": "7q3k9mzx2b4t", "n": 2}));
    }

    #[test]
    fn publish_without_subscribers_does_not_panic() {
        EventBus::new().publish(Event::ArtifactDeleted { artifact_id: "x".into() });
    }
}
```

Add `tokio = { workspace = true, features = ["sync", "macros", "rt"] }` to the core's `[dev-dependencies]` (the `sync` feature is already in `[dependencies]`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-core events`
Expected: compile error.

- [ ] **Step 3: Implement**

```rust
//! In-process broadcast of storage changes, fanned out to SSE clients by the server.

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Version { artifact_id: String, n: u32 },
    ArtifactDeleted { artifact_id: String },
}

impl Event {
    pub fn artifact_id(&self) -> &str {
        match self {
            Event::Version { artifact_id, .. } | Event::ArtifactDeleted { artifact_id } => artifact_id,
        }
    }
}

#[derive(Clone)]
pub struct EventBus(broadcast::Sender<Event>);

impl EventBus {
    pub fn new() -> Self {
        EventBus(broadcast::channel(256).0)
    }
    pub fn publish(&self, event: Event) {
        let _ = self.0.send(event);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
```

Add `pub mod events; pub use events::{Event, EventBus};` to `lib.rs`.

- [ ] **Step 4: Run to verify pass, then commit**

Run: `cargo test -p artifax-core && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Add event bus for version and delete notifications"
```

---

### Task 7: Server skeleton: state, errors, auth, health, token, test harness

**Files:**
- Create: `crates/artifax-server/Cargo.toml`, `crates/artifax-server/src/lib.rs`, `src/state.rs`, `src/error.rs`, `src/auth.rs`, `src/routes/mod.rs`, `src/routes/health.rs`, `src/routes/token.rs`, `tests/common/mod.rs`, `tests/api_auth.rs`

**Interfaces:**
- Produces: `AppState { store: Arc<Store>, home: Home, token: String, events: EventBus, started_at: String, version: &'static str }` (Clone), `build_router(state: AppState) -> axum::Router`, `ApiError` with `ApiError::from(CoreError)` mapping `NotFound → 404 not_found`, `Conflict → 409 conflict` with `"current": n` in the error body, `Invalid → 400 <code>`, `Io/Db → 500 internal`; `ApiError::unauthorized()`, `ApiError::forbidden(msg)`, `ApiError::bad_request(code, msg)`. `RequireToken` axum extractor (rejects with 401 `unauthorized`). `is_loopback(SocketAddr) -> bool`. `GET /healthz` → `{"version","pid","started_at"}` with `Access-Control-Allow-Origin: *`. `GET /api/token` → `{"token"}` for loopback peers else 403 `not_loopback`.
- Test harness: `common::TestServer::spawn() -> TestServer { base: String, token: String, home: Home, client: reqwest::Client, _dir: TempDir }` running on `127.0.0.1:0` with `into_make_service_with_connect_info::<SocketAddr>()`, and helpers `ts.get(path)`, `ts.post_json(path, body)` (with token), `ts.publish(title, files: &[(&str, &str)]) -> serde_json::Value`.

- [ ] **Step 1: Write the server Cargo.toml**

```toml
[package]
name = "artifax-server"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
artifax-core = { path = "../artifax-core" }
anyhow.workspace = true
axum.workspace = true
chrono.workspace = true
libc.workspace = true
rand.workspace = true
rust-embed.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-stream.workspace = true
tower.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
futures = "0.3"
mime_guess.workspace = true

[dev-dependencies]
reqwest = { workspace = true, features = ["stream"] }
tempfile.workspace = true
tokio = { workspace = true, features = ["full"] }
```

- [ ] **Step 2: Write the test harness and the failing auth tests**

`crates/artifax-server/tests/common/mod.rs`:
```rust
#![allow(dead_code)]
use artifax_core::{EventBus, Home, Store};
use artifax_server::{AppState, build_router};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct TestServer {
    pub base: String,
    pub token: String,
    pub home: Home,
    pub client: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl TestServer {
    pub async fn spawn() -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let store = Arc::new(Store::open(&home).unwrap());
        let token = "test-token".to_string();
        let state = AppState {
            store, home: home.clone(), token: token.clone(), events: EventBus::new(),
            started_at: Store::now(), version: "test",
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = build_router(state);
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        TestServer { base: format!("http://{addr}"), token, home, client: reqwest::Client::new(), _dir: dir }
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(format!("{}{}", self.base, path)).send().await.unwrap()
    }

    pub fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    pub async fn post_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.authed(self.client.post(format!("{}{}", self.base, path)).json(&body)).send().await.unwrap()
    }

    /// Publishes a new artifact; `files` are (path, utf8 content).
    pub async fn publish(&self, title: &str, files: &[(&str, &str)]) -> serde_json::Value {
        let files: serde_json::Map<String, serde_json::Value> = files.iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!({"content": v, "encoding": "utf8"}))).collect();
        let res = self.post_json("/api/artifacts", serde_json::json!({"title": title, "files": files})).await;
        assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
        res.json().await.unwrap()
    }
}
```

`crates/artifax-server/tests/api_auth.rs`:
```rust
mod common;
use common::TestServer;

#[tokio::test]
async fn healthz_reports_version_and_allows_any_origin() {
    let ts = TestServer::spawn().await;
    let res = ts.get("/healthz").await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["version"], "test");
    assert!(body["pid"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn token_is_served_to_loopback_peers() {
    let ts = TestServer::spawn().await;
    let body: serde_json::Value = ts.get("/api/token").await.json().await.unwrap();
    assert_eq!(body["token"], "test-token");
}

#[tokio::test]
async fn write_routes_require_bearer_token() {
    let ts = TestServer::spawn().await;
    let res = ts.client.post(format!("{}/api/artifacts", ts.base)).json(&serde_json::json!({})).send().await.unwrap();
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    let res = ts.client.post(format!("{}/api/artifacts", ts.base)).bearer_auth("wrong").json(&serde_json::json!({})).send().await.unwrap();
    assert_eq!(res.status(), 401);
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p artifax-server`
Expected: compile error, `build_router` and `AppState` not defined.

- [ ] **Step 4: Implement state, error, auth, routes**

`src/state.rs`:
```rust
use artifax_core::{EventBus, Home, Store};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub home: Home,
    pub token: String,
    pub events: EventBus,
    pub started_at: String,
    pub version: &'static str,
}
```

`src/error.rs`:
```rust
//! JSON error responses: `{"error": {"code", "message", ...}}`.

use artifax_core::CoreError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError { status, code, message: message.into(), extra: Default::default() }
    }
    pub fn unauthorized() -> Self { Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "a valid bearer token is required") }
    pub fn forbidden(code: &'static str, msg: impl Into<String>) -> Self { Self::new(StatusCode::FORBIDDEN, code, msg) }
    pub fn not_found() -> Self { Self::new(StatusCode::NOT_FOUND, "not_found", "not found") }
    pub fn bad_request(code: &'static str, msg: impl Into<String>) -> Self { Self::new(StatusCode::BAD_REQUEST, code, msg) }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound => ApiError::not_found(),
            CoreError::Conflict { current } => {
                let mut err = ApiError::new(StatusCode::CONFLICT, "conflict", format!("artifact is at version {current}"));
                err.extra.insert("current".into(), json!(current));
                err
            }
            CoreError::Invalid { code, message } => ApiError::bad_request(code, message),
            CoreError::Io(e) => { tracing::error!(error = %e, "io"); ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "storage error") }
            CoreError::Db(e) => { tracing::error!(error = %e, "db"); ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "database error") }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut err = serde_json::Map::new();
        err.insert("code".into(), json!(self.code));
        err.insert("message".into(), json!(self.message));
        err.extend(self.extra);
        (self.status, axum::Json(json!({"error": err}))).into_response()
    }
}
```

`src/auth.rs`:
```rust
//! Bearer-token gate for write routes and the loopback check for /api/token.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::net::SocketAddr;

pub struct RequireToken;

impl FromRequestParts<AppState> for RequireToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let header = parts.headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
        let presented = header.strip_prefix("Bearer ").unwrap_or("");
        if constant_time_eq(presented.as_bytes(), state.token.as_bytes()) { Ok(RequireToken) } else { Err(ApiError::unauthorized()) }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() { return false; }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().is_loopback()
}
```

`src/routes/health.rs`:
```rust
use crate::state::AppState;
use axum::{Json, extract::State};
use serde_json::json;

pub async fn healthz(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"version": s.version, "pid": std::process::id(), "started_at": s.started_at}))
}
```

`src/routes/token.rs`:
```rust
use crate::auth::is_loopback;
use crate::error::ApiError;
use crate::state::AppState;
use axum::{Json, extract::{ConnectInfo, State}};
use std::net::SocketAddr;

pub async fn token(State(s): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_loopback(addr) { return Err(ApiError::forbidden("not_loopback", "the token is only served to local processes")); }
    Ok(Json(serde_json::json!({"token": s.token})))
}
```

`src/routes/mod.rs` (grows in later tasks; keep the shape):
```rust
pub mod health;
pub mod token;

use crate::state::AppState;
use axum::{Router, routing::get};
use tower_http::cors::{Any, CorsLayer};

pub fn router(state: AppState) -> Router {
    let health = Router::new().route("/healthz", get(health::healthz))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any));
    Router::new()
        .merge(health)
        .route("/api/token", get(token::token))
        .with_state(state)
}
```

`src/lib.rs`:
```rust
//! HTTP server for Artifax: REST API, content serving, SSE, embedded UI.

pub mod auth;
pub mod error;
pub mod routes;
pub mod state;

pub use state::AppState;

pub fn build_router(state: AppState) -> axum::Router {
    routes::router(state)
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`
Expected: 3 tests pass. The `write_routes_require_bearer_token` test needs `/api/artifacts` to exist and reject; until Task 8 adds it, add a temporary route `post("/api/artifacts", |_: RequireToken| async { StatusCode::NOT_IMPLEMENTED })` in `routes/mod.rs`, which Task 8 replaces.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Add server skeleton with token auth, health, and test harness"
```

---

### Task 8: Artifact REST routes

**Files:**
- Create: `crates/artifax-server/src/routes/artifacts.rs`, `crates/artifax-server/tests/api_artifacts.rs`
- Modify: `crates/artifax-server/src/routes/mod.rs`

**Interfaces:**
- Consumes: `Store::{create_artifact, publish_version, get_artifact, list_artifacts, update_meta, delete_artifact, get_version, list_versions}`, `publish::validate`, `EventBus::publish`, `RequireToken`, `ApiError`.
- Produces routes:
  - `GET /api/artifacts` → `{"artifacts": [Artifact]}`
  - `POST /api/artifacts` (W, JSON `PublishRequest`) → 201 `{"artifact": Artifact, "version": Version, "url": "/a/<id>"}`
  - `GET /api/artifacts/{aid}` → `{"artifact", "versions": [Version]}`
  - `PATCH /api/artifacts/{aid}` (W, JSON `MetaPatch` fields) → `{"artifact"}`
  - `DELETE /api/artifacts/{aid}` (W) → 204, emits `ArtifactDeleted`
  - `GET /api/artifacts/{aid}/versions` → `{"versions"}`
  - `POST /api/artifacts/{aid}/versions` (W, JSON `PublishRequest`) → 201 `{"artifact","version","url"}`, emits `Event::Version`
  - `GET /api/artifacts/{aid}/versions/{n}` → `{"version"}`
  - `GET /api/artifacts/{aid}/files` → `{"files": current version's map}`
  - Body limit 64 MiB on the two publish routes via `DefaultBodyLimit::max(MAX_BODY_BYTES as usize + 1024)`.
  - Invalid `{aid}` → 400 `invalid_id`; malformed JSON → 400 `invalid_json`.

- [ ] **Step 1: Write the failing tests**

`tests/api_artifacts.rs`:
```rust
mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn publish_get_list_roundtrip() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("Report", &[("index.html", "<p>v1</p>"), ("app.js", "1")]).await;
    let id = created["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 12);
    assert_eq!(created["version"]["n"], 1);
    assert_eq!(created["url"], format!("/a/{id}"));
    let got: serde_json::Value = ts.get(&format!("/api/artifacts/{id}")).await.json().await.unwrap();
    assert_eq!(got["artifact"]["title"], "Report");
    assert_eq!(got["versions"].as_array().unwrap().len(), 1);
    let list: serde_json::Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);
    let files: serde_json::Value = ts.get(&format!("/api/artifacts/{id}/files")).await.json().await.unwrap();
    assert_eq!(files["files"]["app.js"]["content_type"], "text/javascript");
}

#[tokio::test]
async fn republish_requires_matching_if_version() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>v1</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let body = |v: serde_json::Value| json!({"if_version": v, "files": {"index.html": {"content": "<p>v2</p>"}}});
    let res = ts.post_json(&format!("/api/artifacts/{id}/versions"), body(json!(5))).await;
    assert_eq!(res.status(), 409);
    let err: serde_json::Value = res.json().await.unwrap();
    assert_eq!(err["error"]["code"], "conflict");
    assert_eq!(err["error"]["current"], 1);
    let res = ts.post_json(&format!("/api/artifacts/{id}/versions"), json!({"files": {"index.html": {"content": "x"}}})).await;
    assert_eq!(res.status(), 400);
    let res = ts.post_json(&format!("/api/artifacts/{id}/versions"), body(json!(1))).await;
    assert_eq!(res.status(), 201);
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["artifact"]["current_version"], 2);
    let res = ts.get(&format!("/api/artifacts/{id}/versions/2")).await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn validation_errors_are_400_with_codes() {
    let ts = TestServer::spawn().await;
    let res = ts.post_json("/api/artifacts", json!({"files": {"app.js": {"content": "1"}}})).await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "missing_index");
    let res = ts.post_json("/api/artifacts", json!({"files": {"index.html": {"content": "1"}, "../x": {"content": "1"}}})).await;
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "invalid_path");
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts", ts.base)).header("content-type", "application/json").body("{nope")).send().await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "invalid_json");
}

#[tokio::test]
async fn patch_and_delete() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>v1</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts.authed(ts.client.patch(format!("{}/api/artifacts/{id}", ts.base)).json(&json!({"pinned": true, "title": "New"}))).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let a: serde_json::Value = res.json().await.unwrap();
    assert_eq!(a["artifact"]["pinned"], true);
    assert_eq!(a["artifact"]["title"], "New");
    let res = ts.authed(ts.client.delete(format!("{}/api/artifacts/{id}", ts.base))).send().await.unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(ts.get(&format!("/api/artifacts/{id}")).await.status(), 404);
    assert_eq!(ts.get("/api/artifacts/not-an-id").await.status(), 400);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-server --test api_artifacts`
Expected: 404s and compile-time or assertion failures.

- [ ] **Step 3: Implement**

`src/routes/artifacts.rs`:
```rust
//! REST routes for artifacts and versions.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use artifax_core::publish::{PublishRequest, validate};
use artifax_core::{ArtifactId, Event, MetaPatch};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn parse_id(raw: &str) -> Result<ArtifactId, ApiError> {
    ArtifactId::parse(raw).map_err(ApiError::from)
}

fn body<T>(r: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    r.map(|Json(v)| v).map_err(|e| ApiError::bad_request("invalid_json", e.body_text()))
}

pub async fn list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"artifacts": s.store.list_artifacts()?})))
}

pub async fn create(State(s): State<AppState>, _t: RequireToken, req: Result<Json<PublishRequest>, JsonRejection>)
    -> Result<(StatusCode, Json<Value>), ApiError> {
    let p = validate(body(req)?)?;
    let (artifact, version) = s.store.create_artifact(p)?;
    s.events.publish(Event::Version { artifact_id: artifact.id.clone(), n: version.n });
    let url = format!("/a/{}", artifact.id);
    Ok((StatusCode::CREATED, Json(json!({"artifact": artifact, "version": version, "url": url}))))
}

pub async fn get(State(s): State<AppState>, Path(aid): Path<String>) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let artifact = s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"artifact": artifact, "versions": s.store.list_versions(&id)?})))
}

#[derive(Deserialize, Default)]
pub struct PatchBody { title: Option<String>, description: Option<String>, icon: Option<String>, pinned: Option<bool> }

pub async fn patch(State(s): State<AppState>, _t: RequireToken, Path(aid): Path<String>, req: Result<Json<PatchBody>, JsonRejection>)
    -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let artifact = s.store.update_meta(&id, MetaPatch { title: b.title, description: b.description, icon: b.icon, pinned: b.pinned })?;
    Ok(Json(json!({"artifact": artifact})))
}

pub async fn delete(State(s): State<AppState>, _t: RequireToken, Path(aid): Path<String>) -> Result<StatusCode, ApiError> {
    let id = parse_id(&aid)?;
    s.store.delete_artifact(&id)?;
    s.events.publish(Event::ArtifactDeleted { artifact_id: id.as_str().to_string() });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_versions(State(s): State<AppState>, Path(aid): Path<String>) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"versions": s.store.list_versions(&id)?})))
}

pub async fn publish(State(s): State<AppState>, _t: RequireToken, Path(aid): Path<String>, req: Result<Json<PublishRequest>, JsonRejection>)
    -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&aid)?;
    let p = validate(body(req)?)?;
    let (artifact, version) = s.store.publish_version(&id, p)?;
    s.events.publish(Event::Version { artifact_id: artifact.id.clone(), n: version.n });
    let url = format!("/a/{}", artifact.id);
    Ok((StatusCode::CREATED, Json(json!({"artifact": artifact, "version": version, "url": url}))))
}

pub async fn get_version(State(s): State<AppState>, Path((aid, n)): Path<(String, u32)>) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let v = s.store.get_version(&id, n)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"version": v})))
}

pub async fn files(State(s): State<AppState>, Path(aid): Path<String>) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    let a = s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    let v = s.store.get_version(&id, a.current_version)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"files": v.files, "version": v.n})))
}
```

In `routes/mod.rs` replace the temporary route with:
```rust
use artifax_core::publish::MAX_BODY_BYTES;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
pub mod artifacts;
// inside router():
let api = Router::new()
    .route("/api/artifacts", get(artifacts::list).post(artifacts::create))
    .route("/api/artifacts/{aid}", get(artifacts::get).patch(artifacts::patch).delete(artifacts::delete))
    .route("/api/artifacts/{aid}/versions", get(artifacts::list_versions).post(artifacts::publish))
    .route("/api/artifacts/{aid}/versions/{n}", get(artifacts::get_version))
    .route("/api/artifacts/{aid}/files", get(artifacts::files))
    .layer(DefaultBodyLimit::max(MAX_BODY_BYTES as usize + 1024));
// .merge(api) into the main router
```

- [ ] **Step 4: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Add artifact and version REST routes"
```

---

### Task 9: Asset routes and /_blob

**Files:**
- Create: `crates/artifax-server/src/routes/assets.rs`, `crates/artifax-server/tests/api_assets.rs`
- Modify: `crates/artifax-server/src/routes/mod.rs`

**Interfaces:**
- Produces: `POST /api/artifacts/{aid}/assets` (W, multipart field `file` with filename and content type) → 201 `{"asset": Asset, "url": "/_blob/<id>"}`; `GET /api/artifacts/{aid}/assets` → `{"assets"}`; `DELETE /api/artifacts/{aid}/assets/{asset_id}` (W) → 204; `GET /_blob/{asset_id}` → bytes with the asset's content type, `Cache-Control: public, max-age=31536000, immutable`, and `X-Content-Type-Options: nosniff`. Missing multipart field → 400 `missing_file`. Body limit 21 MiB on the upload route.

- [ ] **Step 1: Write the failing tests**

```rust
mod common;
use common::TestServer;

#[tokio::test]
async fn upload_serve_list_delete() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let part = reqwest::multipart::Part::bytes(vec![137, 80, 78, 71]).file_name("logo.png").mime_str("image/png").unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{id}/assets", ts.base)).multipart(form)).send().await.unwrap();
    assert_eq!(res.status(), 201);
    let body: serde_json::Value = res.json().await.unwrap();
    let url = body["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/_blob/"));
    let res = ts.get(&url).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(res.bytes().await.unwrap().to_vec(), vec![137, 80, 78, 71]);
    let list: serde_json::Value = ts.get(&format!("/api/artifacts/{id}/assets")).await.json().await.unwrap();
    assert_eq!(list["assets"].as_array().unwrap().len(), 1);
    let asset_id = body["asset"]["id"].as_str().unwrap();
    let res = ts.authed(ts.client.delete(format!("{}/api/artifacts/{id}/assets/{asset_id}", ts.base))).send().await.unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(ts.get(&url).await.status(), 404);
}

#[tokio::test]
async fn unsupported_type_and_missing_field_are_400() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let part = reqwest::multipart::Part::bytes(vec![0]).file_name("x.exe").mime_str("application/x-msdownload").unwrap();
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{id}/assets", ts.base)).multipart(reqwest::multipart::Form::new().part("file", part))).send().await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "unsupported_type");
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{id}/assets", ts.base)).multipart(reqwest::multipart::Form::new().text("other", "x"))).send().await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "missing_file");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-server --test api_assets`

- [ ] **Step 3: Implement**

```rust
//! Asset upload, listing, deletion, and the /_blob/<id> byte route.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::routes::artifacts::parse_id;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{Value, json};

pub async fn upload(State(s): State<AppState>, _t: RequireToken, Path(aid): Path<String>, mut mp: Multipart)
    -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&aid)?;
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError::bad_request("invalid_multipart", e.to_string()))? {
        if field.name() != Some("file") { continue; }
        let content_type = field.content_type().map(|s| s.to_string())
            .or_else(|| field.file_name().map(|f| artifax_core::publish::content_type_for(f)))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let bytes = field.bytes().await.map_err(|e| ApiError::bad_request("invalid_multipart", e.to_string()))?;
        let asset = s.store.add_asset(&id, &content_type, &bytes)?;
        let url = format!("/_blob/{}", asset.id);
        return Ok((StatusCode::CREATED, Json(json!({"asset": asset, "url": url}))));
    }
    Err(ApiError::bad_request("missing_file", "multipart field 'file' is required"))
}

pub async fn list(State(s): State<AppState>, Path(aid): Path<String>) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&aid)?;
    s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"assets": s.store.list_assets(&id)?})))
}

pub async fn delete(State(s): State<AppState>, _t: RequireToken, Path((aid, asset_id)): Path<(String, String)>) -> Result<StatusCode, ApiError> {
    let id = parse_id(&aid)?;
    match s.store.get_asset(&asset_id)? {
        Some((a, _)) if a.artifact_id == id.as_str() => { s.store.delete_asset(&asset_id)?; Ok(StatusCode::NO_CONTENT) }
        _ => Err(ApiError::not_found()),
    }
}

pub async fn blob(State(s): State<AppState>, Path(asset_id): Path<String>) -> Result<Response, ApiError> {
    let (asset, path) = s.store.get_asset(&asset_id)?.ok_or_else(ApiError::not_found)?;
    let file = tokio::fs::File::open(&path).await.map_err(|_| ApiError::not_found())?;
    let stream = tokio_util::io::ReaderStream::new(file);
    Ok((
        [(header::CONTENT_TYPE, asset.content_type.as_str()),
         (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
         (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        Body::from_stream(stream),
    ).into_response())
}
```

Add `tokio-util = { version = "0.7", features = ["io"] }` to the server's dependencies. In `routes/mod.rs`:
```rust
pub mod assets;
let asset_routes = Router::new()
    .route("/api/artifacts/{aid}/assets", get(assets::list).post(assets::upload))
    .route("/api/artifacts/{aid}/assets/{asset_id}", delete(assets::delete))
    .layer(DefaultBodyLimit::max(21 * 1024 * 1024));
// merge asset_routes and add .route("/_blob/{asset_id}", get(assets::blob))
```

- [ ] **Step 4: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Add asset upload and blob routes"
```

---

### Task 10: Content routes and per-artifact Host routing

**Files:**
- Create: `crates/artifax-server/src/routes/content.rs`, `crates/artifax-server/src/host.rs`, `crates/artifax-server/tests/api_content.rs`
- Modify: `crates/artifax-server/src/routes/mod.rs`, `crates/artifax-server/src/lib.rs`

**Interfaces:**
- Consumes: `Store::{get_artifact, get_version, file_path}`, `wrap::wrap_document`, `model::CONTRACT_VERSION`.
- Produces:
  - `GET /c/{aid}/v/{n}/` → wrapped `index.html`, `text/html; charset=utf-8`, `Cache-Control: no-store`.
  - `GET /c/{aid}/v/{n}/{*path}` → the file with its recorded content type, `Cache-Control: public, max-age=31536000, immutable`, `X-Content-Type-Options: nosniff`. Unknown file → 404 JSON.
  - `GET /c/{aid}/v/{n}` (no trailing slash) → 308 redirect to the slash form so relative URLs in the page resolve.
  - `host::rewrite_artifact_host` middleware: when the `Host` header matches `^([0-9a-hj-km-np-tv-z]{12})\.localhost(:[0-9]+)?$`, rewrite the request path from `/v/…` to `/c/<aid>/v/…` and from `/healthz` unchanged; any other path on such a host → 404. `pub fn artifact_host(host: &str) -> Option<ArtifactId>` is the pure part.
  - `build_router` applies the middleware outermost.

- [ ] **Step 1: Write the failing tests**

`src/host.rs` unit tests:
```rust
#[cfg(test)]
mod tests {
    use super::artifact_host;

    #[test]
    fn accepts_only_anchored_lowercase_ids() {
        assert_eq!(artifact_host("7q3k9mzx2b4t.localhost").unwrap().as_str(), "7q3k9mzx2b4t");
        assert_eq!(artifact_host("7q3k9mzx2b4t.localhost:7480").unwrap().as_str(), "7q3k9mzx2b4t");
        for bad in ["localhost", "localhost:7480", "evil.localhost", "7Q3K9MZX2B4T.localhost",
                    "7q3k9mzx2b4t.localhost.attacker.com", "x.7q3k9mzx2b4t.localhost", "7q3k9mzx2b4t.localhos",
                    "7q3k9mzx2b4t.localhost:abc", "7q3k9mzx2b4i.localhost"] {
            assert!(artifact_host(bad).is_none(), "{bad}");
        }
    }
}
```

`tests/api_content.rs`:
```rust
mod common;
use common::TestServer;

#[tokio::test]
async fn serves_wrapped_index_and_files_with_caching_headers() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<title>R</title><p>hi</p>"), ("css/a.css", "p{color:red}")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let res = ts.get(&format!("/c/{id}/v/1/")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "no-store");
    let html = res.text().await.unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains(&format!("data-artifact=\"{id}\" data-version=\"1\" data-contract=\"0.2.61\"")));
    let res = ts.get(&format!("/c/{id}/v/1/css/a.css")).await;
    assert_eq!(res.headers()["content-type"], "text/css");
    assert_eq!(res.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(res.text().await.unwrap(), "p{color:red}");
    assert_eq!(ts.get(&format!("/c/{id}/v/1/missing.js")).await.status(), 404);
    assert_eq!(ts.get(&format!("/c/{id}/v/9/")).await.status(), 404);
    let res = ts.client.get(format!("{}/c/{id}/v/1", ts.base)).send().await.unwrap();
    assert!(res.url().path().ends_with("/v/1/"), "redirected to trailing slash");
}

#[tokio::test]
async fn artifact_host_routes_to_content_and_nothing_else() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>hi</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let host = format!("{id}.localhost");
    let res = ts.client.get(format!("{}/v/1/", ts.base)).header("host", &host).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("<p>hi</p>"));
    let res = ts.client.get(format!("{}/healthz", ts.base)).header("host", &host).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let res = ts.client.get(format!("{}/api/artifacts", ts.base)).header("host", &host).send().await.unwrap();
    assert_eq!(res.status(), 404, "API is not reachable on an artifact origin");
    let res = ts.client.get(format!("{}/v/1/", ts.base)).header("host", format!("{id}.localhost.attacker.com")).send().await.unwrap();
    assert_eq!(res.status(), 404);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-server host content`

- [ ] **Step 3: Implement host.rs**

```rust
//! Per-artifact origins: `<id>.localhost[:port]` serves only that artifact's content.

use artifax_core::ArtifactId;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub fn artifact_host(host: &str) -> Option<ArtifactId> {
    let host = host.split_once(':').map_or(host, |(h, port)| {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) { "" } else { h }
    });
    let id = host.strip_suffix(".localhost")?;
    if id.contains('.') { return None; }
    ArtifactId::parse(id).ok()
}

pub async fn rewrite_artifact_host(mut req: Request<Body>, next: Next) -> Response {
    let host = req.headers().get(axum::http::header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    if let Some(id) = artifact_host(&host) {
        let path = req.uri().path().to_string();
        let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
        let new_path = if path == "/healthz" {
            path
        } else if path.starts_with("/v/") {
            format!("/c/{}{}", id.as_str(), path)
        } else {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        };
        let uri: Uri = format!("{new_path}{query}").parse().expect("rewritten uri is valid");
        *req.uri_mut() = uri;
    }
    next.run(req).await
}
```

- [ ] **Step 4: Implement content.rs**

```rust
//! Serves published page content: wrapped index and immutable supporting files.

use crate::error::ApiError;
use crate::routes::artifacts::parse_id;
use crate::state::AppState;
use artifax_core::model::CONTRACT_VERSION;
use artifax_core::publish::INDEX;
use artifax_core::wrap::wrap_document;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

pub async fn redirect_to_slash(Path((aid, n)): Path<(String, u32)>) -> Redirect {
    Redirect::permanent(&format!("/c/{aid}/v/{n}/"))
}

pub async fn index(State(s): State<AppState>, Path((aid, n)): Path<(String, u32)>) -> Result<Response, ApiError> {
    let id = parse_id(&aid)?;
    s.store.get_artifact(&id)?.ok_or_else(ApiError::not_found)?;
    let (path, _) = s.store.file_path(&id, n, INDEX)?.ok_or_else(ApiError::not_found)?;
    let page = tokio::fs::read_to_string(&path).await.map_err(|_| ApiError::not_found())?;
    let html = wrap_document(&page, id.as_str(), n, CONTRACT_VERSION);
    Ok(([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response())
}

pub async fn file(State(s): State<AppState>, Path((aid, n, path)): Path<(String, u32, String)>) -> Result<Response, ApiError> {
    let id = parse_id(&aid)?;
    if path == INDEX {
        return Ok(Redirect::permanent(&format!("/c/{aid}/v/{n}/")).into_response());
    }
    let (disk, meta) = s.store.file_path(&id, n, &path)?.ok_or_else(ApiError::not_found)?;
    let f = tokio::fs::File::open(&disk).await.map_err(|_| ApiError::not_found())?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(f));
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, meta.content_type.as_str()),
         (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
         (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        body,
    ).into_response())
}
```

In `routes/mod.rs` add `pub mod content;` and routes:
```rust
.route("/c/{aid}/v/{n}", get(content::redirect_to_slash))
.route("/c/{aid}/v/{n}/", get(content::index))
.route("/c/{aid}/v/{n}/{*path}", get(content::file))
```
In `lib.rs`, `build_router` becomes:
```rust
pub mod host;
pub fn build_router(state: AppState) -> axum::Router {
    routes::router(state).layer(axum::middleware::from_fn(host::rewrite_artifact_host))
}
```

- [ ] **Step 5: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Serve artifact content with per-artifact localhost origins"
```

---

### Task 11: Server-sent events

**Files:**
- Create: `crates/artifax-server/src/routes/events.rs`, `crates/artifax-server/tests/api_events.rs`
- Modify: `crates/artifax-server/src/routes/mod.rs`

**Interfaces:**
- Produces: `GET /api/events[?artifact=<aid>]` → `text/event-stream`; each event is `event: <type>` (`version`, `artifact_deleted`) with `data: <Event JSON>`; a `: keep-alive` comment every 15 s; an initial `event: ready` with `data: {}` so clients know the stream is open. With `?artifact=`, only events whose `artifact_id` matches are sent.

- [ ] **Step 1: Write the failing test**

```rust
mod common;
use common::TestServer;
use futures::StreamExt;

async fn next_event(stream: &mut (impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin), buf: &mut String) -> (String, String) {
    loop {
        if let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf.drain(..end + 2);
            if block.starts_with(':') { continue; }
            let ev = block.lines().find_map(|l| l.strip_prefix("event: ")).unwrap_or("message").to_string();
            let data = block.lines().find_map(|l| l.strip_prefix("data: ")).unwrap_or("").to_string();
            return (ev, data);
        }
        let chunk = stream.next().await.unwrap().unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

#[tokio::test]
async fn publish_emits_version_event_filtered_by_artifact() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "a")]).await;
    let b = ts.publish("B", &[("index.html", "b")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let bid = b["artifact"]["id"].as_str().unwrap().to_string();
    let res = ts.get(&format!("/api/events?artifact={aid}")).await;
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut stream = res.bytes_stream();
    let mut buf = String::new();
    let (ev, _) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "ready");
    ts.post_json(&format!("/api/artifacts/{bid}/versions"), serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "b2"}}})).await;
    ts.post_json(&format!("/api/artifacts/{aid}/versions"), serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "a2"}}})).await;
    let (ev, data) = next_event(&mut stream, &mut buf).await;
    assert_eq!(ev, "version");
    let v: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(v["artifact_id"], aid);
    assert_eq!(v["n"], 2);
}
```

Add `bytes = "1"` and `futures = "0.3"` to the server's `[dev-dependencies]`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-server --test api_events`

- [ ] **Step 3: Implement**

```rust
//! SSE fan-out of the event bus.

use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Deserialize)]
pub struct EventsQuery { artifact: Option<String> }

pub async fn events(State(s): State<AppState>, Query(q): Query<EventsQuery>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = s.events.subscribe();
    let filter = q.artifact;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let live = BroadcastStream::new(rx).filter_map(move |item| {
        let ev = item.ok()?;
        if let Some(f) = &filter { if ev.artifact_id() != f { return None; } }
        let name = match &ev { artifax_core::Event::Version { .. } => "version", artifax_core::Event::ArtifactDeleted { .. } => "artifact_deleted" };
        Some(Ok(SseEvent::default().event(name).data(serde_json::to_string(&ev).unwrap())))
    });
    Sse::new(ready.chain(live)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"))
}
```

Route: `.route("/api/events", get(events::events))`.

- [ ] **Step 4: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Stream version and delete events over SSE"
```

---

### Task 12: Daemon lifecycle: daemon.json, lock, port selection, stale watcher, shutdown

**Files:**
- Create: `crates/artifax-server/src/daemon.rs`, `crates/artifax-server/tests/daemon.rs`
- Modify: `crates/artifax-server/src/lib.rs`, `crates/artifax-server/src/routes/mod.rs`

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct DaemonInfo { pub port: u16, pub pid: u32, pub token: String, pub started_at: String, pub bind: String, pub version: String }`
  - `read_daemon_info(&Home) -> Option<DaemonInfo>`; `write_daemon_info(&Home, &DaemonInfo) -> io::Result<()>` (write to `daemon.json.tmp`, `chmod 0600`, rename); `remove_daemon_info(&Home)`.
  - `pid_alive(u32) -> bool` via `libc::kill(pid, 0)` (`ESRCH` → false; `EPERM` → true).
  - `generate_token() -> String` (32 random bytes, hex).
  - `pub struct ServeConfig { pub home: Home, pub bind: IpAddr, pub port: u16, pub version: &'static str }`
  - `pub async fn serve(cfg: ServeConfig, ready: Option<tokio::sync::oneshot::Sender<DaemonInfo>>) -> anyhow::Result<()>`: binds `bind:port`, then `port+1 ..= port+20` on `AddrInUse`; opens the store; writes `daemon.json`; sends `ready`; runs until `POST /api/admin/shutdown` (W) or the stale watcher fires; removes `daemon.json` on exit only if it still names this pid.
  - Stale watcher: every 30 s read `daemon.json`; if it names a different live pid, log and exit 0.
  - `POST /api/admin/shutdown` (W) → 202, then graceful shutdown.
  - `pub struct DaemonLock(std::fs::File)` with `DaemonLock::acquire(&Home) -> io::Result<DaemonLock>` using `libc::flock(fd, LOCK_EX)`; released on drop. The CLI (Task 14) uses it around auto-start; `serve` does not take it.

- [ ] **Step 1: Write the failing tests**

```rust
use artifax_core::Home;
use artifax_server::daemon::{DaemonInfo, DaemonLock, ServeConfig, pid_alive, read_daemon_info, serve, write_daemon_info};
use std::net::{IpAddr, Ipv4Addr};

#[test]
fn daemon_info_roundtrips_with_0600() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    let info = DaemonInfo { port: 1, pid: 2, token: "t".into(), started_at: "now".into(), bind: "127.0.0.1".into(), version: "v".into() };
    write_daemon_info(&home, &info).unwrap();
    assert_eq!(read_daemon_info(&home), Some(info));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(home.daemon_json()).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::write(home.daemon_json(), "garbage").unwrap();
    assert_eq!(read_daemon_info(&home), None);
}

#[test]
fn pid_alive_distinguishes_live_and_dead() {
    assert!(pid_alive(std::process::id()));
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!pid_alive(pid));
}

#[test]
fn lock_is_exclusive_and_released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    let a = DaemonLock::acquire(&home).unwrap();
    assert!(DaemonLock::try_acquire(&home).unwrap().is_none());
    drop(a);
    assert!(DaemonLock::try_acquire(&home).unwrap().is_some());
}

#[tokio::test]
async fn serve_picks_a_free_port_writes_info_and_shuts_down_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cfg = ServeConfig { home: home.clone(), bind: IpAddr::V4(Ipv4Addr::LOCALHOST), port: busy_port, version: "test" };
    let handle = tokio::spawn(serve(cfg, Some(tx)));
    let info = rx.await.unwrap();
    assert_ne!(info.port, busy_port);
    assert_eq!(read_daemon_info(&home).unwrap().port, info.port);
    let client = reqwest::Client::new();
    let res = client.get(format!("http://127.0.0.1:{}/healthz", info.port)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let res = client.post(format!("http://127.0.0.1:{}/api/admin/shutdown", info.port)).bearer_auth(&info.token).send().await.unwrap();
    assert_eq!(res.status(), 202);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle).await.unwrap().unwrap().unwrap();
    assert!(read_daemon_info(&home).is_none(), "daemon.json removed on clean exit");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-server --test daemon`

- [ ] **Step 3: Implement daemon.rs**

```rust
//! Daemon discovery file, exclusive start lock, port selection, and shutdown.

use crate::state::AppState;
use artifax_core::{EventBus, Home, Store};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

pub const DEFAULT_PORT: u16 = 7480;
pub const PORT_ATTEMPTS: u16 = 21;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DaemonInfo {
    pub port: u16,
    pub pid: u32,
    pub token: String,
    pub started_at: String,
    pub bind: String,
    pub version: String,
}

pub fn read_daemon_info(home: &Home) -> Option<DaemonInfo> {
    let text = std::fs::read_to_string(home.daemon_json()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_daemon_info(home: &Home, info: &DaemonInfo) -> io::Result<()> {
    let tmp = home.root().join("daemon.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(info).expect("serialisable"))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, home.daemon_json())
}

pub fn remove_daemon_info(home: &Home) {
    let _ = std::fs::remove_file(home.daemon_json());
}

pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only probes for existence.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn generate_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct DaemonLock(File);

impl DaemonLock {
    pub fn acquire(home: &Home) -> io::Result<DaemonLock> {
        let f = File::create(home.daemon_lock())?;
        // SAFETY: flock on an owned, open descriptor.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 { return Err(io::Error::last_os_error()); }
        Ok(DaemonLock(f))
    }
    pub fn try_acquire(home: &Home) -> io::Result<Option<DaemonLock>> {
        let f = File::create(home.daemon_lock())?;
        // SAFETY: as above, non-blocking.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(e) };
        }
        Ok(Some(DaemonLock(f)))
    }
}

pub struct ServeConfig {
    pub home: Home,
    pub bind: IpAddr,
    pub port: u16,
    pub version: &'static str,
}

async fn bind_first_free(bind: IpAddr, start: u16) -> io::Result<tokio::net::TcpListener> {
    let mut last = None;
    for port in start..start.saturating_add(PORT_ATTEMPTS) {
        match tokio::net::TcpListener::bind(SocketAddr::new(bind, port)).await {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no port")))
}

pub async fn serve(cfg: ServeConfig, ready: Option<oneshot::Sender<DaemonInfo>>) -> anyhow::Result<()> {
    cfg.home.ensure_dirs()?;
    let listener = bind_first_free(cfg.bind, cfg.port).await?;
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open(&cfg.home)?);
    let token = generate_token();
    let started_at = Store::now();
    let info = DaemonInfo { port, pid: std::process::id(), token: token.clone(), started_at: started_at.clone(), bind: cfg.bind.to_string(), version: cfg.version.to_string() };
    write_daemon_info(&cfg.home, &info)?;

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let state = AppState { store, home: cfg.home.clone(), token, events: EventBus::new(), started_at, version: cfg.version };
    let app = crate::build_router_with_shutdown(state, shutdown_tx.clone());
    if let Some(tx) = ready { let _ = tx.send(info.clone()); }
    tracing::info!(port, "artifax daemon listening");

    let home = cfg.home.clone();
    let stale = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Some(other) = read_daemon_info(&home) {
                if other.pid != std::process::id() && pid_alive(other.pid) {
                    tracing::warn!(other = other.pid, "another daemon owns daemon.json; exiting");
                    return;
                }
            }
        }
    });

    let server = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = async { while shutdown_rx.changed().await.is_ok() { if *shutdown_rx.borrow() { break; } } } => {}
                _ = stale => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        });
    server.await?;
    if read_daemon_info(&cfg.home).map(|i| i.pid) == Some(std::process::id()) {
        remove_daemon_info(&cfg.home);
    }
    Ok(())
}
```

In `lib.rs`, add:
```rust
pub mod daemon;
pub fn build_router_with_shutdown(state: AppState, shutdown: tokio::sync::watch::Sender<bool>) -> axum::Router {
    routes::router(state, Some(shutdown)).layer(axum::middleware::from_fn(host::rewrite_artifact_host))
}
```
and change `build_router` to call `routes::router(state, None)`. In `routes/mod.rs`, `router` takes `shutdown: Option<watch::Sender<bool>>` and adds:
```rust
if let Some(tx) = shutdown {
    let tx = std::sync::Arc::new(tx);
    r = r.route("/api/admin/shutdown", post(move |_t: RequireToken| {
        let tx = tx.clone();
        async move { let _ = tx.send(true); StatusCode::ACCEPTED }
    }));
}
```
(Build `r` with state before adding this route, or add the route to the stateful router before `with_state`; `RequireToken` needs `AppState`, so add it before `.with_state(state)`.)

- [ ] **Step 4: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Add daemon discovery file, start lock, and graceful shutdown"
```

---

### Task 13: Embedded shell serving

**Files:**
- Create: `crates/artifax-server/src/routes/shell.rs`, `web/dist/.gitkeep`
- Modify: `crates/artifax-server/src/routes/mod.rs`, `crates/artifax-server/tests/api_content.rs`

**Interfaces:**
- Produces: `GET /`, `/a/{aid}`, `/a/{aid}/v/{n}` → `web/dist/index.html` (built by Task 16 onward; until then a committed placeholder is not used: when the file is absent the route returns 503 `ui_not_built` with a message naming `just web`). `GET /_artifax/{*path}` → embedded static file with content type from extension and `Cache-Control: public, max-age=3600`; `bridge.js` and `shell/*` live there.
- `Assets` is `#[derive(RustEmbed)] #[folder = "../../web/dist/"]`. In debug builds rust-embed reads from disk at request time, so `just web` output is picked up without a rebuild.

- [ ] **Step 1: Write the failing tests**

Append to `tests/api_content.rs`:
```rust
#[tokio::test]
async fn shell_routes_serve_ui_or_explain_missing_build() {
    let ts = TestServer::spawn().await;
    for path in ["/", "/a/7q3k9mzx2b4t", "/a/7q3k9mzx2b4t/v/2"] {
        let res = ts.get(path).await;
        let status = res.status().as_u16();
        assert!(status == 200 || status == 503, "{path} → {status}");
        if status == 200 { assert!(res.headers()["content-type"].to_str().unwrap().starts_with("text/html")); }
        else { assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "ui_not_built"); }
    }
    assert_eq!(ts.get("/_artifax/does-not-exist.js").await.status(), 404);
}
```

- [ ] **Step 2: Implement**

```rust
//! Serves the embedded web UI: the shell document and static assets.

use crate::error::ApiError;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

pub async fn shell() -> Result<Response, ApiError> {
    match Assets::get("index.html") {
        Some(f) => Ok(([(header::CACHE_CONTROL, "no-store")], Html(f.data.into_owned())).into_response()),
        None => Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "ui_not_built", "the web UI has not been built; run `just web`")),
    }
}

pub async fn static_file(Path(path): Path<String>) -> Result<Response, ApiError> {
    let f = Assets::get(&format!("_artifax/{path}")).ok_or_else(ApiError::not_found)?;
    let ct = mime_guess::from_path(&path).first_or_octet_stream().to_string();
    let ct = if path.ends_with(".js") { "text/javascript".to_string() } else { ct };
    Ok(([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "public, max-age=3600".to_string())], f.data.into_owned()).into_response())
}
```

Routes: `.route("/", get(shell::shell)).route("/a/{aid}", get(shell::shell)).route("/a/{aid}/v/{n}", get(shell::shell)).route("/_artifax/{*path}", get(shell::static_file))`. Create `web/dist/.gitkeep` (empty).

- [ ] **Step 3: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Serve the embedded web UI"
```

---

### Task 14: CLI client with discovery and auto-start; serve, stop, status

**Files:**
- Create: `crates/artifax-cli/Cargo.toml`, `src/main.rs`, `src/client.rs`, `src/commands/mod.rs`, `src/commands/serve.rs`, `src/commands/stop.rs`, `src/commands/status.rs`, `tests/cli.rs`
- Binary name: `artifax`.

**Interfaces:**
- Produces:
  - `Client { base: String, token: String, http: reqwest::blocking::Client }` with `Client::connect(home: &Home) -> anyhow::Result<Client>` (discover or auto-start), `Client::discover(home) -> Option<Client>` (no auto-start), `Client::healthz(&self) -> anyhow::Result<serde_json::Value>`, `Client::get(&self, path) -> Result<serde_json::Value>`, `Client::post(&self, path, body) -> Result<serde_json::Value>`, `Client::patch`, `Client::delete(&self, path) -> Result<()>`, `Client::shutdown(&self) -> Result<()>`, `Client::browser_url(&self, path) -> String` (`http://localhost:<port><path>`).
  - Auto-start: `DaemonLock::acquire`, re-check discovery, spawn `std::env::current_exe()` with `serve --foreground`, stdin null, stdout and stderr appended to `home.log_path()`, `process_group(0)`, env `ARTIFAX_HOME` set to the home root, then poll `daemon.json` + `/healthz` every 100 ms for 5 s.
  - Commands: `artifax serve [--bind <ip>] [--port <n>] [--foreground]` (default: ensure a daemon is running in the background and print its URL; `--foreground` runs it in this process), `artifax stop`, `artifax status [--json]`.
  - Global flag `--json` on every command switches output to one JSON object on stdout; errors go to stderr as `error: <message>` with exit code 1.

- [ ] **Step 1: Write Cargo.toml and the failing tests**

`crates/artifax-cli/Cargo.toml`:
```toml
[package]
name = "artifax-cli"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "artifax"
path = "src/main.rs"

[dependencies]
artifax-core = { path = "../artifax-core" }
artifax-server = { path = "../artifax-server" }
anyhow.workspace = true
clap.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing-subscriber.workspace = true
base64.workspace = true

[dev-dependencies]
assert_cmd.workspace = true
predicates.workspace = true
tempfile.workspace = true
```

`tests/cli.rs`:
```rust
use assert_cmd::Command;
use predicates::prelude::*;

struct Env { dir: tempfile::TempDir }
impl Env {
    fn new() -> Env { Env { dir: tempfile::tempdir().unwrap() } }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("artifax").unwrap();
        c.env("ARTIFAX_HOME", self.dir.path().join("ax")).env("HOME", self.dir.path());
        c
    }
    fn stop(&self) { self.cmd().arg("stop").assert().success(); }
}

#[test]
fn status_without_daemon_reports_not_running() {
    let e = Env::new();
    e.cmd().args(["status", "--json"]).assert().success().stdout(predicate::str::contains("\"running\":false"));
}

#[test]
fn serve_starts_a_background_daemon_and_stop_ends_it() {
    let e = Env::new();
    let out = e.cmd().args(["serve", "--json", "--port", "0"]).assert().success().get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["running"], true);
    let port = v["port"].as_u64().unwrap();
    assert!(port > 0);
    let again: serde_json::Value = serde_json::from_slice(&e.cmd().args(["serve", "--json", "--port", "0"]).assert().success().get_output().stdout).unwrap();
    assert_eq!(again["port"].as_u64().unwrap(), port, "second serve reuses the running daemon");
    let st: serde_json::Value = serde_json::from_slice(&e.cmd().args(["status", "--json"]).assert().success().get_output().stdout).unwrap();
    assert_eq!(st["running"], true);
    assert!(st["url"].as_str().unwrap().starts_with("http://localhost:"));
    e.stop();
    let st: serde_json::Value = serde_json::from_slice(&e.cmd().args(["status", "--json"]).assert().success().get_output().stdout).unwrap();
    assert_eq!(st["running"], false);
}

#[test]
fn concurrent_auto_starts_yield_one_daemon() {
    let e = Env::new();
    let handles: Vec<_> = (0..4).map(|_| {
        let home = e.dir.path().join("ax");
        let hd = e.dir.path().to_path_buf();
        std::thread::spawn(move || {
            let mut c = Command::cargo_bin("artifax").unwrap();
            c.env("ARTIFAX_HOME", home).env("HOME", hd).args(["status", "--start", "--json", "--port", "0"]);
            let out = c.assert().success().get_output().stdout.clone();
            serde_json::from_slice::<serde_json::Value>(&out).unwrap()["pid"].as_u64().unwrap()
        })
    }).collect();
    let pids: std::collections::HashSet<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(pids.len(), 1, "all callers found the same daemon");
    e.stop();
}
```

`--port 0` in tests makes the OS pick a free port so parallel test runs do not collide on 7480; `serve --port 0` passes it straight to the bind. `status --start` performs discovery with auto-start (it is the only status form that starts anything).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-cli`
Expected: binary has no subcommands; assertions fail.

- [ ] **Step 3: Implement client.rs**

```rust
//! HTTP client for the daemon, with discovery and auto-start.

use anyhow::{Context, anyhow, bail};
use artifax_core::Home;
use artifax_server::daemon::{DaemonInfo, DaemonLock, pid_alive, read_daemon_info};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Client {
    pub base: String,
    pub token: String,
    pub info: DaemonInfo,
    http: reqwest::blocking::Client,
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build().expect("client")
}

impl Client {
    fn from_info(info: DaemonInfo) -> Client {
        Client { base: format!("http://127.0.0.1:{}", info.port), token: info.token.clone(), info, http: http() }
    }

    /// A live daemon named by daemon.json, or None. Never starts one.
    pub fn discover(home: &Home) -> Option<Client> {
        let info = read_daemon_info(home)?;
        if !pid_alive(info.pid) { return None; }
        let c = Client::from_info(info);
        let probe = reqwest::blocking::Client::builder().timeout(Duration::from_secs(1)).build().ok()?;
        let res = probe.get(format!("{}/healthz", c.base)).send().ok()?;
        res.status().is_success().then_some(c)
    }

    /// Discover, or start a daemon on `port` (0 = any free port) and wait for it.
    pub fn connect(home: &Home, port: u16) -> anyhow::Result<Client> {
        if let Some(c) = Client::discover(home) { return Ok(c); }
        home.ensure_dirs()?;
        let _lock = DaemonLock::acquire(home).context("acquiring daemon lock")?;
        if let Some(c) = Client::discover(home) { return Ok(c); }
        let log = std::fs::OpenOptions::new().create(true).append(true).open(home.log_path())?;
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.args(["serve", "--foreground", "--port", &port.to_string()])
            .env("ARTIFAX_HOME", home.root())
            .stdin(Stdio::null()).stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log));
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn().context("spawning artifax serve")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(c) = Client::discover(home) {
                if c.info.pid == child.id() { return Ok(c); }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("daemon did not become ready within 5s; see {}", home.log_path().display())
    }

    pub fn browser_url(&self, path: &str) -> String {
        format!("http://localhost:{}{}", self.info.port, path)
    }

    fn check(res: reqwest::blocking::Response) -> anyhow::Result<serde_json::Value> {
        let status = res.status();
        if status == reqwest::StatusCode::NO_CONTENT { return Ok(serde_json::json!({})); }
        let body: serde_json::Value = res.json().unwrap_or(serde_json::json!({}));
        if status.is_success() { Ok(body) } else {
            let code = body["error"]["code"].as_str().unwrap_or("error");
            let msg = body["error"]["message"].as_str().unwrap_or("request failed");
            Err(anyhow!("{code}: {msg}"))
        }
    }

    pub fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        Self::check(self.http.get(format!("{}{path}", self.base)).send()?)
    }
    pub fn post(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::check(self.http.post(format!("{}{path}", self.base)).bearer_auth(&self.token).json(body).send()?)
    }
    pub fn patch(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::check(self.http.patch(format!("{}{path}", self.base)).bearer_auth(&self.token).json(body).send()?)
    }
    pub fn delete(&self, path: &str) -> anyhow::Result<()> {
        Self::check(self.http.delete(format!("{}{path}", self.base)).bearer_auth(&self.token).send()?).map(|_| ())
    }
    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.post("/api/admin/shutdown", &serde_json::json!({})).map(|_| ())
    }
}
```

- [ ] **Step 4: Implement main.rs and the three commands**

`src/main.rs`:
```rust
//! The `artifax` command line: run the daemon and manage artifacts.

mod client;
mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "artifax", version, about = "Local artifacts with comment-driven development")]
pub struct Cli {
    /// Emit one JSON object on stdout instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Port to use when starting a daemon (0 = any free port).
    #[arg(long, global = true, default_value_t = artifax_server::daemon::DEFAULT_PORT)]
    pub port: u16,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Start the daemon in the background (or run it here with --foreground).
    Serve(commands::serve::Args),
    /// Stop the running daemon.
    Stop,
    /// Show whether a daemon is running.
    Status(commands::status::Args),
}

fn main() {
    let cli = Cli::parse();
    let home = artifax_core::Home::from_env();
    let result = match &cli.cmd {
        Cmd::Serve(a) => commands::serve::run(&cli, &home, a),
        Cmd::Stop => commands::stop::run(&cli, &home),
        Cmd::Status(a) => commands::status::run(&cli, &home, a),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
```

`src/commands/mod.rs`:
```rust
pub mod serve;
pub mod status;
pub mod stop;

use crate::client::Client;

pub fn daemon_json(c: &Client) -> serde_json::Value {
    serde_json::json!({"running": true, "port": c.info.port, "pid": c.info.pid, "url": c.browser_url("/"), "version": c.info.version, "bind": c.info.bind})
}

pub fn print(cli: &crate::Cli, json: serde_json::Value, text: impl FnOnce(&serde_json::Value) -> String) {
    if cli.json { println!("{json}"); } else { println!("{}", text(&json)); }
}
```

`src/commands/serve.rs`:
```rust
use crate::client::Client;
use artifax_core::Home;
use artifax_server::daemon::{ServeConfig, serve};
use std::net::IpAddr;

#[derive(clap::Args)]
pub struct Args {
    /// Address to bind (use 0.0.0.0 for LAN access).
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
    /// Run the daemon in this process instead of the background.
    #[arg(long)]
    pub foreground: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.foreground {
        tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?)).init();
        let rt = tokio::runtime::Runtime::new()?;
        let cfg = ServeConfig { home: home.clone(), bind: a.bind, port: cli.port, version: env!("CARGO_PKG_VERSION") };
        return rt.block_on(serve(cfg, None));
    }
    let c = Client::connect(home, cli.port)?;
    super::print(cli, super::daemon_json(&c), |j| format!("artifax daemon running at {} (pid {})", j["url"].as_str().unwrap(), j["pid"]));
    Ok(())
}
```
`--bind` in background mode is passed through to the spawned `serve --foreground` by extending `Client::connect` with a `bind: IpAddr` parameter; default `127.0.0.1`.

`src/commands/stop.rs`:
```rust
use crate::client::Client;
use artifax_core::Home;

pub fn run(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    match Client::discover(home) {
        Some(c) => {
            c.shutdown()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline && artifax_server::daemon::pid_alive(c.info.pid) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            super::print(cli, serde_json::json!({"stopped": true, "pid": c.info.pid}), |_| "artifax daemon stopped".into());
        }
        None => super::print(cli, serde_json::json!({"stopped": false}), |_| "no artifax daemon is running".into()),
    }
    Ok(())
}
```

`src/commands/status.rs`:
```rust
use crate::client::Client;
use artifax_core::Home;

#[derive(clap::Args)]
pub struct Args {
    /// Start a daemon if none is running.
    #[arg(long)]
    pub start: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = if a.start { Some(Client::connect(home, cli.port)?) } else { Client::discover(home) };
    match c {
        Some(c) => super::print(cli, super::daemon_json(&c), |j| format!("running at {} (pid {}, v{})", j["url"].as_str().unwrap(), j["pid"], j["version"].as_str().unwrap())),
        None => super::print(cli, serde_json::json!({"running": false, "home": home.root()}), |_| format!("not running (home {})", home.root().display())),
    }
    Ok(())
}
```

- [ ] **Step 5: Run to verify pass, then commit**

Run: `cargo test -p artifax-cli && cargo clippy --workspace -- -D warnings`
Expected: 3 tests pass; the concurrent test proves the lock (Review Focus 4).

```bash
git add -A
git commit -m "Add artifax CLI with daemon discovery, auto-start, stop, and status"
```

---

### Task 15: CLI publish, list, open, delete, pin, unpin, doctor

**Files:**
- Create: `src/commands/publish.rs`, `src/commands/list.rs`, `src/commands/open.rs`, `src/commands/delete.rs`, `src/commands/pin.rs`, `src/commands/doctor.rs`
- Modify: `src/main.rs`, `src/commands/mod.rs`, `tests/cli.rs`

**Interfaces:**
- `artifax publish <index.html> [--file <path>[=<published/path>]]... [--dir <dir>] [--id <id> | --url <url>] [--title] [--description] [--icon] [--label] [--if-version <n>]`. With `--dir`, every file under the directory except `index.html` is included at its relative path. When updating and `--if-version` is omitted, the current version is fetched and used (the CLI is for humans, not merges). Text files (by extension: html, css, js, mjs, json, svg, md, txt, csv) are sent `utf8`; others `base64`. Output: `{"id","url","version"}`; text form prints the browser URL.
- `artifax list` → table `ID  V  PINNED  UPDATED  TITLE`; `--json` → `{"artifacts": [...]}`.
- `artifax open <id|url>` → opens `http://localhost:<port>/a/<id>` with `open` on macOS or `xdg-open` elsewhere; `--json` prints `{"url"}` without opening. `id_from(arg)` accepts a bare ID or any URL whose path contains `/a/<id>`.
- `artifax delete <id>`, `artifax pin <id>`, `artifax unpin <id>`.
- `artifax doctor` → checks: home writable, daemon reachable, `daemon.json` mode is 0600, SQLite `PRAGMA integrity_check`, every current version's `index.html` exists on disk, UI built (GET `/` is 200). Output `{"checks": [{"name","ok","detail"}], "ok": bool}`; exit 1 if any check fails.

- [ ] **Step 1: Write the failing tests**

Append to `tests/cli.rs`:
```rust
fn write(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent).unwrap(); }
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn publish_list_pin_open_delete_roundtrip() {
    let e = Env::new();
    let index = write(e.dir.path(), "site/index.html", "<title>Hello</title><p>v1</p>");
    write(e.dir.path(), "site/app.js", "1");
    write(e.dir.path(), "site/img/logo.png", "not-really-png");
    let out = e.cmd().args(["publish", "--json", "--port", "0", "--title", "Hello", "--dir"]).arg(e.dir.path().join("site")).arg(&index)
        .assert().success().get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["version"], 1);
    assert!(v["url"].as_str().unwrap().ends_with(&format!("/a/{id}")));

    let list: serde_json::Value = serde_json::from_slice(&e.cmd().args(["list", "--json"]).assert().success().get_output().stdout).unwrap();
    assert_eq!(list["artifacts"][0]["title"], "Hello");
    let files: serde_json::Value = serde_json::from_slice(&e.cmd().args(["list", "--json", "--files", &id]).assert().success().get_output().stdout).unwrap();
    assert!(files["files"]["img/logo.png"].is_object());

    write(e.dir.path(), "site/index.html", "<p>v2</p>");
    let v2: serde_json::Value = serde_json::from_slice(&e.cmd().args(["publish", "--json", "--id", &id]).arg(&index).assert().success().get_output().stdout).unwrap();
    assert_eq!(v2["version"], 2);

    e.cmd().args(["pin", &id]).assert().success();
    let list: serde_json::Value = serde_json::from_slice(&e.cmd().args(["list", "--json"]).assert().success().get_output().stdout).unwrap();
    assert_eq!(list["artifacts"][0]["pinned"], true);
    e.cmd().args(["unpin", &id]).assert().success();

    let open: serde_json::Value = serde_json::from_slice(&e.cmd().args(["open", "--json", &id]).assert().success().get_output().stdout).unwrap();
    assert!(open["url"].as_str().unwrap().contains(&format!("/a/{id}")));
    let open2: serde_json::Value = serde_json::from_slice(&e.cmd().args(["open", "--json", open["url"].as_str().unwrap()]).assert().success().get_output().stdout).unwrap();
    assert_eq!(open, open2, "open accepts a URL too");

    e.cmd().args(["delete", &id]).assert().success();
    e.cmd().args(["list", "--json"]).assert().success().stdout(predicate::str::contains(&id).not());
    e.cmd().args(["delete", &id]).assert().failure().stderr(predicate::str::contains("not_found"));
    e.stop();
}

#[test]
fn publish_rejects_missing_index_and_bad_id() {
    let e = Env::new();
    e.cmd().args(["publish", "--port", "0"]).arg(e.dir.path().join("nope.html")).assert().failure().stderr(predicate::str::contains("nope.html"));
    e.cmd().args(["open", "not-an-id"]).assert().failure().stderr(predicate::str::contains("invalid_id"));
    e.stop();
}

#[test]
fn doctor_runs_all_checks() {
    let e = Env::new();
    e.cmd().args(["serve", "--port", "0"]).assert().success();
    let out = e.cmd().args(["doctor", "--json"]).assert().get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let names: Vec<&str> = v["checks"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    for n in ["home", "daemon", "daemon_json_mode", "db_integrity", "version_files", "ui"] { assert!(names.contains(&n), "{n}"); }
    e.stop();
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-cli`

- [ ] **Step 3: Implement publish.rs**

```rust
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
    #[arg(long)] pub title: Option<String>,
    #[arg(long)] pub description: Option<String>,
    #[arg(long)] pub icon: Option<String>,
    #[arg(long)] pub label: Option<String>,
    /// Expected current version; defaults to the artifact's current version.
    #[arg(long)]
    pub if_version: Option<u32>,
}

const TEXT_EXT: &[&str] = &["html", "htm", "css", "js", "mjs", "json", "svg", "md", "txt", "csv", "xml", "map"];

pub fn file_entry(path: &Path) -> anyhow::Result<serde_json::Value> {
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    Ok(if TEXT_EXT.contains(&ext.as_str()) {
        match String::from_utf8(bytes) {
            Ok(s) => serde_json::json!({"content": s, "encoding": "utf8"}),
            Err(e) => serde_json::json!({"content": base64::engine::general_purpose::STANDARD.encode(e.into_bytes()), "encoding": "base64"}),
        }
    } else {
        serde_json::json!({"content": base64::engine::general_purpose::STANDARD.encode(bytes), "encoding": "base64"})
    })
}

fn collect_dir(dir: &Path, base: &Path, out: &mut serde_json::Map<String, serde_json::Value>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() { collect_dir(&p, base, out)?; continue; }
        let rel = p.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
        if rel == "index.html" { continue; }
        out.insert(rel, file_entry(&p)?);
    }
    Ok(())
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let mut files = serde_json::Map::new();
    files.insert("index.html".into(), file_entry(&a.index)?);
    if let Some(dir) = &a.dir { collect_dir(dir, dir, &mut files)?; }
    for spec in &a.files {
        let (src, dest) = match spec.split_once('=') { Some((s, d)) => (PathBuf::from(s), d.to_string()),
            None => (PathBuf::from(spec), Path::new(spec).file_name().unwrap().to_string_lossy().to_string()) };
        files.insert(dest, file_entry(&src)?);
    }
    let c = Client::connect(home, cli.port)?;
    let target = match (&a.id, &a.url) { (Some(id), _) => Some(super::open::id_from(id)?), (_, Some(u)) => Some(super::open::id_from(u)?), _ => None };
    let mut body = serde_json::json!({"files": files});
    for (k, v) in [("title", &a.title), ("description", &a.description), ("icon", &a.icon), ("label", &a.label)] {
        if let Some(v) = v { body[k] = serde_json::json!(v); }
    }
    let res = match target {
        None => c.post("/api/artifacts", &body)?,
        Some(id) => {
            let current = match a.if_version { Some(v) => v, None => c.get(&format!("/api/artifacts/{id}"))?["artifact"]["current_version"].as_u64().unwrap() as u32 };
            body["if_version"] = serde_json::json!(current);
            c.post(&format!("/api/artifacts/{id}/versions"), &body)?
        }
    };
    let id = res["artifact"]["id"].as_str().unwrap().to_string();
    let url = c.browser_url(&format!("/a/{id}"));
    super::print(cli, serde_json::json!({"id": id, "url": url, "version": res["version"]["n"]}),
        |j| format!("published v{} at {}", j["version"], j["url"].as_str().unwrap()));
    Ok(())
}
```

- [ ] **Step 4: Implement list, open, delete, pin, doctor**

`src/commands/open.rs`:
```rust
use crate::client::Client;
use artifax_core::{ArtifactId, Home};

#[derive(clap::Args)]
pub struct Args { pub target: String }

/// Accepts a bare artifact ID or any URL whose path contains `/a/<id>`.
pub fn id_from(s: &str) -> anyhow::Result<String> {
    let candidate = match s.find("/a/") { Some(i) => s[i + 3..].split(['/', '?', '#']).next().unwrap_or(""), None => s };
    Ok(ArtifactId::parse(candidate).map_err(|e| anyhow::anyhow!("invalid_id: {e}"))?.as_str().to_string())
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = id_from(&a.target)?;
    let c = Client::connect(home, cli.port)?;
    c.get(&format!("/api/artifacts/{id}"))?;
    let url = c.browser_url(&format!("/a/{id}"));
    if !cli.json {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        std::process::Command::new(opener).arg(&url).spawn().map_err(|e| anyhow::anyhow!("cannot run {opener}: {e}"))?;
    }
    super::print(cli, serde_json::json!({"url": url}), |j| j["url"].as_str().unwrap().to_string());
    Ok(())
}
```

`src/commands/list.rs`:
```rust
use crate::client::Client;
use artifax_core::Home;

#[derive(clap::Args)]
pub struct Args {
    /// Show the current version's files for this artifact instead.
    #[arg(long)]
    pub files: Option<String>,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port)?;
    if let Some(target) = &a.files {
        let id = super::open::id_from(target)?;
        let res = c.get(&format!("/api/artifacts/{id}/files"))?;
        super::print(cli, res, |j| j["files"].as_object().unwrap().iter()
            .map(|(k, v)| format!("{:>8}  {:<24} {}", v["size"], v["content_type"].as_str().unwrap_or(""), k)).collect::<Vec<_>>().join("\n"));
        return Ok(());
    }
    let res = c.get("/api/artifacts")?;
    super::print(cli, res, |j| {
        let mut lines = vec![format!("{:<12}  {:>3}  {:<6}  {:<24}  {}", "ID", "V", "PINNED", "UPDATED", "TITLE")];
        for a in j["artifacts"].as_array().unwrap() {
            lines.push(format!("{:<12}  {:>3}  {:<6}  {:<24}  {}", a["id"].as_str().unwrap(), a["current_version"],
                if a["pinned"].as_bool().unwrap_or(false) { "yes" } else { "" }, a["updated_at"].as_str().unwrap_or(""), a["title"].as_str().unwrap_or("")));
        }
        lines.join("\n")
    });
    Ok(())
}
```

`src/commands/delete.rs` and `src/commands/pin.rs`:
```rust
// delete.rs
use crate::client::Client;
use artifax_core::Home;
#[derive(clap::Args)]
pub struct Args { pub target: String }
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let id = super::open::id_from(&a.target)?;
    let c = Client::connect(home, cli.port)?;
    c.delete(&format!("/api/artifacts/{id}"))?;
    super::print(cli, serde_json::json!({"deleted": id}), |j| format!("deleted {}", j["deleted"].as_str().unwrap()));
    Ok(())
}

// pin.rs
use crate::client::Client;
use artifax_core::Home;
#[derive(clap::Args)]
pub struct Args { pub target: String }
pub fn run(cli: &crate::Cli, home: &Home, a: &Args, pinned: bool) -> anyhow::Result<()> {
    let id = super::open::id_from(&a.target)?;
    let c = Client::connect(home, cli.port)?;
    c.patch(&format!("/api/artifacts/{id}"), &serde_json::json!({"pinned": pinned}))?;
    super::print(cli, serde_json::json!({"id": id, "pinned": pinned}), |j| format!("{} {}", if pinned { "pinned" } else { "unpinned" }, j["id"].as_str().unwrap()));
    Ok(())
}
```

`src/commands/doctor.rs`:
```rust
use crate::client::Client;
use artifax_core::{ArtifactId, Home, Store};
use std::os::unix::fs::PermissionsExt;

fn check(name: &str, ok: bool, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"name": name, "ok": ok, "detail": detail.into()})
}

pub fn run(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let mut checks = vec![];
    let writable = home.ensure_dirs().is_ok() && std::fs::write(home.root().join(".doctor"), b"").map(|_| std::fs::remove_file(home.root().join(".doctor")).is_ok()).unwrap_or(false);
    checks.push(check("home", writable, home.root().display().to_string()));
    let client = Client::discover(home);
    checks.push(check("daemon", client.is_some(), client.as_ref().map(|c| c.base.clone()).unwrap_or("not running".into())));
    let mode = std::fs::metadata(home.daemon_json()).map(|m| m.permissions().mode() & 0o777).ok();
    checks.push(check("daemon_json_mode", mode.is_none_or(|m| m == 0o600), mode.map(|m| format!("{m:o}")).unwrap_or("absent".into())));
    match Store::open(home) {
        Ok(store) => {
            let integrity = store.integrity_check();
            checks.push(check("db_integrity", integrity.as_deref() == Ok("ok"), integrity.unwrap_or_else(|e| e.to_string())));
            let mut missing = vec![];
            for a in store.list_artifacts()? {
                let id = ArtifactId::parse(&a.id)?;
                if !home.version_dir(&id, a.current_version).join("index.html").exists() { missing.push(a.id.clone()); }
            }
            checks.push(check("version_files", missing.is_empty(), if missing.is_empty() { "all current versions present".into() } else { missing.join(", ") }));
        }
        Err(e) => checks.push(check("db_integrity", false, e.to_string())),
    }
    let ui = client.as_ref().map(|c| c.http_status("/") == Some(200)).unwrap_or(false);
    checks.push(check("ui", ui, if ui { "shell served" } else { "UI not built or daemon down; run `just web`" }));
    let ok = checks.iter().all(|c| c["ok"].as_bool().unwrap());
    super::print(cli, serde_json::json!({"ok": ok, "checks": checks}), |j| j["checks"].as_array().unwrap().iter()
        .map(|c| format!("{} {:<18} {}", if c["ok"].as_bool().unwrap() { "ok  " } else { "FAIL" }, c["name"].as_str().unwrap(), c["detail"].as_str().unwrap())).collect::<Vec<_>>().join("\n"));
    if !ok { std::process::exit(1); }
    Ok(())
}
```

Supporting additions: `Store::integrity_check(&self) -> Result<String>` in `artifax-core/src/store/mod.rs` (`PRAGMA integrity_check` first row as text), and `Client::http_status(&self, path) -> Option<u16>` in `client.rs` (GET without auth, returns status code). Wire the new subcommands into `main.rs`:
```rust
/// Publish a page (and files) as a new artifact or a new version.
Publish(commands::publish::Args),
/// List artifacts, or the files of one.
List(commands::list::Args),
/// Open an artifact in the browser.
Open(commands::open::Args),
/// Delete an artifact.
Delete(commands::delete::Args),
/// Pin an artifact to the top of the gallery.
Pin(commands::pin::Args),
/// Unpin an artifact.
Unpin(commands::pin::Args),
/// Check the installation and storage.
Doctor,
```

- [ ] **Step 5: Run to verify pass, then commit**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`

```bash
git add -A
git commit -m "Add publish, list, open, delete, pin, and doctor commands"
```

---

### Task 16: Web scaffold and the bridge script

**Files:**
- Create: `web/package.json`, `web/tsconfig.json`, `web/vite.shell.config.ts`, `web/vite.bridge.config.ts`, `web/vitest.config.ts`, `web/bridge/src/bridge.ts`, `web/bridge/test/bridge.test.ts`, `justfile`

**Interfaces:**
- Produces: `web/dist/_artifax/bridge.js` (IIFE, no exports) that defines `window.claude = Object.freeze({ use })` where `use(name: string): Promise<null>` resolves on a microtask, is memoised per name, never rejects, and reads `data-artifact`, `data-version`, `data-contract` from its own `<script>` tag into `window.__artifax = { artifact, version, contract }` for later phases. `just web` builds both bundles into `web/dist`; `just web-test` runs vitest; `just web-e2e` runs Playwright (Task 19).

- [ ] **Step 1: Write the web manifests**

`web/package.json`:
```json
{
  "name": "artifax-web",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "vite build -c vite.bridge.config.ts && vite build -c vite.shell.config.ts",
    "dev": "vite -c vite.shell.config.ts",
    "test": "vitest run",
    "typecheck": "tsc --noEmit",
    "e2e": "playwright test"
  },
  "devDependencies": {
    "@playwright/test": "^1.50.0",
    "@preact/preset-vite": "^2.10.0",
    "jsdom": "^26.0.0",
    "typescript": "^5.7.0",
    "vite": "^6.0.0",
    "vitest": "^3.0.0"
  },
  "dependencies": {
    "preact": "^10.25.0"
  }
}
```

`web/tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2022", "module": "ESNext", "moduleResolution": "Bundler", "strict": true,
    "jsx": "react-jsx", "jsxImportSource": "preact", "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "noEmit": true, "skipLibCheck": true, "types": ["vite/client"]
  },
  "include": ["shell/src", "bridge/src", "bridge/test", "e2e"]
}
```

`web/vite.bridge.config.ts`:
```ts
import { defineConfig } from "vite";
export default defineConfig({
  build: {
    outDir: "dist/_artifax", emptyOutDir: false,
    lib: { entry: "bridge/src/bridge.ts", name: "artifaxBridge", formats: ["iife"], fileName: () => "bridge.js" },
    minify: true, sourcemap: false,
  },
});
```

`web/vite.shell.config.ts`:
```ts
import { defineConfig } from "vite";
import preact from "@preact/preset-vite";
export default defineConfig({
  root: "shell", base: "/_artifax/shell/", plugins: [preact()],
  build: { outDir: "../dist", emptyOutDir: false, assetsDir: "shell", rollupOptions: { input: "shell/index.html" } },
  server: { proxy: { "/api": "http://127.0.0.1:7480", "/c": "http://127.0.0.1:7480", "/_blob": "http://127.0.0.1:7480", "/healthz": "http://127.0.0.1:7480" } },
});
```
Vite writes `dist/index.html` from `shell/index.html` because `root` is `shell`; verify after the first build that the file lands at `web/dist/index.html` (not `web/dist/shell/index.html`) and adjust `rollupOptions.input` to an absolute path if not.

`web/vitest.config.ts`:
```ts
import { defineConfig } from "vitest/config";
export default defineConfig({ test: { environment: "jsdom", include: ["bridge/test/**/*.test.ts", "shell/src/**/*.test.ts"] } });
```

`justfile` (repo root):
```
set shell := ["bash", "-cu"]

default: build

build:
    cargo build --workspace

web-install:
    cd web && npm ci

web: web-install
    cd web && npm run build

web-test: web-install
    cd web && npm run typecheck && npm test

web-e2e: web
    cd web && npx playwright install --with-deps chromium && npm run e2e

test:
    cargo test --workspace

ci:
    scripts/quality_gates.sh
```

- [ ] **Step 2: Write the failing bridge test**

`web/bridge/test/bridge.test.ts`:
```ts
import { describe, it, expect, beforeEach } from "vitest";

declare global { interface Window { claude?: { use(name: string): Promise<unknown> }; __artifax?: { artifact: string; version: number; contract: string } } }

async function loadBridge() {
  document.body.innerHTML = "";
  const script = document.createElement("script");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "3"; script.dataset.contract = "0.2.61";
  document.body.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  delete (window as any).claude;
  await import("../src/bridge.ts?" + Math.random());
}

describe("bridge", () => {
  beforeEach(loadBridge);

  it("exposes only use() and resolves null for every capability", async () => {
    expect(Object.keys(window.claude!)).toEqual(["use"]);
    for (const name of ["db", "artifact", "permissions", "room", "sample", "nonsense"]) {
      await expect(window.claude!.use(name)).resolves.toBeNull();
    }
  });

  it("is frozen and memoised", async () => {
    expect(Object.isFrozen(window.claude)).toBe(true);
    expect(window.claude!.use("db")).toBe(window.claude!.use("db"));
    expect(() => { (window as any).claude = {}; }).toThrow();
  });

  it("reads its metadata from the script tag", () => {
    expect(window.__artifax).toEqual({ artifact: "7q3k9mzx2b4t", version: 3, contract: "0.2.61" });
  });
});
```

- [ ] **Step 3: Run to verify failure**

Run: `cd web && npm install && npm test`
Expected: fail, `bridge.ts` missing.

- [ ] **Step 4: Implement the bridge**

`web/bridge/src/bridge.ts`:
```ts
/**
 * Runtime bridge injected into every published page.
 * Phase 1 exposes `window.claude.use(name)` and resolves `null` for every
 * capability, so pages written against the claude.ai contract load and
 * degrade correctly. Later phases add the shell handshake.
 */
(() => {
  const script = document.currentScript as HTMLScriptElement | null;
  const meta = {
    artifact: script?.dataset.artifact ?? "",
    version: Number(script?.dataset.version ?? "0"),
    contract: script?.dataset.contract ?? "",
  };
  (window as any).__artifax = meta;

  const cache = new Map<string, Promise<null>>();
  function use(name: string): Promise<null> {
    let p = cache.get(name);
    if (!p) {
      p = Promise.resolve().then(() => null);
      cache.set(name, p);
    }
    return p;
  }

  Object.defineProperty(window, "claude", {
    value: Object.freeze({ use }),
    writable: false,
    configurable: false,
    enumerable: true,
  });
})();
```

- [ ] **Step 5: Run to verify pass, build, commit**

Run: `cd web && npm test && npm run build && ls dist/_artifax/bridge.js`
Expected: 3 tests pass; `bridge.js` exists (the shell build fails until Task 17 adds `shell/index.html`; run only the bridge config for now: `npx vite build -c vite.bridge.config.ts`).

```bash
git add -A
git commit -m "Add web build scaffold and the phase 1 bridge script"
```

---

### Task 17: Gallery

**Files:**
- Create: `web/shell/index.html`, `web/shell/src/main.tsx`, `web/shell/src/api.ts`, `web/shell/src/theme.css`, `web/shell/src/gallery.tsx`, `web/shell/src/gallery.test.tsx`, `web/shell/src/format.ts`

**Interfaces:**
- Produces: `api.ts` with `type Artifact`, `type Version`, `listArtifacts(): Promise<Artifact[]>`, `getArtifact(id): Promise<{artifact: Artifact; versions: Version[]}>`, `getToken(): Promise<string | null>` (cached; `null` when 403), `patchArtifact(id, patch, token)`, `deleteArtifact(id, token)`. `format.ts` with `relativeTime(iso: string, now?: Date): string` ("just now", "5 min ago", "3 h ago", "2 d ago", else `YYYY-MM-DD`). `gallery.tsx` default export `<Gallery/>`. `main.tsx` routes on `location.pathname`: `/` → Gallery, `/a/<id>[/v/<n>]` → Artifact (Task 18).
- Visual contract: CSS tokens on `:root` (`--bg`, `--fg`, `--muted`, `--card`, `--border`, `--accent`), dark mode under `@media (prefers-color-scheme: dark)`, 16 px side gutters, cards in a responsive grid (`repeat(auto-fill, minmax(260px, 1fr))`), body has an explicit background.

- [ ] **Step 1: Write the failing tests**

`web/shell/src/gallery.test.tsx`:
```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "preact";
import { relativeTime } from "./format";

describe("relativeTime", () => {
  const now = new Date("2026-09-28T12:00:00Z");
  it("buckets", () => {
    expect(relativeTime("2026-09-28T11:59:40Z", now)).toBe("just now");
    expect(relativeTime("2026-09-28T11:55:00Z", now)).toBe("5 min ago");
    expect(relativeTime("2026-09-28T09:00:00Z", now)).toBe("3 h ago");
    expect(relativeTime("2026-09-26T12:00:00Z", now)).toBe("2 d ago");
    expect(relativeTime("2026-01-01T00:00:00Z", now)).toBe("2026-01-01");
  });
});

describe("Gallery", () => {
  beforeEach(() => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [
        { id: "7q3k9mzx2b4t", title: "Pinned one", description: "d", icon: "chart", pinned: true, current_version: 3, updated_at: "2026-09-28T11:00:00Z" },
        { id: "aaaaaaaaaaaa", title: "Other", description: null, icon: null, pinned: false, current_version: 1, updated_at: "2026-09-27T11:00:00Z" },
      ] }));
      if (url.endsWith("/api/token")) return new Response(JSON.stringify({ token: "t" }));
      return new Response("{}", { status: 404 });
    }));
  });

  it("renders cards with title, version, and link, pinned first", async () => {
    const { default: Gallery } = await import("./gallery");
    const root = document.createElement("div");
    render(<Gallery />, root);
    await new Promise(r => setTimeout(r, 0));
    const cards = root.querySelectorAll("a.card");
    expect(cards.length).toBe(2);
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[0].textContent).toContain("Pinned one");
    expect(cards[0].textContent).toContain("v3");
    expect(cards[0].querySelector(".pin")).not.toBeNull();
    expect(root.textContent).toContain("published from the command line");
  });

  it("shows an empty state", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ artifacts: [] }))));
    const { default: Gallery } = await import("./gallery");
    const root = document.createElement("div");
    render(<Gallery />, root);
    await new Promise(r => setTimeout(r, 0));
    expect(root.textContent).toContain("No artifacts yet");
    expect(root.textContent).toContain("artifax publish");
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd web && npm test`

- [ ] **Step 3: Implement api.ts, format.ts, theme.css, index.html, main.tsx, gallery.tsx**

`web/shell/src/api.ts`:
```ts
export type Artifact = {
  id: string; title: string; description: string | null; icon: string | null;
  created_at?: string; updated_at: string; current_version: number; pinned: boolean;
  capabilities?: Record<string, unknown>; contract_version?: string; owner_session_id?: string | null;
};
export type FileMeta = { content_type: string; size: number };
export type Version = { artifact_id: string; n: number; label: string | null; created_at: string; files: Record<string, FileMeta> };

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let msg = res.statusText;
    try { msg = (await res.json()).error?.message ?? msg; } catch { /* not json */ }
    throw new Error(`${res.status} ${msg}`);
  }
  return res.json() as Promise<T>;
}

export async function listArtifacts(): Promise<Artifact[]> {
  return (await json<{ artifacts: Artifact[] }>(await fetch("/api/artifacts"))).artifacts;
}
export async function getArtifact(id: string): Promise<{ artifact: Artifact; versions: Version[] }> {
  return json(await fetch(`/api/artifacts/${id}`));
}

let tokenPromise: Promise<string | null> | null = null;
/** The write token, only served to loopback browsers; null on a LAN viewer. */
export function getToken(): Promise<string | null> {
  if (!tokenPromise) {
    tokenPromise = fetch("/api/token").then(async r => (r.ok ? (await r.json()).token as string : null)).catch(() => null);
  }
  return tokenPromise;
}
export async function patchArtifact(id: string, patch: Partial<Pick<Artifact, "title" | "description" | "icon" | "pinned">>, token: string): Promise<Artifact> {
  return (await json<{ artifact: Artifact }>(await fetch(`/api/artifacts/${id}`, { method: "PATCH", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(patch) }))).artifact;
}
export async function deleteArtifact(id: string, token: string): Promise<void> {
  await json<unknown>(await fetch(`/api/artifacts/${id}`, { method: "DELETE", headers: { authorization: `Bearer ${token}` } }).then(r => (r.status === 204 ? new Response("{}") : r)));
}
```

`web/shell/src/format.ts`:
```ts
export function relativeTime(iso: string, now: Date = new Date()): string {
  const t = new Date(iso).getTime();
  const s = Math.max(0, Math.round((now.getTime() - t) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  if (s < 7 * 86400) return `${Math.round(s / 86400)} d ago`;
  return iso.slice(0, 10);
}
```

`web/shell/src/theme.css`:
```css
:root {
  --bg: #f6f5f2; --fg: #1d1c1a; --muted: #6b6864; --card: #ffffff; --border: #e3e0da; --accent: #c2410c;
  --radius: 10px; --gutter: 16px; color-scheme: light dark;
}
:root:not([data-theme="light"]) { @media (prefers-color-scheme: dark) {
  --bg: #161513; --fg: #ecebe7; --muted: #9a968f; --card: #1f1e1b; --border: #2d2b27; --accent: #fb923c;
} }
:root[data-theme="dark"] { --bg: #161513; --fg: #ecebe7; --muted: #9a968f; --card: #1f1e1b; --border: #2d2b27; --accent: #fb923c; }
*, *::before, *::after { box-sizing: border-box; }
html, body { margin: 0; height: 100%; }
body { background: var(--bg); color: var(--fg); font: 15px/1.45 system-ui, -apple-system, "Segoe UI", sans-serif; }
a { color: inherit; text-decoration: none; }
.wrap { max-width: 1200px; margin: 0 auto; padding: 24px var(--gutter); }
.topbar { display: flex; align-items: center; gap: 12px; padding: 12px var(--gutter); border-bottom: 1px solid var(--border); background: var(--card); }
.topbar h1 { font-size: 16px; margin: 0; font-weight: 600; }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 16px; }
.card { display: block; background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 16px; position: relative; }
.card:hover { border-color: var(--accent); }
.card h2 { font-size: 16px; margin: 0 0 6px; }
.card p { margin: 0 0 10px; color: var(--muted); font-size: 14px; }
.card .meta { display: flex; gap: 10px; color: var(--muted); font-size: 12px; }
.card .pin { position: absolute; top: 12px; right: 12px; color: var(--accent); }
.empty { text-align: center; color: var(--muted); padding: 64px 0; }
.empty code { background: var(--card); border: 1px solid var(--border); padding: 2px 6px; border-radius: 6px; }
button { font: inherit; background: var(--card); color: var(--fg); border: 1px solid var(--border); border-radius: 8px; padding: 6px 10px; cursor: pointer; }
button.primary { background: var(--accent); color: #fff; border-color: var(--accent); }
select { font: inherit; background: var(--card); color: var(--fg); border: 1px solid var(--border); border-radius: 8px; padding: 6px 8px; }
.frame { position: absolute; inset: 0; width: 100%; height: 100%; border: 0; background: #fff; }
.viewer { position: relative; height: calc(100vh - 53px); }
.banner { position: absolute; left: 50%; transform: translateX(-50%); top: 12px; background: var(--card); border: 1px solid var(--accent); border-radius: 999px; padding: 8px 14px; display: flex; gap: 10px; align-items: center; box-shadow: 0 6px 24px rgba(0,0,0,.15); }
.muted { color: var(--muted); }
@media (max-width: 480px) { .topbar h1 { font-size: 14px; } .topbar .hide-sm { display: none; } }
```

`web/shell/index.html`:
```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>Artifax</title>
<link rel="stylesheet" href="./src/theme.css">
</head>
<body>
<div id="app"></div>
<script type="module" src="./src/main.tsx"></script>
</body>
</html>
```

`web/shell/src/main.tsx`:
```tsx
import { render } from "preact";
import Gallery from "./gallery";
import ArtifactView from "./artifact";

function route() {
  const m = location.pathname.match(/^\/a\/([0-9a-hj-km-np-tv-z]{12})(?:\/v\/(\d+))?\/?$/);
  if (m) return <ArtifactView id={m[1]} pinnedVersion={m[2] ? Number(m[2]) : null} />;
  return <Gallery />;
}

render(route(), document.getElementById("app")!);
```
(Until Task 18 exists, create `artifact.tsx` with `export default function ArtifactView(_: { id: string; pinnedVersion: number | null }) { return <p>viewer</p>; }` so the shell builds.)

`web/shell/src/gallery.tsx`:
```tsx
import { useEffect, useState } from "preact/hooks";
import { type Artifact, listArtifacts } from "./api";
import { relativeTime } from "./format";

export default function Gallery() {
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { listArtifacts().then(setArtifacts, e => setError(String(e))); }, []);
  return (
    <>
      <header class="topbar"><h1>Artifax</h1><span class="muted hide-sm">local artifacts</span></header>
      <main class="wrap">
        {error && <p class="empty">Could not load artifacts: {error}</p>}
        {artifacts && artifacts.length === 0 && (
          <p class="empty">No artifacts yet. Publish one with <code>artifax publish index.html</code>.</p>
        )}
        {artifacts && artifacts.length > 0 && (
          <div class="grid">
            {artifacts.map(a => (
              <a class="card" href={`/a/${a.id}`} key={a.id}>
                {a.pinned && <span class="pin" title="Pinned">★</span>}
                <h2>{a.title}</h2>
                {a.description && <p>{a.description}</p>}
                <div class="meta">
                  <span>v{a.current_version}</span>
                  <span>{relativeTime(a.updated_at)}</span>
                  <span>{a.owner_session_id ? "published by an agent" : "published from the command line"}</span>
                </div>
              </a>
            ))}
          </div>
        )}
      </main>
    </>
  );
}
```

- [ ] **Step 4: Run tests, build, check in the browser, commit**

Run: `cd web && npm run typecheck && npm test && npm run build && cd .. && cargo run -p artifax-cli -- serve --foreground` then open `http://localhost:7480/` and confirm: empty state renders; after `cargo run -p artifax-cli -- publish <any html>` and a reload, a card appears, in light and dark mode (toggle the OS setting or add `data-theme="dark"` on `<html>` in devtools), and at 375 px width nothing overflows horizontally.

```bash
git add -A
git commit -m "Add the gallery shell"
```

---

### Task 18: Artifact shell: frame modes, origin probe, version picker, live banner

**Files:**
- Create: `web/shell/src/artifact.tsx`, `web/shell/src/frame.tsx`, `web/shell/src/origin.ts`, `web/shell/src/origin.test.ts`, `web/shell/src/events.ts`, `web/shell/src/events.test.ts`
- Modify: `web/shell/src/main.tsx` (remove the placeholder)

**Interfaces:**
- `origin.ts`: `artifactOrigin(id: string, loc: Location = location): string | null` → `http://<id>.localhost:<port>` when `loc.hostname` is `localhost` or `127.0.0.1`, else `null` (LAN: no per-artifact origin). `probeOrigin(origin: string, fetchImpl = fetch, timeoutMs = 1000): Promise<boolean>` → GET `${origin}/healthz` with an `AbortController` timeout; result cached in `sessionStorage` under `artifax.origin-ok` (wrapped in try/catch). `contentSrc(id, n, originOk): string` → `${origin}/v/${n}/` or `/c/${id}/v/${n}/`.
- `events.ts`: `subscribe(artifactId: string, onEvent: (e: {type: string; artifact_id: string; n?: number}) => void): () => void` using `EventSource("/api/events?artifact=<id>")`, listening for `version` and `artifact_deleted`, returning an unsubscribe.
- `frame.tsx`: `<Frame id n originOk onLoad?/>` renders the iframe: with `originOk`, `src` is the per-artifact origin and no `sandbox`; otherwise `src` is the same-origin path with `sandbox="allow-scripts allow-forms allow-modals allow-popups allow-downloads"`. `title` is "artifact content". `key` includes `n` so a version change remounts.
- `artifact.tsx`: `<ArtifactView id pinnedVersion/>`: loads the artifact and versions; shows topbar with a back link, title, version `<select>` (`v3 of 3`; choosing an older version navigates to `/a/<id>/v/<n>`), "open raw" link to the content URL, a copy-link button; shows the frame; subscribes to events; when a `version` event arrives with `n > shownVersion`, shows a banner "v4 published" with a "Reload" button that navigates to `/a/<id>` (latest); on `artifact_deleted` replaces the frame with "This artifact was deleted". A 404 shows "Artifact not found". A deleted-while-pinned version is not special-cased.

- [ ] **Step 1: Write the failing unit tests**

`web/shell/src/origin.test.ts`:
```ts
import { describe, it, expect, vi } from "vitest";
import { artifactOrigin, probeOrigin, contentSrc } from "./origin";

const loc = (hostname: string, port = "7480") => ({ hostname, port, protocol: "http:" } as unknown as Location);

describe("artifactOrigin", () => {
  it("uses <id>.localhost on local hosts only", () => {
    expect(artifactOrigin("7q3k9mzx2b4t", loc("localhost"))).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", loc("127.0.0.1"))).toBe("http://7q3k9mzx2b4t.localhost:7480");
    expect(artifactOrigin("7q3k9mzx2b4t", loc("192.168.1.20"))).toBeNull();
    expect(artifactOrigin("7q3k9mzx2b4t", loc("mymac.local"))).toBeNull();
  });
});

describe("probeOrigin", () => {
  it("is true on 200, false on error or timeout", async () => {
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => new Response("{}", { status: 200 }))).toBe(true);
    sessionStorage.clear();
    expect(await probeOrigin("http://x.localhost:1", async () => { throw new TypeError("dns"); })).toBe(false);
    sessionStorage.clear();
    const never = (_: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_, rej) => init?.signal?.addEventListener("abort", () => rej(new DOMException("aborted", "AbortError"))));
    expect(await probeOrigin("http://x.localhost:1", never as typeof fetch, 20)).toBe(false);
  });
  it("caches the answer per session", async () => {
    sessionStorage.clear();
    const f = vi.fn(async () => new Response("{}"));
    await probeOrigin("http://x.localhost:1", f);
    await probeOrigin("http://x.localhost:1", f);
    expect(f).toHaveBeenCalledTimes(1);
  });
});

describe("contentSrc", () => {
  it("picks the origin or the same-origin path", () => {
    expect(contentSrc("7q3k9mzx2b4t", 2, "http://7q3k9mzx2b4t.localhost:7480")).toBe("http://7q3k9mzx2b4t.localhost:7480/v/2/");
    expect(contentSrc("7q3k9mzx2b4t", 2, null)).toBe("/c/7q3k9mzx2b4t/v/2/");
  });
});
```

`web/shell/src/events.test.ts`:
```ts
import { describe, it, expect, vi } from "vitest";
import { subscribe } from "./events";

class FakeES {
  static last: FakeES;
  listeners = new Map<string, (e: MessageEvent) => void>();
  closed = false;
  constructor(public url: string) { FakeES.last = this; }
  addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, fn); }
  close() { this.closed = true; }
  emit(t: string, data: unknown) { this.listeners.get(t)?.(new MessageEvent(t, { data: JSON.stringify(data) })); }
}

describe("subscribe", () => {
  it("opens a filtered stream, forwards parsed events, and closes on unsubscribe", () => {
    vi.stubGlobal("EventSource", FakeES);
    const seen: unknown[] = [];
    const off = subscribe("7q3k9mzx2b4t", e => seen.push(e));
    expect(FakeES.last.url).toBe("/api/events?artifact=7q3k9mzx2b4t");
    FakeES.last.emit("version", { type: "version", artifact_id: "7q3k9mzx2b4t", n: 4 });
    FakeES.last.emit("artifact_deleted", { type: "artifact_deleted", artifact_id: "7q3k9mzx2b4t" });
    expect(seen).toEqual([{ type: "version", artifact_id: "7q3k9mzx2b4t", n: 4 }, { type: "artifact_deleted", artifact_id: "7q3k9mzx2b4t" }]);
    off();
    expect(FakeES.last.closed).toBe(true);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd web && npm test`

- [ ] **Step 3: Implement origin.ts and events.ts**

`web/shell/src/origin.ts`:
```ts
const CACHE_KEY = "artifax.origin-ok";

export function artifactOrigin(id: string, loc: Location = location): string | null {
  if (loc.hostname !== "localhost" && loc.hostname !== "127.0.0.1") return null;
  const port = loc.port ? `:${loc.port}` : "";
  return `${loc.protocol}//${id}.localhost${port}`;
}

function readCache(): boolean | null {
  try { const v = sessionStorage.getItem(CACHE_KEY); return v === null ? null : v === "1"; } catch { return null; }
}
function writeCache(ok: boolean) { try { sessionStorage.setItem(CACHE_KEY, ok ? "1" : "0"); } catch { /* storage unavailable */ } }

/** Whether the browser resolves `<id>.localhost`; probed once per session. */
export async function probeOrigin(origin: string, fetchImpl: typeof fetch = fetch, timeoutMs = 1000): Promise<boolean> {
  const cached = readCache();
  if (cached !== null) return cached;
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  let ok = false;
  try { ok = (await fetchImpl(`${origin}/healthz`, { signal: ctl.signal, mode: "cors" })).ok; } catch { ok = false; }
  clearTimeout(timer);
  writeCache(ok);
  return ok;
}

export function contentSrc(id: string, n: number, origin: string | null): string {
  return origin ? `${origin}/v/${n}/` : `/c/${id}/v/${n}/`;
}
```

`web/shell/src/events.ts`:
```ts
export type ArtifactEvent = { type: "version"; artifact_id: string; n: number } | { type: "artifact_deleted"; artifact_id: string };

export function subscribe(artifactId: string, onEvent: (e: ArtifactEvent) => void): () => void {
  const es = new EventSource(`/api/events?artifact=${artifactId}`);
  const handler = (e: MessageEvent) => { try { onEvent(JSON.parse(e.data)); } catch { /* ignore malformed */ } };
  es.addEventListener("version", handler);
  es.addEventListener("artifact_deleted", handler);
  return () => es.close();
}
```

- [ ] **Step 4: Implement frame.tsx and artifact.tsx**

`web/shell/src/frame.tsx`:
```tsx
import { contentSrc } from "./origin";

export function Frame({ id, n, origin }: { id: string; n: number; origin: string | null }) {
  const src = contentSrc(id, n, origin);
  return origin
    ? <iframe key={`${n}-o`} class="frame" title="artifact content" src={src} allow="clipboard-write; fullscreen" />
    : <iframe key={`${n}-s`} class="frame" title="artifact content" src={src} sandbox="allow-scripts allow-forms allow-modals allow-popups allow-downloads" allow="clipboard-write; fullscreen" />;
}
```

`web/shell/src/artifact.tsx`:
```tsx
import { useEffect, useState } from "preact/hooks";
import { type Artifact, type Version, getArtifact } from "./api";
import { subscribe } from "./events";
import { Frame } from "./frame";
import { artifactOrigin, contentSrc, probeOrigin } from "./origin";

type Props = { id: string; pinnedVersion: number | null };

export default function ArtifactView({ id, pinnedVersion }: Props) {
  const [data, setData] = useState<{ artifact: Artifact; versions: Version[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [origin, setOrigin] = useState<string | null | undefined>(undefined);
  const [newer, setNewer] = useState<number | null>(null);
  const [deleted, setDeleted] = useState(false);

  useEffect(() => { getArtifact(id).then(setData, e => setError(String(e).includes("404") ? "Artifact not found" : String(e))); }, [id]);
  useEffect(() => {
    const o = artifactOrigin(id);
    if (!o) { setOrigin(null); return; }
    probeOrigin(o).then(ok => setOrigin(ok ? o : null));
  }, [id]);

  const shown = pinnedVersion ?? data?.artifact.current_version ?? 0;
  useEffect(() => subscribe(id, e => {
    if (e.type === "version" && e.n > shown) setNewer(e.n);
    if (e.type === "artifact_deleted") setDeleted(true);
  }), [id, shown]);

  if (error) return <Shell title="Artifax"><p class="empty">{error}</p></Shell>;
  if (!data || origin === undefined) return <Shell title="Artifax"><p class="empty muted">Loading…</p></Shell>;
  const { artifact, versions } = data;
  const latest = artifact.current_version;
  const raw = contentSrc(id, shown, origin);

  return (
    <Shell title={artifact.title} right={
      <>
        <select value={shown} onChange={e => { const n = Number((e.target as HTMLSelectElement).value); location.assign(n === latest ? `/a/${id}` : `/a/${id}/v/${n}`); }}>
          {versions.map(v => <option value={v.n} key={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>)}
        </select>
        <a class="hide-sm" href={raw} target="_blank" rel="noopener">open raw</a>
        <button onClick={() => navigator.clipboard?.writeText(location.origin + `/a/${id}`)}>copy link</button>
      </>
    }>
      <div class="viewer">
        {deleted ? <p class="empty">This artifact was deleted.</p> : <Frame id={id} n={shown} origin={origin} />}
        {newer && !deleted && (
          <div class="banner"><span>v{newer} published</span><button class="primary" onClick={() => location.assign(`/a/${id}`)}>Reload</button></div>
        )}
        {shown < latest && !newer && <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={`/a/${id}`}>latest</a></div>}
      </div>
    </Shell>
  );
}

function Shell({ title, right, children }: { title: string; right?: preact.ComponentChildren; children: preact.ComponentChildren }) {
  return (
    <>
      <header class="topbar">
        <a href="/" title="Gallery">←</a>
        <h1>{title}</h1>
        <span style="flex:1" />
        {right}
      </header>
      {children}
    </>
  );
}
```

- [ ] **Step 5: Run tests, build, verify in the browser, commit**

Run: `cd web && npm run typecheck && npm test && npm run build`, then with the daemon running in the foreground: publish a page, open `/a/<id>`, confirm the content renders (devtools: the iframe `src` is `http://<id>.localhost:7480/v/1/` in Chrome; if the probe failed the `src` is `/c/<id>/v/1/` with a `sandbox` attribute), republish from another terminal, confirm the "v2 published" banner appears within a second and Reload shows v2, pick v1 in the select and confirm the pinned banner. Check dark mode and 375 px width.

```bash
git add -A
git commit -m "Add the artifact viewer with origin probe, version picker, and live banner"
```

---

### Task 19: Browser end-to-end tests

**Files:**
- Create: `web/playwright.config.ts`, `web/e2e/fixtures.ts`, `web/e2e/viewer.spec.ts`

**Interfaces:**
- `fixtures.ts` exports `startDaemon(): Promise<{ base: string; token: string; stop(): Promise<void> }>` that runs `cargo run -q -p artifax-cli -- serve --foreground --port 0` with a temp `ARTIFAX_HOME` and waits for `daemon.json`, and `publish(base, token, title, files, ifVersion?, id?)`.
- Tests run against the built UI (`just web` first) in Chromium.

- [ ] **Step 1: Write config and fixtures**

`web/playwright.config.ts`:
```ts
import { defineConfig } from "@playwright/test";
export default defineConfig({ testDir: "e2e", timeout: 60_000, use: { browserName: "chromium" }, workers: 1, reporter: "list" });
```

`web/e2e/fixtures.ts`:
```ts
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export async function startDaemon() {
  const home = mkdtempSync(join(tmpdir(), "artifax-e2e-"));
  const child: ChildProcess = spawn("cargo", ["run", "-q", "-p", "artifax-cli", "--", "serve", "--foreground", "--port", "0"],
    { cwd: join(__dirname, "..", ".."), env: { ...process.env, ARTIFAX_HOME: home }, stdio: ["ignore", "inherit", "inherit"] });
  const infoPath = join(home, "daemon.json");
  const deadline = Date.now() + 120_000;
  while (!existsSync(infoPath)) { if (Date.now() > deadline) throw new Error("daemon did not start"); await new Promise(r => setTimeout(r, 200)); }
  const info = JSON.parse(readFileSync(infoPath, "utf8"));
  const base = `http://localhost:${info.port}`;
  for (;;) { try { if ((await fetch(`${base}/healthz`)).ok) break; } catch { /* retry */ } await new Promise(r => setTimeout(r, 100)); }
  return {
    base, token: info.token as string,
    async stop() { await fetch(`${base}/api/admin/shutdown`, { method: "POST", headers: { authorization: `Bearer ${info.token}` } }).catch(() => {}); child.kill(); rmSync(home, { recursive: true, force: true }); },
  };
}

export async function publish(base: string, token: string, title: string, files: Record<string, string>, ifVersion?: number, id?: string) {
  const body = { title, if_version: ifVersion, files: Object.fromEntries(Object.entries(files).map(([k, v]) => [k, { content: v, encoding: "utf8" }])) };
  const url = id ? `${base}/api/artifacts/${id}/versions` : `${base}/api/artifacts`;
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}
```

- [ ] **Step 2: Write the spec**

`web/e2e/viewer.spec.ts`:
```ts
import { test, expect } from "@playwright/test";
import { startDaemon, publish } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { d = await startDaemon(); });
test.afterAll(async () => { await d.stop(); });

test("gallery shows an empty state then a card", async ({ page }) => {
  await page.goto(`${d.base}/`);
  await expect(page.getByText("No artifacts yet")).toBeVisible();
  await publish(d.base, d.token, "Hello Report", { "index.html": "<title>Hello</title><h1>Hi</h1>" });
  await page.reload();
  await expect(page.locator("a.card")).toHaveCount(1);
  await expect(page.locator("a.card")).toContainText("Hello Report");
});

test("viewer renders content with the bridge, and shows a banner on republish", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Live", { "index.html": "<h1 id=h>v1</h1><script>document.title = typeof window.claude.use</script>" });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const frame = page.frameLocator("iframe.frame");
  await expect(frame.locator("#h")).toHaveText("v1");
  await expect.poll(async () => await page.frames()[1]?.title()).toBe("function");
  const src = await page.locator("iframe.frame").getAttribute("src");
  expect(src).toMatch(new RegExp(`(${artifact.id}\\.localhost:\\d+/v/1/|/c/${artifact.id}/v/1/)$`));
  await publish(d.base, d.token, "Live", { "index.html": "<h1 id=h>v2</h1>" }, 1, artifact.id);
  await expect(page.getByText("v2 published")).toBeVisible({ timeout: 5000 });
  await page.getByRole("button", { name: "Reload" }).click();
  await expect(page.frameLocator("iframe.frame").locator("#h")).toHaveText("v2");
  await page.selectOption("select", "1");
  await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}/v/1$`));
  await expect(page.frameLocator("iframe.frame").locator("#h")).toHaveText("v1");
});

test("LAN-style host falls back to the sandboxed frame", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Lan", { "index.html": "<p id=p>lan</p>" });
  const lanBase = d.base.replace("localhost", "127.0.0.1").replace("127.0.0.1", "0.0.0.0");
  await page.goto(`${lanBase}/a/${artifact.id}`).catch(() => {});
  if (page.url().startsWith(lanBase)) {
    await expect(page.locator("iframe.frame")).toHaveAttribute("sandbox", /allow-scripts/);
    await expect(page.frameLocator("iframe.frame").locator("#p")).toHaveText("lan");
  }
});
```
The third test only asserts when the browser can reach the daemon via `0.0.0.0` (it can on macOS and Linux Chromium; if not, the test is a no-op rather than a false failure).

- [ ] **Step 3: Run**

Run: `just web-e2e`
Expected: 3 passed. If the first test fails on the empty state because another test published first, `workers: 1` keeps order; the spec relies on the gallery test running first in file order.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "Add browser end-to-end tests for gallery and viewer"
```

---

### Task 20: Quality gates, CI, README

**Files:**
- Create: `scripts/quality_gates.sh`, `.github/workflows/ci.yml`, `README.md`

- [ ] **Step 1: Write the gate script**

```bash
#!/usr/bin/env bash
# Runs every check CI runs. Pass --verbose to stream each gate's output.
set -euo pipefail
cd "$(dirname "$0")/.."
VERBOSE="${1:-}"
run() {
    local name="$1"; shift
    printf '%-28s' "$name"
    if [ "$VERBOSE" = "--verbose" ]; then echo; "$@"; else
        if out="$("$@" 2>&1)"; then echo ok; else echo FAIL; echo "$out"; exit 1; fi
    fi
}
run "cargo fmt --check"     cargo fmt --all -- --check
run "cargo clippy"          cargo clippy --workspace --all-targets -- -D warnings
run "cargo test"            cargo test --workspace
run "web typecheck + unit"  bash -c 'cd web && npm ci --silent && npm run typecheck && npm test -- --reporter=dot'
run "web build"             bash -c 'cd web && npm run build'
run "web e2e"               bash -c 'cd web && npx playwright install --with-deps chromium >/dev/null && npm run e2e'
echo "all gates passed"
```

- [ ] **Step 2: Write CI**

`.github/workflows/ci.yml`:
```yaml
name: ci
on: { push: { branches: [main] }, pull_request: {} }
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.94.0, components: "rustfmt, clippy" }
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with: { node-version: 22, cache: npm, cache-dependency-path: web/package-lock.json }
      - run: scripts/quality_gates.sh
```

- [ ] **Step 3: Write the README**

Cover: what Artifax is (two sentences), install from source (`cargo install --path crates/artifax-cli` after `just web`), the four commands a person needs (`artifax publish index.html --dir site`, `artifax open <id>`, `artifax list`, `artifax serve --bind 0.0.0.0` for LAN), where data lives (`~/.artifax`), and a link to the spec. State that comments, agent plugins, and runtime capabilities arrive in later phases.

- [ ] **Step 4: Run the gates and commit**

Run: `chmod +x scripts/quality_gates.sh && scripts/quality_gates.sh`
Expected: `all gates passed`.

```bash
git add -A
git commit -m "Add quality gates, CI workflow, and README"
```

---

## Self-review notes

- Spec coverage for phase 1 (§17): daemon + discovery + auto-start + stop (Tasks 12, 14); storage tables `artifacts`, `versions`, `assets` (Tasks 2–4); REST for artifacts/versions/files/assets, `/api/token`, `/healthz`, SSE `version` event (Tasks 7–11); wrapping with the recognition rule (Task 5, 10); bridge with `use()` → `null` (Task 16); D5 probe and both frame modes (Task 18, verified in Task 19); gallery and shell per §8 phase 1 paragraph (Tasks 17–18); CLI commands (Tasks 14–15); `owner_session_id` null and the gallery's command-line label (Tasks 2, 17). The `PATCH /api/artifacts/<aid>` `capabilities` field from §6 is accepted only through publish in phase 1; the PATCH route takes title, description, icon, pinned. Add `capabilities` to `PatchBody` in phase 4 when it means something.
- Review Focus 1 → Task 3 `rejects_unsafe_paths`, Task 8 `validation_errors_are_400_with_codes`. 2 → Task 10 host tests. 3 → Task 3 `stale_if_version_conflicts_and_writes_nothing`, Task 8. 4 → Task 14 `concurrent_auto_starts_yield_one_daemon`. 5 → Task 5 tests. 6 → Task 3 `decodes_base64_and_rejects_bad_base64`.
- Names used across tasks: `Store::{open, now, home, get_artifact, list_artifacts, update_meta, set_pinned, delete_artifact, create_artifact, publish_version, get_version, list_versions, file_path, add_asset, get_asset, list_assets, delete_asset, integrity_check, insert_artifact_for_test}`; `publish::{validate, PublishRequest, FileInput, Encoding, ValidatedPublish, FileChange, DecodedFile, content_type_for, check_path, INDEX, MAX_FILE_BYTES, MAX_BODY_BYTES}`; `wrap::{wrap_document, bridge_tag, is_full_document, RESET_CSS}`; `Event::{Version, ArtifactDeleted}`, `EventBus::{new, publish, subscribe}`; server `AppState`, `build_router`, `build_router_with_shutdown`, `ApiError`, `RequireToken`, `daemon::{DaemonInfo, DaemonLock, ServeConfig, serve, read_daemon_info, write_daemon_info, remove_daemon_info, pid_alive, generate_token, DEFAULT_PORT}`; CLI `Client::{connect, discover, get, post, patch, delete, shutdown, browser_url, http_status}`. Task 14's `Client::connect(home, port)` gains a `bind` parameter in the same task's serve command; keep the two-argument form for other callers by defaulting `bind` to loopback in a `connect_with_bind` and having `connect` call it.
