# Clax in Chrome Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Manifest V3 Chrome extension that puts Clax's comment overlay on any web page, stores each comment's screenshot and a sanitized snapshot as a version of a new "live page" artifact, and links those threads to agents that `watch` the page's URL.

**Architecture:** The daemon gains a `live` artifact kind keyed by origin and path, routes for live pages, scope watches, pending addresses, extension credentials and an extension gateway that admits the extension's origin only with a credential and hands its requests to the existing viewer routes. `clax native-host` pairs the extension with the daemon over Chrome native messaging; `clax extension install` (run by `clax init`) writes the unpacked extension and the host manifests. The extension (service worker, a tiny loader, a lazily injected overlay reusing the bridge's comment mode and anchoring, an extension-frame composer, and a Svelte side panel reusing the shell's sidebar) holds the only credential in its service worker.

**Tech Stack:** Rust 2024 (axum, tokio, rusqlite, `url`, `sha2`, `base64`, rust-embed, clap), Svelte 5 (runes) + TypeScript, Vite 6, Vitest + jsdom, Playwright (Chromium persistent context with `--load-extension`), Chrome Extensions MV3 (`sidePanel`, `scripting`, `nativeMessaging`, `activeTab`).

**Spec:** `docs/superpowers/specs/2026-10-05-chrome-overlay-design.md` (and the main spec `docs/superpowers/specs/2026-09-28-clax-design.md`). Read both before Task 1; every task's requirements include the spec section it names.

**Before Task 1:** execution starts only after the `cli-comments` branch (the owner identity, spec §2.1) is merged to main. Rebase this branch on main first, then read what that merge added: the owner viewer, how the daemon resolves it, and the hook that maps a credential kind to it. Tasks 5, 6, 10, 14 and 16 consume it (see "Owner identity" below); where this plan writes a placeholder name for it, use main's.

## Global Constraints

- No `unsafe` Rust anywhere: every crate keeps `#![forbid(unsafe_code)]`, and the `no unsafe code` gate in `scripts/quality_gates.sh` stays green.
- UI is Svelte 5 in runes mode; no other UI framework and no CSS framework.
- The whole `scripts/quality_gates.sh` stays within about 2 minutes on a warm cache: tests run in parallel, use fake or injected clocks, and never sleep a fixed time; browser tests wait on events. A new slow test is a defect.
- Extension: Manifest V3, `minimum_chrome_version` `"116"`. The release manifest declares no `host_permissions` and no static `content_scripts`; it declares `"optional_host_permissions": ["http://*/*", "https://*/*"]`.
- Native host name: `dev.empathic.clax`. Extension ID: **the ID in effect** for the Clax home (spec L15): derived from `web/extension/key/key.pub.b64` when that file is committed, else from the canonicalized absolute path `<home>/extension` (Chromium's unpacked-extension rule: the first 128 bits of SHA-256, each nibble written `a`–`p`; for a path, the SHA-256 is of the path bytes). It is computed once per home and carried as `AppState.extension_id`; there is no compile-time `EXTENSION_ID` constant. Wherever a code block in Tasks 5–16 writes `EXTENSION_ID`, read it as the ID in effect: `state.extension_id` in server code, `ts.extension_id()` in server tests, `clax_core::extension::extension_id_in_effect(home)` in the CLI and the native host, `chrome.runtime.id` in extension code, and a fixed test ID in pure unit tests.
- Keys and signing (spec L15, §6.7): no key is needed to build, install, test or use the extension, and nothing in Tasks 1–16 waits on the owner. The manifest carries `key` only when `web/extension/key/key.pub.b64` is committed (Task 8's build adds it). Never ask for, read, store or generate a production private key or its path; it lives only in the owner's 1Password, referenced by `CLAX_EXTENSION_KEY_REF`, and is read only by Task 17's owner-run scripts. Tests that need a key generate a throwaway one in the test.
- Owner identity (spec L6, §2.1): the extension acts as the owner identity, never as a viewer of its own. There is no extension viewer, no `extension_viewers` table, and no viewer ID on a credential. Consumes, from the owner identity on main (placeholder names; use main's): **`owner_viewer(&Store) -> Result<Viewer>`** (the owner's `viewers` row, created on first use if that work does so), **the owner hook** (the one place that maps an authenticated credential kind to the owner identity, for example a `Principal::Owner` resolved by one extractor or middleware), and test access to the owner viewer (**`TestServer::owner_viewer() -> Viewer`**; Task 5 adds it over `owner_viewer` if main has none). If main also changed what `TestServer::viewer` returns (for example, because every loopback browser is now the owner), keep Tasks 1–4's tests meaning what they say: two named people stay two distinct viewers.
- Credential format: `cxe_` followed by 43 base64url characters (32 random bytes). Stored only as SHA-256 (lowercase hex). Never logged, never in a URL, never sent to a content script, composer or page.
- The daemon token never reaches the extension, a content script or a web page.
- Live-page key: origin (scheme, lowercased host, non-default port) + path; `route` = query without `utm_*`, `fbclid`, `gclid`, then a `#/…` or `#!/…` hash route; route at most 512 bytes.
- Snapshot caps: 100,000 elements, 8 MiB of HTML, 1.5 s of serialization. Clip: PNG, at most 1600 px on the long side, at most 5 MiB.
- Bundle budgets (gzip): `loader.js` 2 KiB, `overlay.js` 32 KiB, `sw.js` 24 KiB, `sidepanel.js` 64 KiB, `composer.js` 28 KiB.
- Page content, page URLs and titles, quotes and comment text are untrusted: Svelte renders them as text; payload lines use `quoted`/`one_line` (`crates/clax-core/src/feedback.rs`).
- The shell's viewer routes keep their `Origin`/`Sec-Fetch-Site` rules; the extension's origin is admitted only by the gateway with a live credential, and a bearer token from that origin is refused.
- Live pages are invisible to non-loopback callers without the token.
- Migrations: 15 is live pages, 16 is extension credentials. Never edit an earlier migration.
- Doc comments and commit messages describe the contract or the change, never a conversation; in prose write "ID", never "id" (except as a literal symbol).
- Commit with `git -c commit.gpgsign=false commit …`; never `--no-gpg-sign`; never push or merge.

## Review Focus

1. **Hot reload replaces the DOM wholesale** (Vite HMR re-rendering `#app`): pins must re-resolve within one animation frame after 150 ms of quiet, a changed element keeps its pin, and a removed one goes to Detached; never a pin on a wrong element. Pinned in Task 12 (resolver test "re-resolves after a wholesale replacement and detaches what is gone") and Task 16 (e2e HMR edit).
2. **Query strings, cache-busters and hash routes**: `?t=123&utm_source=x` and `#/users/7` must land on one live page per path, with a route per thread, and a pin shows only on its own route. Pinned in Task 1 (`live-url-cases.json`) and Task 12 (resolver test "shows only the current route's threads").
3. **Hostile page content in a snapshot**: inline handlers, `javascript:` links, a password field with a `value` attribute, hidden CSRF inputs, `srcdoc` frames, `<meta http-equiv="refresh">`, `<base href>`. All must be gone from the snapshot, and a served snapshot must not run any script even if one slipped through. Pinned in Task 11 (serializer tests) and Task 2 (snapshot policy header test).
4. **Daemon restarted on another port, or credential revoked, while the side panel is open**: the worker re-pairs once and retries the request once; the panel recovers without a reload. Pinned in Task 10 (`api.test.ts` "re-pairs once on 401 and on a network error").
5. **Agent watches `http://localhost:5173/` before any page exists, then the person comments on `/settings`**: the agent receives it. Pinned in Task 3 (`scope_watch_covers_pages_created_later`).

---

## File structure

```
crates/clax-core/src/live.rs                    page URL → key + route; placeholder page
crates/clax-core/src/live-url-cases.json        normalization cases (Rust test)
crates/clax-core/src/extension.rs               EXTENSION_ID, ID-from-key, credential format
crates/clax-core/src/store/live.rs              live pages, snapshots, scope watches, pending addresses
crates/clax-core/src/store/extension.rs         extension credentials
crates/clax-server/src/live.rs                  LiveIds cache; hiding live pages from the LAN
crates/clax-server/src/routes/live.rs           /api/live/pages, /threads, /snapshots
crates/clax-server/src/extension.rs             credential cache; the extension gateway middleware
crates/clax-server/src/routes/extension.rs      /api/extension (token routes)
crates/clax-mcp/src/target.rs                   url_or_id → artifact or page
crates/clax-cli/src/commands/native_host.rs     `clax native-host`
crates/clax-cli/src/commands/extension.rs       `clax extension install|uninstall|status`
crates/clax-cli/src/extension_files.rs          the embedded extension; host manifest directories
web/extension/manifest.json                     MV3 manifest with the fixed key
web/extension/src/messages.ts                   every message type and its validator
web/extension/src/sw/{main,pairing,api,origins,hub,tabs,picks,capture}.ts
web/extension/src/content/{loader,overlay,pins,resolver,snapshot}.ts
web/extension/src/composer/{composer.html,main.ts,ComposerFrame.svelte}
web/extension/src/panel/{sidepanel.html,main.ts,Panel.svelte,adapt.ts}
web/extension/test/fake-chrome.ts               a fake `chrome` for unit tests
web/scripts/build-extension.mjs                 builds dist-extension and dist-extension-test
web/scripts/extension-icons.mjs                 draws the Echo mark PNGs
web/e2e/chrome-overlay.spec.ts                  real extension, native host, daemon, Vite dev server
web/e2e/live-site/                              the fixture app the Vite dev server serves
```

---

### Task 1: Live-page identity and storage

**Files:**
- Create: `crates/clax-core/src/live.rs`
- Create: `crates/clax-core/src/live-url-cases.json`
- Create: `crates/clax-core/src/store/live.rs`
- Modify: `crates/clax-core/Cargo.toml` (add `url.workspace = true`; add `url = "2"` to `[workspace.dependencies]` in the root `Cargo.toml` if absent)
- Modify: `crates/clax-core/src/lib.rs` (`pub mod live;`)
- Modify: `crates/clax-core/src/store/mod.rs` (`pub mod live;`)
- Modify: `crates/clax-core/src/store/migrations.rs` (append migration 15; add a migration test)
- Modify: `crates/clax-core/src/model.rs` (`Artifact.kind`)
- Modify: `crates/clax-core/src/store/artifacts.rs` (`SELECT` and `row_to_artifact` read `kind`; `write_version` becomes `pub(super)`; `delete_artifact` and `delete_zero_version_artifacts` delete the `live_pages` row)
- Modify: `crates/clax-core/src/anchor.rs` (`route`)
- Modify: `crates/clax-core/src/store/threads.rs` (`create_thread` refuses `route` on a non-live artifact)
- Modify: `web/bridge/src/protocol.ts` (`Anchor.route?: string`)

**Interfaces:**
- Consumes: `Store::write_version` (artifacts.rs), `publish::validate`, `ArtifactId::generate`.
- Produces:
  - `clax_core::live::{KIND_HTML, KIND_LIVE, MAX_ROUTE, MAX_URL, PageKey {origin: String, path: String}, PageUrl {key: PageKey, route: Option<String>}, parse_page_url(&str) -> Result<PageUrl>, placeholder_html(&PageKey) -> String}`
  - `PageKey::page_url(&self) -> String`, `PageKey::covered_by(&self, scope: &PageKey) -> bool`
  - `clax_core::store::live::{LivePage {artifact_id, origin, path}, EnsuredPage {artifact, version, created, new_version, scoped_sessions: Vec<String>}}`
  - `Store::find_live_page(&PageKey) -> Result<Option<LivePage>>`, `Store::live_page_of(&ArtifactId) -> Result<Option<LivePage>>`, `Store::ensure_live_page(&PageKey, title: &str, snapshot: Option<&[u8]>) -> Result<EnsuredPage>`, `Store::store_snapshot(&ArtifactId, title: &str, html: &[u8], force: bool) -> Result<(Version, bool)>`
  - `Artifact.kind: String` (`"html"` or `"live"`), serialized as `kind`.
  - `Anchor.route: Option<String>`, serialized only when present.

- [ ] **Step 1: Write the failing normalization test and its cases**

`crates/clax-core/src/live-url-cases.json`:

```json
[
  {"url": "http://LOCALHOST:5173/settings?tab=billing&utm_source=x#top", "origin": "http://localhost:5173", "path": "/settings", "route": "?tab=billing"},
  {"url": "http://localhost:3000/#/users/7", "origin": "http://localhost:3000", "path": "/", "route": "#/users/7"},
  {"url": "http://localhost:3000", "origin": "http://localhost:3000", "path": "/", "route": null},
  {"url": "https://example.com:443/a/./b/../c", "origin": "https://example.com", "path": "/a/c", "route": null},
  {"url": "http://user:pw@127.0.0.1:8080/x/?q=1#!/r", "origin": "http://127.0.0.1:8080", "path": "/x/", "route": "?q=1#!/r"},
  {"url": "http://[::1]:5173/", "origin": "http://[::1]:5173", "path": "/", "route": null},
  {"url": "http://localhost:5173/?fbclid=1&gclid=2&utm_medium=m", "origin": "http://localhost:5173", "path": "/", "route": null},
  {"url": "http://localhost:5173/docs?t=1700000000", "origin": "http://localhost:5173", "path": "/docs", "route": "?t=1700000000"},
  {"url": "http://bücher.example/", "origin": "http://xn--bcher-kva.example", "path": "/", "route": null},
  {"url": "  http://localhost:5173/p  ", "origin": "http://localhost:5173", "path": "/p", "route": null},
  {"url": "file:///etc/passwd", "error": "unsupported_url"},
  {"url": "chrome://extensions", "error": "unsupported_url"},
  {"url": "javascript:alert(1)", "error": "unsupported_url"},
  {"url": "not a url", "error": "invalid_url"}
]
```

At the bottom of `crates/clax-core/src/live.rs` (create the file with only this test module and `use super::*;` above it for now):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        url: String,
        origin: Option<String>,
        path: Option<String>,
        route: Option<String>,
        error: Option<String>,
    }

    #[test]
    fn page_urls_normalize_as_the_cases_say() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("live-url-cases.json")).unwrap();
        for c in cases {
            match (parse_page_url(&c.url), c.error) {
                (Ok(p), None) => {
                    assert_eq!(Some(p.key.origin), c.origin, "{}", c.url);
                    assert_eq!(Some(p.key.path), c.path, "{}", c.url);
                    assert_eq!(p.route, c.route, "{}", c.url);
                }
                (Err(crate::CoreError::Invalid { code, .. }), Some(want)) => {
                    assert_eq!(code, want, "{}", c.url)
                }
                (got, want) => panic!("{}: got {got:?}, want error {want:?}", c.url),
            }
        }
    }

    #[test]
    fn routes_are_cut_at_a_character_boundary() {
        let long = format!("http://localhost/?q={}", "é".repeat(400));
        let r = parse_page_url(&long).unwrap().route.unwrap();
        assert!(r.len() <= MAX_ROUTE);
        assert!(r.is_char_boundary(r.len()));
    }

    #[test]
    fn urls_over_the_limit_are_refused() {
        let long = format!("http://localhost/{}", "a".repeat(MAX_URL));
        assert!(matches!(parse_page_url(&long), Err(crate::CoreError::Invalid { code, .. }) if code == "invalid_url"));
    }

    #[test]
    fn a_scope_covers_its_path_and_below_on_the_same_origin() {
        let k = |o: &str, p: &str| PageKey { origin: o.into(), path: p.into() };
        let root = k("http://localhost:5173", "/");
        assert!(k("http://localhost:5173", "/settings").covered_by(&root));
        assert!(k("http://localhost:5173", "/docs/a").covered_by(&k("http://localhost:5173", "/docs")));
        assert!(k("http://localhost:5173", "/docs/a").covered_by(&k("http://localhost:5173", "/docs/")));
        assert!(k("http://localhost:5173", "/docs").covered_by(&k("http://localhost:5173", "/docs")));
        assert!(!k("http://localhost:5173", "/docsx").covered_by(&k("http://localhost:5173", "/docs")));
        assert!(!k("http://localhost:3000", "/").covered_by(&root));
    }

    #[test]
    fn the_placeholder_escapes_the_url() {
        let html = placeholder_html(&PageKey { origin: "http://localhost:1".into(), path: "/<b>\"".into() });
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("/&lt;b&gt;&quot;"));
        assert!(!html.contains("<b>\""));
    }
}
```

Check how `CoreError::Invalid` is shaped (`grep -n "Invalid" crates/clax-core/src/error.rs`) and match its field names in the two `matches!`/`match` arms.

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p clax-core --lib live::tests`
Expected: FAIL to compile: `cannot find function parse_page_url`.

- [ ] **Step 3: Implement `live.rs`**

Above the test module in `crates/clax-core/src/live.rs`:

```rust
//! Live pages (spec 2026-10-05-chrome-overlay-design §7): the key a page
//! URL names (origin and path) and the route a thread on it was made at
//! (the query, less tracking parameters, and a hash route).

use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// `artifacts.kind` of a published HTML artifact.
pub const KIND_HTML: &str = "html";
/// `artifacts.kind` of a live page.
pub const KIND_LIVE: &str = "live";
/// The longest page URL accepted, in bytes.
pub const MAX_URL: usize = 4096;
/// The longest route kept, in bytes; a longer one is cut at a character boundary.
pub const MAX_ROUTE: usize = 512;
/// Query parameters that never form part of a route, besides every `utm_*`.
const TRACKING: &[&str] = &["fbclid", "gclid"];

/// A live page's identity: `origin` is the scheme, the lowercased host and
/// the port when it is not the scheme's default; `path` is the parsed path
/// (dot segments resolved, a trailing slash kept).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageKey {
    pub origin: String,
    pub path: String,
}

/// A page URL split into its key and its route (`None` when empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageUrl {
    pub key: PageKey,
    pub route: Option<String>,
}

impl PageKey {
    /// The page's URL without a route: `origin` followed by `path`.
    pub fn page_url(&self) -> String {
        format!("{}{}", self.origin, self.path)
    }

    /// Whether a scope watch on `scope` covers this page: the same origin,
    /// and a path equal to the scope's or below it (`/` covers every path).
    pub fn covered_by(&self, scope: &PageKey) -> bool {
        if self.origin != scope.origin {
            return false;
        }
        if scope.path == "/" || self.path == scope.path {
            return true;
        }
        let base = scope.path.trim_end_matches('/');
        self.path.starts_with(&format!("{base}/"))
    }
}

/// `s` cut to at most `max` bytes at a character boundary.
fn cut(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Splits a page URL into its key and route.
///
/// # Errors
/// `invalid_url` for text that is not a URL, names no host or is longer than
/// [`MAX_URL`]; `unsupported_url` for any scheme but `http` and `https`.
pub fn parse_page_url(raw: &str) -> Result<PageUrl> {
    let raw = raw.trim();
    if raw.len() > MAX_URL {
        return Err(CoreError::invalid("invalid_url", format!("a page URL is at most {MAX_URL} bytes")));
    }
    let u = url::Url::parse(raw).map_err(|e| CoreError::invalid("invalid_url", format!("not a URL: {e}")))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(CoreError::invalid("unsupported_url", "only http and https pages can be live pages"));
    }
    let host = u.host_str().ok_or_else(|| CoreError::invalid("invalid_url", "the URL names no host"))?;
    let origin = match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    };
    let path = if u.path().is_empty() { "/".to_string() } else { u.path().to_string() };
    let mut route = String::new();
    if let Some(q) = u.query() {
        let kept: Vec<&str> = q
            .split('&')
            .filter(|kv| {
                let k = kv.split('=').next().unwrap_or("");
                !kv.is_empty() && !k.starts_with("utm_") && !TRACKING.contains(&k)
            })
            .collect();
        if !kept.is_empty() {
            route.push('?');
            route.push_str(&kept.join("&"));
        }
    }
    if let Some(f) = u.fragment()
        && (f.starts_with('/') || f.starts_with("!/"))
    {
        route.push('#');
        route.push_str(f);
    }
    let route = cut(route, MAX_ROUTE);
    Ok(PageUrl {
        key: PageKey { origin, path },
        route: (!route.is_empty()).then_some(route),
    })
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Version 1 of a live page created before any snapshot (spec L3): a page
/// saying no snapshot exists yet, following the page contract.
pub fn placeholder_html(key: &PageKey) -> String {
    let u = escape_html(&key.page_url());
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{u}</title><style>:root{{color-scheme:light dark;--bg:#f7f7f5;--ink:#1c1b19;--muted:#6b6862}}@media (prefers-color-scheme:dark){{:root{{--bg:#141413;--ink:#ecebe7;--muted:#a8a59e}}}}body{{margin:0;padding:48px 16px;background:var(--bg);color:var(--ink);font:16px/1.5 ui-sans-serif,-apple-system,system-ui,sans-serif}}main{{max-width:40rem;margin:0 auto}}p{{color:var(--muted)}}a{{color:inherit}}</style></head><body><main><h1>No snapshot yet</h1><p>Open <a href=\"{u}\">{u}</a> in Chrome and comment on it with the Clax extension. Each comment saves a snapshot of the page here.</p></main></body></html>"
    )
}
```

Add `pub mod live;` to `crates/clax-core/src/lib.rs` and `url.workspace = true` to `crates/clax-core/Cargo.toml` (`url = "2"` under `[workspace.dependencies]` in the root `Cargo.toml`; `Cargo.lock` already carries it through `reqwest`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p clax-core --lib live::tests`
Expected: PASS (5 tests).

- [ ] **Step 5: Write the failing store tests**

`crates/clax-core/src/store/live.rs` (test module only for now):

```rust
#[cfg(test)]
mod tests {
    use crate::live::{KIND_LIVE, PageKey};
    use crate::{ArtifactId, Home, Store};

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }
    fn key(path: &str) -> PageKey {
        PageKey { origin: "http://localhost:5173".into(), path: path.into() }
    }
    fn index(st: &Store, id: &ArtifactId, n: u32) -> String {
        std::fs::read_to_string(st.home().version_dir(id, n).join("index.html")).unwrap()
    }

    #[test]
    fn a_page_created_without_a_snapshot_gets_a_placeholder_version() {
        let (_d, st) = store();
        let e = st.ensure_live_page(&key("/"), "localhost:5173/", None).unwrap();
        assert!(e.created && e.new_version);
        assert_eq!(e.artifact.kind, KIND_LIVE);
        assert_eq!(e.version.n, 1);
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        assert!(index(&st, &id, 1).contains("No snapshot yet"));
        assert_eq!(st.find_live_page(&key("/")).unwrap().unwrap().artifact_id, e.artifact.id);
        assert!(st.list_artifacts().unwrap().iter().any(|a| a.id == e.artifact.id));
    }

    #[test]
    fn a_first_comment_makes_its_snapshot_version_one_and_identical_snapshots_reuse_it() {
        let (_d, st) = store();
        let e = st.ensure_live_page(&key("/s"), "Settings", Some(b"<!doctype html><p>a")).unwrap();
        assert_eq!((e.version.n, e.created, e.new_version), (1, true, true));
        let again = st.ensure_live_page(&key("/s"), "Settings", Some(b"<!doctype html><p>a")).unwrap();
        assert_eq!((again.version.n, again.created, again.new_version), (1, false, false));
        let changed = st.ensure_live_page(&key("/s"), "Settings", Some(b"<!doctype html><p>b")).unwrap();
        assert_eq!((changed.version.n, changed.new_version), (2, true));
        let id = ArtifactId::parse(&e.artifact.id).unwrap();
        let (v, new) = st.store_snapshot(&id, "Settings", b"<!doctype html><p>b", true).unwrap();
        assert_eq!((v.n, new), (3, true), "force makes a version even when identical");
        assert_eq!(index(&st, &id, 3), "<!doctype html><p>b");
    }

    #[test]
    fn deleting_a_live_page_frees_its_key() {
        let (_d, st) = store();
        let e = st.ensure_live_page(&key("/"), "x", None).unwrap();
        st.delete_artifact(&ArtifactId::parse(&e.artifact.id).unwrap()).unwrap();
        assert!(st.find_live_page(&key("/")).unwrap().is_none());
        let again = st.ensure_live_page(&key("/"), "x", None).unwrap();
        assert_ne!(again.artifact.id, e.artifact.id);
    }

    #[test]
    fn snapshots_are_refused_on_html_artifacts() {
        let (_d, st) = store();
        let id = st.insert_artifact_for_test("T", "2026-10-05T00:00:00.000Z");
        let err = st.store_snapshot(&id, "T", b"<p>", false).unwrap_err();
        assert!(matches!(err, crate::CoreError::Invalid { code, .. } if code == "not_live"));
    }

    #[test]
    fn a_route_is_refused_on_threads_of_html_artifacts() {
        let (_d, st) = store();
        let id = st.insert_artifact_for_test("T", "2026-10-05T00:00:00.000Z");
        let mut anchor: crate::Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "body", "file": "index.html"
        })).unwrap();
        anchor.route = Some("?a=1".into());
        let err = st.create_thread(&id, crate::store::threads::NewThread {
            author_public_id: None, version_n: 1, anchor, author_name: "A".into(),
            body: "x".into(), clip: None, via_page: false,
        }).unwrap_err();
        assert!(matches!(err, crate::CoreError::Invalid { code, .. } if code == "invalid_anchor"));
    }
}
```

`insert_artifact_for_test` exists in `store/artifacts.rs`; check it writes a version 1 (if it writes none, publish one through `create_artifact` with a one-file `ValidatedPublish` instead).

- [ ] **Step 6: Run them to make sure they fail**

Run: `cargo test -p clax-core --lib store::live::tests`
Expected: FAIL to compile: no method `ensure_live_page`, no field `kind`, no field `route`.

- [ ] **Step 7: Add migration 15**

Append to `MIGRATIONS` in `crates/clax-core/src/store/migrations.rs`:

```rust
    // 15: live pages (spec 2026-10-05-chrome-overlay-design §5.1): the
    // artifact kind, each live page's key, scope watches (and the watches
    // they made), and addresses waiting for a live page's next snapshot.
    "ALTER TABLE artifacts ADD COLUMN kind TEXT NOT NULL DEFAULT 'html'
        CHECK (kind IN ('html', 'live'));
    CREATE TABLE live_pages (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE (origin, path)
    );
    CREATE TABLE live_watches (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        replies_armed INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, origin, path)
    );
    ALTER TABLE watches ADD COLUMN source TEXT NOT NULL DEFAULT 'direct'
        CHECK (source IN ('direct', 'scope'));
    CREATE TABLE live_pending (
        artifact_id TEXT NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('explicit', 'resolve')),
        harness TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, thread_id)
    );",
```

Add to the migrations test module:

```rust
    #[test]
    fn migration_15_marks_existing_artifacts_html() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        {
            let mut c = Connection::open(home.db_path()).unwrap();
            let tx = c.transaction().unwrap();
            for sql in &MIGRATIONS[..14] {
                tx.execute_batch(sql).unwrap();
            }
            tx.pragma_update(None, "user_version", 14).unwrap();
            tx.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                 VALUES ('7q3k9mzx2b4t', 'T', 'x', 'x', 1, '0.2.61')",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let st = Store::open(&home).unwrap();
        let kind: String = st
            .with_read(|c| Ok(c.query_row("SELECT kind FROM artifacts", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(kind, "html");
    }
```

- [ ] **Step 8: Add `kind` to `Artifact` and the artifact reader**

In `crates/clax-core/src/model.rs`, add to `Artifact`:

```rust
    /// `html` (published by an agent) or `live` (a live page, spec 2026-10-05 §5.1).
    #[serde(default = "html_kind")]
    pub kind: String,
```

with `fn html_kind() -> String { crate::live::KIND_HTML.to_string() }` beside it. In `crates/clax-core/src/store/artifacts.rs`, add `kind` to `SELECT` (after `contract_version`) and `kind: r.get("kind")?,` to `row_to_artifact`. Change `fn write_version` to `pub(super) fn write_version`. In `delete_artifact`, after the `send_batches` delete, add:

```rust
            tx.execute(
                "DELETE FROM live_pages WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM live_pending WHERE artifact_id = ?1",
                params![id.as_str()],
            )?;
```

In `delete_zero_version_artifacts`, before the `DELETE FROM artifacts`, add `DELETE FROM live_pages WHERE artifact_id = ?1` with the same parameter.

- [ ] **Step 9: Add `route` to anchors**

In `crates/clax-core/src/anchor.rs`, add to `Anchor` after `file`:

```rust
    /// The route a live page's thread was made at (spec 2026-10-05 §5.2):
    /// the query and hash route, set by the daemon; absent elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
```

In `Anchor::validate`, add:

```rust
        if let Some(r) = &self.route {
            if r.len() > crate::live::MAX_ROUTE
                || !(r.starts_with('?') || r.starts_with('#'))
                || r.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
            {
                return Err(bad("route must start with ? or #, be at most 512 bytes, and hold no control characters"));
            }
        }
```

In `Anchor::summary`, prefix `"{route} › "` when `route` is set (before the existing `file` prefix logic), and add a test beside the existing summary tests:

```rust
    #[test]
    fn a_route_leads_the_summary() {
        let a: Anchor = serde_json::from_value(serde_json::json!({
            "kind": "element", "selector": "main > button", "quote": "Save", "route": "?tab=billing"
        })).unwrap();
        assert!(a.summary().starts_with("?tab=billing › main > button"));
    }
```

Every `Anchor { … }` literal in the crate's tests gains `route: None` (the compiler lists them). In `web/bridge/src/protocol.ts`, add to `Anchor`:

```ts
  /** A live page's route (`?query#/hash-route`), set by the daemon; absent on artifacts. */
  route?: string;
```

In `Store::create_thread` (`store/threads.rs`), inside the transaction after `artifact_live`, add:

```rust
            if t.anchor.route.is_some() {
                let kind: String = tx.query_row(
                    "SELECT kind FROM artifacts WHERE id = ?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )?;
                if kind != crate::live::KIND_LIVE {
                    return Err(CoreError::invalid("invalid_anchor", "route is only for live pages"));
                }
            }
```

- [ ] **Step 10: Implement the live-page store**

Above the tests in `crates/clax-core/src/store/live.rs`:

```rust
//! Live pages (spec 2026-10-05-chrome-overlay-design §5): artifacts of kind
//! `live`, keyed by origin and path, whose versions are snapshots.

use super::Store;
use crate::live::{KIND_LIVE, PageKey, placeholder_html};
use crate::model::{Artifact, CONTRACT_VERSION, Version};
use crate::publish::{Encoding, FileInput, INDEX, PublishRequest};
use crate::{ArtifactId, CoreError, Result};
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// A live page's key and its artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LivePage {
    pub artifact_id: String,
    pub origin: String,
    pub path: String,
}

/// What [`Store::ensure_live_page`] found or made.
#[derive(Clone, Debug)]
pub struct EnsuredPage {
    pub artifact: Artifact,
    /// The version a thread made now belongs on.
    pub version: Version,
    /// The page did not exist before this call.
    pub created: bool,
    /// This call wrote `version`.
    pub new_version: bool,
    /// Sessions a scope watch made watchers of the page in this call (Task 3).
    pub scoped_sessions: Vec<String>,
}

fn row_to_page(r: &Row<'_>) -> rusqlite::Result<LivePage> {
    Ok(LivePage {
        artifact_id: r.get(0)?,
        origin: r.get(1)?,
        path: r.get(2)?,
    })
}

fn page_by_key(c: &Connection, key: &PageKey) -> Result<Option<LivePage>> {
    Ok(c.query_row(
        "SELECT p.artifact_id, p.origin, p.path FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
         WHERE p.origin = ?1 AND p.path = ?2 AND a.deleted_at IS NULL",
        params![key.origin, key.path],
        row_to_page,
    )
    .optional()?)
}

impl Store {
    /// The live page `key` names, if it exists (including one whose first
    /// version is still being written).
    pub fn find_live_page(&self, key: &PageKey) -> Result<Option<LivePage>> {
        self.with_read(|c| page_by_key(c, key))
    }

    /// The live page whose artifact is `id`, or `None` for any other artifact.
    pub fn live_page_of(&self, id: &ArtifactId) -> Result<Option<LivePage>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT artifact_id, origin, path FROM live_pages WHERE artifact_id = ?1",
                params![id.as_str()],
                row_to_page,
            )
            .optional()?)
        })
    }

    /// Finds or creates the live page `key` and gives it the version a new
    /// thread belongs on: a new page's version 1 is `snapshot`, or the
    /// placeholder without one; an existing page takes `snapshot` as a new
    /// version when it differs from the current one ([`Store::store_snapshot`]).
    /// Concurrent first calls for one key settle on one page.
    pub fn ensure_live_page(&self, key: &PageKey, title: &str, snapshot: Option<&[u8]>) -> Result<EnsuredPage> {
        let (id, created) = self.with_tx(|tx| {
            if let Some(p) = page_by_key(tx, key)? {
                return Ok((ArtifactId::parse(&p.artifact_id)?, false));
            }
            let id = ArtifactId::generate();
            let now = Store::now();
            tx.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, pinned,
                    capabilities_json, contract_version, kind)
                 VALUES (?1, ?2, ?3, ?3, 0, 0, '{}', ?4, 'live')",
                params![id.as_str(), title, now, CONTRACT_VERSION],
            )?;
            tx.execute(
                "INSERT INTO live_pages (artifact_id, origin, path, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id.as_str(), key.origin, key.path, now],
            )?;
            Ok((id, true))
        })?;
        let current: u32 = self.with_read(|c| {
            Ok(c.query_row("SELECT current_version FROM artifacts WHERE id = ?1", params![id.as_str()], |r| r.get(0))?)
        })?;
        let (version, new_version) = if current == 0 {
            let html = snapshot.map(<[u8]>::to_vec).unwrap_or_else(|| placeholder_html(key).into_bytes());
            match self.write_snapshot(&id, 0, title, &html) {
                Ok(v) => (v, true),
                // Another first call wrote version 1 meanwhile: build on it.
                Err(CoreError::Conflict { .. }) => match snapshot {
                    Some(s) => self.store_snapshot(&id, title, s, false)?,
                    None => (self.get_version(&id, 1)?.ok_or(CoreError::NotFound)?, false),
                },
                Err(e) => return Err(e),
            }
        } else if let Some(s) = snapshot {
            self.store_snapshot(&id, title, s, false)?
        } else {
            (self.get_version(&id, current)?.ok_or(CoreError::NotFound)?, false)
        };
        let artifact = self.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        Ok(EnsuredPage { artifact, version, created, new_version, scoped_sessions: Vec::new() })
    }

    /// Stores `html` as the next version of the live page `id`, titled
    /// `title`, unless it is byte-identical to the current version's
    /// `index.html` and `force` is false; then the current version is
    /// returned. The flag says whether a version was written.
    ///
    /// # Errors
    /// `NotFound` for a missing or deleted artifact; `not_live` for an
    /// artifact that is not a live page.
    pub fn store_snapshot(&self, id: &ArtifactId, title: &str, html: &[u8], force: bool) -> Result<(Version, bool)> {
        let a = self.get_artifact(id)?.ok_or(CoreError::NotFound)?;
        if a.kind != KIND_LIVE {
            return Err(CoreError::invalid("not_live", format!("{id} is not a live page")));
        }
        if !force {
            let current = std::fs::read(self.home.version_dir(id, a.current_version).join(INDEX)).ok();
            if current.as_deref() == Some(html) {
                return Ok((self.get_version(id, a.current_version)?.ok_or(CoreError::NotFound)?, false));
            }
        }
        Ok((self.write_snapshot(id, a.current_version, title, html)?, true))
    }

    /// Writes `html` as version `expected + 1` (its only file), noted `snapshot`.
    fn write_snapshot(&self, id: &ArtifactId, expected: u32, title: &str, html: &[u8]) -> Result<Version> {
        let p = crate::publish::validate(PublishRequest {
            title: Some(title.to_string()),
            note: Some("snapshot".to_string()),
            if_version: Some(expected),
            files: BTreeMap::from([(
                INDEX.to_string(),
                Some(FileInput {
                    content: base64::engine::general_purpose::STANDARD.encode(html),
                    encoding: Encoding::Base64,
                    content_type: None,
                }),
            )]),
            ..Default::default()
        })?;
        let (_, v) = self.write_version(id, expected, &p, &BTreeMap::new(), None)?;
        Ok(v)
    }
}
```

Add `pub mod live;` to `crates/clax-core/src/store/mod.rs`. If `publish::INDEX` is not public, make it `pub const`.

- [ ] **Step 11: Run the core tests**

Run: `cargo test -p clax-core`
Expected: PASS (including the migration test and every earlier test).

- [ ] **Step 12: Commit**

```bash
git add Cargo.toml crates/clax-core web/bridge/src/protocol.ts
git -c commit.gpgsign=false commit -m "Add live pages: the page key and route, migration 15, and snapshot versions"
```

---

### Task 2: Live-page routes, views and serving

**Files:**
- Create: `crates/clax-server/src/live.rs`
- Create: `crates/clax-server/src/routes/live.rs`
- Create: `crates/clax-server/tests/api_live.rs`
- Modify: `crates/clax-server/src/lib.rs` (`pub mod live;`) and `crates/clax-server/Cargo.toml` (`url.workspace = true`)
- Modify: `crates/clax-server/src/state.rs` (`live_ids: Arc<crate::live::LiveIds>`), `crates/clax-server/src/daemon.rs` and `crates/clax-server/src/testing.rs` (build it with `LiveIds::load(&store)`)
- Modify: `crates/clax-server/src/routes/mod.rs` (routes; the `hide_live_pages` layer)
- Modify: `crates/clax-server/src/routes/threads.rs` (extract `create_thread_now`)
- Modify: `crates/clax-server/src/routes/artifacts.rs` (views carry `live`; `publish` refuses live pages; `list` hides live pages from non-local callers)
- Modify: `crates/clax-server/src/routes/content.rs` (the snapshot policy)
- Modify: `crates/clax-server/src/stream.rs` (gallery events of live pages reach only local streams)

**Interfaces:**
- Consumes: Task 1's `parse_page_url`, `Store::{find_live_page, live_page_of, ensure_live_page}`, `Artifact.kind`.
- Produces:
  - `GET /api/live/pages?url=` → `{page: PageView | null, route: string | null}` where `PageView = {artifact_id, origin, path, page_url, title, current_version, url}` (`url` is the Clax view `/a/<aid>` on `browser_base`).
  - `POST /api/live/threads` (multipart `url`, `title`, `anchor`, `body`, `clip?`, `snapshot`) → `201 {thread, page: PageView, version: u32, clip_error?}`.
  - `crate::live::{LiveIds, is_local(&HeaderMap, &Extensions, token: &str) -> bool, hide_live_pages}`; `LiveIds::{load(&Store) -> Result<LiveIds>, insert(&str), remove(&str), contains(&str) -> bool}`.
  - `routes::threads::create_thread_now(st: &Store, ctx: &FeedbackCtx, id: &ArtifactId, t: NewThread, with_path: bool) -> clax_core::Result<Value>` (the thread view; handles `@agent`).
  - `routes::live::page_view(s: &AppState, p: &LivePage, a: &Artifact) -> Value`.
  - Artifact views (`GET /api/artifacts`, `/api/artifacts/<aid>`, bootstrap) carry `kind`, and `live: {origin, path, page_url}` for live pages.

- [ ] **Step 1: Write the failing route tests**

`crates/clax-server/tests/api_live.rs`:

```rust
mod common;
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use serde_json::{Value, json};

fn anchor() -> String {
    json!({"kind": "element", "selector": "main > button", "quote": "Save", "file": "index.html"}).to_string()
}

async fn post_thread(ts: &TestServer, cookie: &str, url: &str, snapshot: &str) -> reqwest::Response {
    let form = reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "Settings")
        .text("anchor", anchor())
        .text("body", "The button overflows")
        .text("snapshot", snapshot.to_string())
        .part("clip", reqwest::multipart::Part::bytes(FAKE_PNG.to_vec()).mime_str("image/png").unwrap());
    ts.client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_first_comment_creates_the_page_its_snapshot_and_the_thread_with_its_route() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let res = post_thread(&ts, &v.cookie, "http://localhost:5173/settings?tab=billing&utm_source=x", "<!doctype html><main><button>Save</button></main>").await;
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["page"]["page_url"], "http://localhost:5173/settings");
    assert_eq!(body["version"], 1);
    assert_eq!(body["thread"]["anchor"]["route"], "?tab=billing");
    assert_eq!(body["thread"]["has_clip"], true);
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(a["artifact"]["kind"], "live");
    assert_eq!(a["artifact"]["live"]["origin"], "http://localhost:5173");
    let found: Value = ts.get("/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2Fsettings%23%2Fx").await.json().await.unwrap();
    assert_eq!(found["page"]["artifact_id"], aid);
    assert_eq!(found["route"], "#/x");
    let none: Value = ts.get("/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2Fother").await.json().await.unwrap();
    assert!(none["page"].is_null(), "a lookup never creates");
}

#[tokio::test]
async fn an_unchanged_snapshot_reuses_the_version() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let html = "<!doctype html><p>same</p>";
    let a: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", html).await.json().await.unwrap();
    let b: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", html).await.json().await.unwrap();
    assert_eq!(a["version"], 1);
    assert_eq!(b["version"], 1);
    let c: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<!doctype html><p>new</p>").await.json().await.unwrap();
    assert_eq!(c["version"], 2);
}

#[tokio::test]
async fn the_daemons_own_origin_and_other_schemes_are_refused() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let own = format!("http://localhost:{}/a/7q3k9mzx2b4t", ts.addr.port());
    let res = post_thread(&ts, &v.cookie, &own, "<p>").await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "own_origin");
    let res = post_thread(&ts, &v.cookie, "file:///etc/passwd", "<p>").await;
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "unsupported_url");
}

#[tokio::test]
async fn a_live_page_cannot_be_published() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>").await.json().await.unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>x", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "live_page");
}

#[tokio::test]
async fn snapshots_are_served_with_a_policy_that_runs_only_claxs_scripts() {
    let ts = TestServer::spawn().await;
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<!doctype html><p>x</p>").await.json().await.unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let res = ts.get(&format!("/c/{aid}/v/1/")).await;
    let policies: Vec<String> = res.headers().get_all("content-security-policy").iter().map(|v| v.to_str().unwrap().to_string()).collect();
    let host = format!("localhost:{}", ts.addr.port());
    assert!(policies.iter().any(|p| p.contains(&format!("script-src http://{host}/_clax/")) && p.contains("connect-src 'none'")), "{policies:?}");
    // An HTML artifact has no such policy.
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let res = ts.get(&format!("/c/{}/v/1/", a["artifact"]["id"].as_str().unwrap())).await;
    assert!(!res.headers().get_all("content-security-policy").iter().any(|v| v.to_str().unwrap().contains("script-src")));
}
```

Add a LAN test (the `spawn_on` helper binds an unspecified address; reach it from a non-loopback interface address as `crates/clax-server/tests/api_host.rs` does — copy its helper that finds a LAN address and skip the test with a printed note when the machine has none):

```rust
#[tokio::test]
async fn live_pages_are_hidden_from_lan_callers_without_the_token() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let Some(lan) = lan_base(&ts) else { eprintln!("no LAN address; skipped"); return };
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<p>").await.json().await.unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap();
    let list: Value = ts.client.get(format!("{lan}/api/artifacts")).send().await.unwrap().json().await.unwrap();
    assert!(!list["artifacts"].as_array().unwrap().iter().any(|a| a["id"] == aid));
    for path in [format!("/api/artifacts/{aid}"), format!("/api/artifacts/{aid}/threads"), format!("/c/{aid}/v/1/"), format!("/a/{aid}")] {
        let st = ts.client.get(format!("{lan}{path}")).send().await.unwrap().status();
        assert_eq!(st, 404, "{path}");
    }
    // Loopback, and the token from the LAN, still see it.
    assert_eq!(ts.get(&format!("/api/artifacts/{aid}")).await.status(), 200);
    let st = ts.authed(ts.client.get(format!("{lan}/api/artifacts/{aid}"))).send().await.unwrap().status();
    assert_eq!(st, 200);
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p clax-server --test api_live`
Expected: FAIL: `/api/live/threads` answers 404 (no route), `kind` missing.

- [ ] **Step 3: Extract `create_thread_now` from `threads::create`**

In `crates/clax-server/src/routes/threads.rs`, move the body of `create`'s `store_call` closure into:

```rust
/// Creates the thread `t` on `id` as its viewer, sends it when its body
/// mentions `@agent` (unless the page wrote it), publishes the `thread`
/// event, and returns the thread view (with `clip_path` when `with_path`).
pub(crate) fn create_thread_now(
    st: &Store,
    ctx: &crate::feedback::FeedbackCtx,
    id: &ArtifactId,
    t: NewThread,
    with_path: bool,
) -> clax_core::Result<Value> {
    let mention = !t.via_page && mentions_agent(&t.body);
    let mut thread = st.create_thread(id, t)?;
    if mention {
        let (sent, touched) = st.send_to_agent(&thread.id)?;
        thread = sent;
        apply(ctx, st, &touched);
    }
    publish_thread(ctx, st, &thread)?;
    thread_view(st, &thread, ctx.codex_push(), with_path)
}
```

and make `create` call it (`let (author, author_public_id) = author(st, …)?; create_thread_now(st, &ctx, &id, NewThread { … }, with_path)`). Run `cargo test -p clax-server --test api_threads` and expect PASS before going on.

- [ ] **Step 4: Add `LiveIds` and the LAN rule**

`crates/clax-server/src/live.rs`:

```rust
//! Live pages on the daemon (spec 2026-10-05-chrome-overlay-design L10):
//! the set of live-page artifact IDs, and the rule that hides them from
//! callers that are neither on this machine nor hold the token.

use crate::auth::{Conn, has_token, is_loopback};
use axum::http::{HeaderMap, Extensions};
use clax_core::Store;
use std::collections::HashSet;
use std::sync::RwLock;

/// Every live page's artifact ID, kept in step with the store.
#[derive(Default)]
pub struct LiveIds(RwLock<HashSet<String>>);

impl LiveIds {
    /// The live pages the store holds now.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<LiveIds> {
        Ok(LiveIds(RwLock::new(st.live_page_ids()?.into_iter().collect())))
    }
    pub fn insert(&self, id: &str) {
        self.0.write().expect("live IDs lock").insert(id.to_string());
    }
    pub fn remove(&self, id: &str) {
        self.0.write().expect("live IDs lock").remove(id);
    }
    pub fn contains(&self, id: &str) -> bool {
        self.0.read().expect("live IDs lock").contains(id)
    }
}

/// Whether the request may see live pages: it came over loopback, or it
/// carries the token.
pub fn is_local(headers: &HeaderMap, ext: &Extensions, token: &str) -> bool {
    has_token(headers, token)
        || ext
            .get::<axum::extract::ConnectInfo<Conn>>()
            .is_some_and(|c| is_loopback(c.0.peer))
}

/// The artifact a path names: `/api/artifacts/<aid>…`, `/c/<aid>/…`, `/a/<aid>…`.
fn artifact_in(path: &str) -> Option<&str> {
    let rest = path
        .strip_prefix("/api/artifacts/")
        .or_else(|| path.strip_prefix("/c/"))
        .or_else(|| path.strip_prefix("/a/"))?;
    Some(rest.split(['/', ':']).next().unwrap_or(""))
}

/// Middleware: a live page's API, content and shell paths answer 404 to a
/// caller [`is_local`] refuses, exactly as a missing artifact does. Artifact
/// hosts (`<aid>.localhost`) resolve to loopback and pass.
pub async fn hide_live_pages(
    axum::extract::State(s): axum::extract::State<crate::state::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Some(aid) = artifact_in(req.uri().path())
        && s.live_ids.contains(aid)
        && !is_local(req.headers(), req.extensions(), &s.token)
    {
        return crate::error::ApiError::from(clax_core::CoreError::NotFound).into_response();
    }
    next.run(req).await
}
```

Add to `Store` (in `store/live.rs`):

```rust
    /// The artifact IDs of every live page that is not deleted.
    pub fn live_page_ids(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut st = c.prepare("SELECT p.artifact_id FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id WHERE a.deleted_at IS NULL")?;
            let ids = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(ids)
        })
    }
```

Add `pub live_ids: Arc<crate::live::LiveIds>` to `AppState`, built with `LiveIds::load(&store)?` in `daemon.rs` and `testing.rs`. In `routes/mod.rs`, apply `.layer(axum::middleware::from_fn_with_state(state.clone(), crate::live::hide_live_pages))` to the final router before `.with_state(state)`. In `artifacts::delete`, call `s.live_ids.remove(&aid)` after a successful delete. In `artifacts::list`, add the parameters `headers: HeaderMap` and `ConnectInfo(conn): ConnectInfo<Conn>`, compute `let local = has_token(&headers, &s.token) || is_loopback(conn.peer);`, and drop artifacts whose `kind` is `live` when `local` is false. `api_live.rs` gets the `lan_base(&TestServer) -> Option<String>` helper by copying the one `crates/clax-server/tests/api_host.rs` uses to reach a test daemon on a non-loopback interface address.

- [ ] **Step 5: Gallery events of live pages reach only local streams**

In `crates/clax-server/src/stream.rs`: give `Sub` a `local: bool` and `Gate::admits` a second argument:

```rust
#[derive(Clone, Copy, Debug)]
enum Gate {
    Any,
    /// Only subscribers on this machine or holding the token (a live page's gallery event).
    Local,
    Level { min: Level, unless: Option<Level> },
}

impl Gate {
    fn admits(self, level: Level, local: bool) -> bool {
        match self {
            Gate::Any => true,
            Gate::Local => local,
            Gate::Level { min, unless } => level >= min && unless.is_none_or(|u| level < u),
        }
    }
}
```

`Hub::open(caller, local, resume)` stores `local` on the stream and on every `Sub` it creates; `Hub` holds `live: Arc<crate::live::LiveIds>` (given to `Hub::new`), and where it files an event on `Chan::Gallery`, it uses `Gate::Local` when the event's artifact ID is in `live`. A subscription of a non-local stream to `artifact:`, `working:`, `presence:` or `docs:` of a live page is refused like an unknown artifact (404 `not_found`). `routes::stream::open` computes `local` with `crate::live::is_local(&headers, &extensions, &s.token)` (take `extensions: axum::http::Extensions` through `axum::extract::Request` parts, or `ConnectInfo<Conn>` as in Step 4). Update the hub's unit tests' `open` calls to pass `true`, and add:

```rust
    #[test]
    fn a_live_pages_gallery_events_reach_only_local_streams() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert("7q3k9mzx2b4t");
        let hub = Hub::new_for_test(live);
        let near = hub.open(viewer(Level::View, None), true, None);
        let far = hub.open(viewer(Level::View, None), false, None);
        for o in [&near, &far] {
            hub.update(&o.id, &viewer(Level::View, None), &[Topic::Gallery], &[]).unwrap();
        }
        hub.dispatch(&version_event("7q3k9mzx2b4t"));
        assert_eq!(drain(&hub, &near).len(), 1);
        assert_eq!(drain(&hub, &far).len(), 0);
    }
```

(`new_for_test`, `version_event` and `drain` follow the module's existing test helpers; reuse those that exist and add the missing ones in the test module.)

- [ ] **Step 6: Implement the routes**

`crates/clax-server/src/routes/live.rs`:

```rust
//! `/api/live/*` (spec 2026-10-05-chrome-overlay-design §9.2): look up a
//! page URL's live page, and post a comment on a live page with its
//! screenshot and snapshot. Viewer routes: no token, the shell's origin or
//! the extension through its gateway.

use super::artifacts::path;
use super::assets::multipart_error;
use super::threads::create_thread_now;
use crate::auth::has_token;
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie, author};
use axum::Json;
use axum::extract::multipart::MultipartRejection;
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use clax_core::live::{PageUrl, parse_page_url};
use clax_core::model::Artifact;
use clax_core::store::live::LivePage;
use clax_core::store::threads::{NewThread, clip_problem};
use clax_core::{Anchor, ArtifactId, CoreError, Event};
use serde::Deserialize;
use serde_json::{Value, json};

/// The request cap of `POST /api/live/threads`: an 8 MiB snapshot, a 5 MiB
/// clip, and the rest.
pub const LIVE_THREAD_LIMIT: usize = 24 * 1024 * 1024;
/// The largest snapshot accepted.
pub const MAX_SNAPSHOT: usize = 8 * 1024 * 1024;
/// The longest page title kept.
pub const MAX_TITLE: usize = 200;

/// `raw` parsed, refusing the daemon's own pages (`own_origin`): a local
/// name (`localhost`, `127.0.0.1`, `[::1]`, `*.localhost`) or the bind
/// address, on the daemon's port.
pub(crate) fn page_url(s: &AppState, raw: &str) -> Result<PageUrl, ApiError> {
    let p = parse_page_url(raw).map_err(ApiError::from)?;
    let origin = url::Url::parse(&p.key.origin)
        .map_err(|e| ApiError::bad_request("invalid_url", e.to_string()))?;
    let own = url::Url::parse(&s.self_base).ok();
    let own_port = own.as_ref().and_then(url::Url::port_or_known_default);
    let own_host = own.as_ref().and_then(|u| u.host_str().map(str::to_string));
    let host = origin.host_str().unwrap_or("");
    let ours = matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        || host.ends_with(".localhost")
        || own_host.as_deref() == Some(host);
    if ours && origin.port_or_known_default() == own_port {
        return Err(ApiError::bad_request("own_origin", "Clax's own pages have their own comment mode"));
    }
    Ok(p)
}

/// A title as a page gave it: control characters dropped, whitespace collapsed, cut.
fn clean_title(t: &str, fallback: &str) -> String {
    let t: String = clax_core::anchor::collapse(&t.chars().filter(|c| !c.is_control()).collect::<String>())
        .chars()
        .take(MAX_TITLE)
        .collect();
    if t.is_empty() { fallback.to_string() } else { t }
}

/// The view of a live page the routes answer with.
pub(crate) fn page_view(s: &AppState, p: &LivePage, a: &Artifact) -> Value {
    json!({
        "artifact_id": p.artifact_id,
        "origin": p.origin,
        "path": p.path,
        "page_url": format!("{}{}", p.origin, p.path),
        "title": a.title,
        "current_version": a.current_version,
        "url": format!("{}/a/{}", s.browser_base.trim_end_matches('/'), p.artifact_id),
    })
}

#[derive(Deserialize)]
pub struct PageQuery {
    url: String,
}

/// `GET /api/live/pages?url=`: the live page the URL names, or null, and the
/// URL's route. Never creates a page.
pub async fn page(
    State(s): State<AppState>,
    _o: SameOrigin,
    q: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = page_url(&s, &q.url)?;
    let key = pu.key.clone();
    let found = s
        .store_call(move |st| {
            let Some(p) = st.find_live_page(&key)? else { return Ok(None) };
            let Some(a) = st.get_artifact(&ArtifactId::parse(&p.artifact_id)?)? else { return Ok(None) };
            Ok(Some((p, a)))
        })
        .await?;
    Ok(Json(json!({
        "page": found.map(|(p, a)| page_view(&s, &p, &a)),
        "route": pu.route,
    })))
}

/// `POST /api/live/threads`: finds or creates the live page `url` names,
/// stores `snapshot` as its next version when it changed, and creates the
/// thread there as the viewer, with `route` from `url` and `clip` as its
/// screenshot. A clip failing `clip_problem` is dropped and reported.
pub async fn thread(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let (mut url, mut title, mut anchor, mut body, mut clip, mut snapshot) = (None, None, None, None, None, None);
    while let Some(field) = mp.next_field().await.map_err(|e| multipart_error(e.status(), e.body_text()))? {
        let name = field.name().unwrap_or("").to_string();
        let bytes = field.bytes().await.map_err(|e| multipart_error(e.status(), e.body_text()))?;
        let text = || String::from_utf8(bytes.to_vec()).map_err(|_| ApiError::bad_request("invalid_field", format!("{name} is not UTF-8")));
        match name.as_str() {
            "url" => url = Some(text()?),
            "title" => title = Some(text()?),
            "anchor" => anchor = Some(text()?),
            "body" => body = Some(text()?),
            "clip" => clip = Some(bytes.to_vec()),
            "snapshot" => {
                if bytes.len() > MAX_SNAPSHOT {
                    return Err(ApiError::bad_request("snapshot_too_large", "a snapshot is at most 8 MiB"));
                }
                snapshot = Some(bytes.to_vec());
            }
            _ => {}
        }
    }
    let pu = page_url(&s, url.as_deref().ok_or_else(|| ApiError::bad_request("invalid_args", "url is required"))?)?;
    let mut anchor: Anchor = serde_json::from_str(anchor.as_deref().unwrap_or(""))
        .map_err(|e| ApiError::bad_request("invalid_anchor", e.to_string()))?;
    anchor.route = pu.route.clone();
    let snapshot = snapshot.ok_or_else(|| ApiError::bad_request("invalid_args", "snapshot is required"))?;
    let title = clean_title(title.as_deref().unwrap_or(""), &pu.key.page_url());
    let clip_error = clip.as_deref().and_then(clip_problem);
    let clip = if clip_error.is_some() { None } else { clip };
    let with_path = has_token(&headers, &s.token);
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let live_ids = s.live_ids.clone();
    let key = pu.key.clone();
    let (view, page, artifact, version) = s
        .store_call(move |st| {
            let e = st.ensure_live_page(&key, &title, Some(&snapshot))?;
            live_ids.insert(&e.artifact.id);
            let id = ArtifactId::parse(&e.artifact.id)?;
            if e.new_version {
                events.publish(Event::Version {
                    artifact_id: e.artifact.id.clone(),
                    n: e.version.n,
                    by_page: false,
                    title: Some(e.artifact.title.clone()),
                    at: Some(e.version.created_at.clone()),
                });
            }
            let (author_name, author_public_id) = author(st, viewer.0.as_deref())?;
            let view = create_thread_now(st, &ctx, &id, NewThread {
                author_public_id,
                version_n: e.version.n,
                anchor,
                author_name,
                body: body.unwrap_or_default(),
                clip,
                via_page: false,
            }, with_path)?;
            let page = st.live_page_of(&id)?.ok_or(CoreError::NotFound)?;
            Ok((view, page, e.artifact, e.version.n))
        })
        .await?;
    let mut out = json!({"thread": view, "page": page_view(&s, &page, &artifact), "version": version});
    if let Some(e) = clip_error {
        out["clip_error"] = json!(e);
    }
    Ok((StatusCode::CREATED, Json(out)))
}
```

Register in `routes/mod.rs`: `.route("/api/live/pages", get(live::page))` in `api_fast`, and `.route("/api/live/threads", post(live::thread.layer(DefaultBodyLimit::max(live::LIVE_THREAD_LIMIT))))` in `api_slow`; add `pub mod live;`. If `anchor::collapse` is not public, use the `collapse` it re-exports (check `crates/clax-core/src/lib.rs`).

- [ ] **Step 7: Artifact views, the publish refusal and the snapshot policy**

In `routes/artifacts.rs`: where an artifact view is built (`with_owner` or the list/get handlers), add for a live artifact `view["live"] = json!({"origin", "path", "page_url"})` from `st.live_page_of(&id)`. In `publish`, before validating, refuse:

```rust
    if s.live_ids.contains(id.as_str()) {
        return Err(ApiError::bad_request("live_page", "a live page takes snapshots from the Clax extension; it cannot be published"));
    }
```

In `routes/content.rs`, for an HTML response (`Served::Page`) of an artifact in `s.live_ids`, append a second policy:

```rust
/// The second policy on a live page's snapshot (spec 2026-10-05 §8.4): only
/// Clax's own scripts under `/_clax/` on the request's host run.
fn snapshot_policy(req: &HeaderMap) -> HeaderValue {
    let host = req.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
    HeaderValue::from_str(&format!(
        "script-src http://{host}/_clax/; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'; worker-src 'none'"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("script-src 'none'"))
}
```

with `res.headers_mut().append(header::CONTENT_SECURITY_POLICY, snapshot_policy(&req))` in `index` and in `file` for HTML pages when `s.live_ids.contains(&aid)`.

- [ ] **Step 8: Run the tests**

Run: `cargo test -p clax-server --test api_live && cargo test -p clax-server`
Expected: PASS.

- [ ] **Step 9: Document and commit**

In `docs/contract.md`, add a "Live pages" section after "Comments and feedback": the key and route rules (spec §7), `GET /api/live/pages`, `POST /api/live/threads`, the `live_page` publish error, the snapshot policy, and the LAN rule; add the LAN rule to "Security model".

```bash
git add crates/clax-server docs/contract.md
git -c commit.gpgsign=false commit -m "Serve live pages: lookup, comment with snapshot, views, snapshot policy, LAN hiding"
```

---

### Task 3: "Addressed" on live pages

**Files:**
- Modify: `crates/clax-core/src/store/live.rs` (pending addresses)
- Modify: `crates/clax-core/src/store/threads.rs` (deleting a thread deletes its pending address)
- Modify: `crates/clax-server/src/routes/threads.rs` (`addressed` on agent replies; agent resolve on a live page)
- Modify: `crates/clax-server/src/routes/live.rs` (`POST /api/live/snapshots`; link pending addresses on every new snapshot)
- Modify: `crates/clax-server/src/feedback.rs` (thread views carry `addressed_pending`)
- Modify: `crates/clax-server/src/routes/mod.rs`
- Modify: `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/src/client.rs` (`comments_reply` takes `addressed`)
- Modify: `plugins/pi/src/clax.ts`, `plugins/pi/test/fixtures/contract.json`, `plugins/pi/test/clax.test.ts` (the same argument)
- Test: `crates/clax-server/tests/api_live.rs`, `crates/clax-mcp/tests/comments.rs`

**Interfaces:**
- Consumes: Task 1's `Store::{store_snapshot, ensure_live_page}`; Task 2's `routes::live::{page_url, page_view, clean_title, MAX_SNAPSHOT, LIVE_THREAD_LIMIT}`, `LiveIds`, `create_thread_now`.
- Produces:
  - `Store::mark_pending(&ArtifactId, tid: &str, source: &str /* "explicit" | "resolve" */, harness: &str) -> Result<bool>`
  - `Store::link_pending(&ArtifactId, n: u32) -> Result<Vec<String>>` (thread IDs linked to version `n`)
  - `Store::pending_address(tid: &str) -> Result<Option<(String /* harness */, String /* created_at */)>>`
  - `Store::has_pending(&ArtifactId) -> Result<bool>`
  - Thread views carry `addressed_pending: {harness, at} | null`.
  - `POST /api/live/snapshots` (multipart `url`, `title`, `snapshot`) → `{page, version, linked}`; 409 `nothing_pending`.
  - `POST …/threads/<tid>/comments` accepts `addressed: true` on an agent reply on a live page; the answer carries `addressed: "pending"`.
  - MCP `comments_reply` gains `addressed: Option<bool>`; `DaemonClient::reply(&self, id, tid, text, addressed: bool)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/clax-server/tests/api_live.rs`:

```rust
/// A live page with one thread, sent to a `claude` session watching it.
async fn sent_live_thread(ts: &TestServer) -> (String, String, String) {
    let v = ts.viewer(Some("Alex")).await;
    let body: Value = post_thread(ts, &v.cookie, "http://localhost:5173/", "<!doctype html><p>v1</p>").await.json().await.unwrap();
    let aid = body["page"]["artifact_id"].as_str().unwrap().to_string();
    let tid = body["thread"]["id"].as_str().unwrap().to_string();
    let s = ts.register_session("claude", "live-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let res = ts.authed(ts.client.put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base))).send().await.unwrap();
    assert_eq!(res.status(), 200);
    ts.send_thread(&aid, &tid).await;
    (aid, tid, sid)
}

async fn agent_reply(ts: &TestServer, aid: &str, tid: &str, sid: &str, addressed: bool) -> reqwest::Response {
    ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base)))
        .header("x-clax-session", sid)
        .json(&json!({"body": "Fixed", "author_kind": "agent", "addressed": addressed}))
        .send()
        .await
        .unwrap()
}

async fn post_snapshot(ts: &TestServer, cookie: &str, html: &str) -> reqwest::Response {
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Home")
        .text("snapshot", html.to_string());
    ts.client
        .post(format!("{}/api/live/snapshots", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn an_addressed_reply_waits_for_the_next_snapshot_and_links_to_it() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    let v = ts.viewer(Some("Mia")).await;
    assert_eq!(post_snapshot(&ts, &v.cookie, "<p>x").await.status(), 409, "nothing pending yet");
    let res = agent_reply(&ts, &aid, &tid, &sid, true).await;
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["addressed"], "pending");
    assert_eq!(body["thread"]["addressed_pending"]["harness"], "claude");
    let snap: Value = post_snapshot(&ts, &v.cookie, "<!doctype html><p>v1</p>").await.json().await.unwrap();
    assert_eq!(snap["version"], 2, "a snapshot after an address is a version even when identical");
    assert_eq!(snap["linked"], json!([tid]));
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{tid}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["addressed_in"], json!([2]));
    assert!(t["thread"]["addressed_pending"].is_null());
}

#[tokio::test]
async fn a_comment_snapshot_also_links_pending_addresses() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    agent_reply(&ts, &aid, &tid, &sid, true).await;
    let v = ts.viewer(Some("Mia")).await;
    let body: Value = post_thread(&ts, &v.cookie, "http://localhost:5173/", "<!doctype html><p>v2</p>").await.json().await.unwrap();
    assert_eq!(body["version"], 2);
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{tid}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["addressed_in"], json!([2]));
}

#[tokio::test]
async fn an_agent_resolve_on_a_live_page_is_pending_too() {
    let ts = TestServer::spawn().await;
    let (aid, tid, sid) = sent_live_thread(&ts).await;
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"as": "agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{tid}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["addressed_in"], json!([]), "not linked to the current version");
    assert_eq!(t["thread"]["addressed_pending"]["harness"], "claude");
}

#[tokio::test]
async fn addressed_is_refused_on_html_artifacts_and_for_viewers() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "h-1").await;
    let sid = s["id"].as_str().unwrap();
    let a = ts.publish_as(sid, "T", "<p>").await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let t = ts.thread(aid, 1, "x").await;
    let tid = t["id"].as_str().unwrap();
    ts.send_thread(aid, tid).await;
    let res = agent_reply(&ts, aid, tid, sid, true).await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "invalid_args");
    let v = ts.viewer(Some("Alex")).await;
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .json(&json!({"body": "x", "addressed": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}
```

Append to `crates/clax-mcp/tests/comments.rs` a test that `comments_reply` with `addressed: Some(true)` on an HTML artifact's sent thread returns an error result whose JSON `error.code` is `invalid_args`; every existing construction of `CommentsReplyArgs` gains `addressed: None`.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p clax-server --test api_live`
Expected: FAIL: `addressed` is an unknown field (400 `invalid_json`), and `/api/live/snapshots` answers 404.

- [ ] **Step 3: Pending addresses in the store**

Append to the `impl Store` in `crates/clax-core/src/store/live.rs`:

```rust
    /// Records that the agent of `harness` addressed thread `tid` of the
    /// live page `id`, to be linked to the page's next snapshot (spec L11).
    /// A `resolve` is recorded only when the thread has no version link and
    /// no pending address yet; an `explicit` one replaces a pending
    /// `resolve`. Returns whether a row was written.
    pub fn mark_pending(&self, id: &ArtifactId, tid: &str, source: &str, harness: &str) -> Result<bool> {
        self.with_tx(|tx| {
            if source == "resolve" {
                let linked: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM version_threads WHERE thread_id = ?1)
                        OR EXISTS(SELECT 1 FROM live_pending WHERE thread_id = ?1)",
                    params![tid],
                    |r| r.get(0),
                )?;
                if linked {
                    return Ok(false);
                }
            }
            tx.execute(
                "INSERT INTO live_pending (artifact_id, thread_id, source, harness, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(artifact_id, thread_id) DO UPDATE SET source = excluded.source,
                    harness = excluded.harness, created_at = excluded.created_at",
                params![id.as_str(), tid, source, harness, Store::now()],
            )?;
            Ok(true)
        })
    }

    /// Links every pending address of the live page `id` to its version `n`
    /// and clears them; returns the linked thread IDs.
    pub fn link_pending(&self, id: &ArtifactId, n: u32) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let ids: Vec<String> = {
                let mut st = tx.prepare(
                    "SELECT thread_id FROM live_pending WHERE artifact_id = ?1 ORDER BY created_at, thread_id",
                )?;
                st.query_map(params![id.as_str()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            tx.execute(
                "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
                 SELECT artifact_id, ?2, thread_id, source, ?3 FROM live_pending WHERE artifact_id = ?1",
                params![id.as_str(), n, Store::now()],
            )?;
            tx.execute("DELETE FROM live_pending WHERE artifact_id = ?1", params![id.as_str()])?;
            Ok(ids)
        })
    }

    /// The pending address of thread `tid`: the addressing agent's harness and when.
    pub fn pending_address(&self, tid: &str) -> Result<Option<(String, String)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT harness, created_at FROM live_pending WHERE thread_id = ?1",
                params![tid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// Whether the live page `id` has addresses waiting for a snapshot.
    pub fn has_pending(&self, id: &ArtifactId) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_pending WHERE artifact_id = ?1)",
                params![id.as_str()],
                |r| r.get(0),
            )?)
        })
    }
```

Deleting a thread must delete its `live_pending` row: in the store's thread delete (`grep -n "DELETE FROM version_threads WHERE thread_id" crates/clax-core/src/store/threads.rs`), add `tx.execute("DELETE FROM live_pending WHERE thread_id = ?1", params![tid])?;` beside it.

- [ ] **Step 4: The routes**

In `routes/threads.rs`, add to `CommentBody`:

```rust
    /// An agent reply on a live page says the page now shows the fix: the
    /// thread is linked to the page's next snapshot (spec 2026-10-05 L11).
    #[serde(default)]
    addressed: bool,
```

In `comment`, before writing anything: when `addressed` is true and the comment is not an agent reply, answer 400 `invalid_args` ("addressed is only for agent replies"); when the artifact is not in `s.live_ids`, answer 400 `invalid_args` ("addressed is for live pages; publish with addresses instead"). After an agent reply is written (`Outcome::Done`), when `addressed`, add:

```rust
            if addressed {
                st.mark_pending(&id, &tid, "explicit", &session.harness)?;
                let t = thread_of(st, &id, &tid)?;
                publish_thread(&ctx, st, &t)?;
                view = thread_view(st, &t, ctx.codex_push(), with_path)?;
                out["addressed"] = json!("pending");
            }
```

(adapt the names to the handler's: it already holds the agent's `Session` from `agent_session`, the feedback context and the view it answers with).

In `resolve`, where the agent path calls `st.link_on_resolve(&tid)?`, branch on the live page:

```rust
            if let Some(sess) = &agent {
                if live {
                    st.mark_pending(&id, &tid, "resolve", &sess.harness)?;
                } else {
                    st.link_on_resolve(&tid)?;
                }
            }
```

(`live` is `s.live_ids.contains(id.as_str())`, computed before the `store_call`; `agent` is the `Session` the agent path looks up, kept instead of only its ID.)

In `crate::feedback::thread_view` (`crates/clax-server/src/feedback.rs`), add:

```rust
    view["addressed_pending"] = match st.pending_address(&t.id)? {
        Some((harness, at)) => json!({"harness": harness, "at": at}),
        None => Value::Null,
    };
```

In `routes/live.rs::thread`, link on every new snapshot, right after `ensure_live_page`:

```rust
            if e.new_version {
                for tid in st.link_pending(&id, e.version.n)? {
                    if let Some(t) = st.get_thread(&tid)? {
                        super::threads::publish_thread(&ctx, st, &t)?;
                    }
                }
            }
```

(make `publish_thread` `pub(crate)` if it is not; `id` is parsed from `e.artifact.id` above this point). Add the snapshot route to `routes/live.rs`:

```rust
/// `POST /api/live/snapshots` (multipart `url`, `title`, `snapshot`): the
/// extension's snapshot of a page with pending addresses (spec L11). Always
/// a new version, linking every pending address to it; 409
/// `nothing_pending` when the page has none (or does not exist).
pub async fn snapshot(
    State(s): State<AppState>,
    _o: SameOrigin,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let (mut url, mut title, mut html) = (None, None, None);
    while let Some(field) = mp.next_field().await.map_err(|e| multipart_error(e.status(), e.body_text()))? {
        let name = field.name().unwrap_or("").to_string();
        let bytes = field.bytes().await.map_err(|e| multipart_error(e.status(), e.body_text()))?;
        match name.as_str() {
            "url" => url = Some(String::from_utf8_lossy(&bytes).into_owned()),
            "title" => title = Some(String::from_utf8_lossy(&bytes).into_owned()),
            "snapshot" if bytes.len() > MAX_SNAPSHOT => {
                return Err(ApiError::bad_request("snapshot_too_large", "a snapshot is at most 8 MiB"));
            }
            "snapshot" => html = Some(bytes.to_vec()),
            _ => {}
        }
    }
    let pu = page_url(&s, url.as_deref().unwrap_or(""))?;
    let html = html.ok_or_else(|| ApiError::bad_request("invalid_args", "snapshot is required"))?;
    let title = clean_title(title.as_deref().unwrap_or(""), &pu.key.page_url());
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let done = s
        .store_call(move |st| {
            let Some(p) = st.find_live_page(&pu.key)? else { return Ok(None) };
            let id = ArtifactId::parse(&p.artifact_id)?;
            if !st.has_pending(&id)? {
                return Ok(None);
            }
            let (v, _) = st.store_snapshot(&id, &title, &html, true)?;
            let linked = st.link_pending(&id, v.n)?;
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            events.publish(Event::Version {
                artifact_id: a.id.clone(),
                n: v.n,
                by_page: false,
                title: Some(a.title.clone()),
                at: Some(v.created_at.clone()),
            });
            for tid in &linked {
                if let Some(t) = st.get_thread(tid)? {
                    super::threads::publish_thread(&ctx, st, &t)?;
                }
            }
            Ok(Some((p, a, v.n, linked)))
        })
        .await?;
    let Some((page, artifact, n, linked)) = done else {
        return Err(ApiError::new(StatusCode::CONFLICT, "nothing_pending", "the page has no address waiting for a snapshot"));
    };
    Ok(Json(json!({"page": page_view(&s, &page, &artifact), "version": n, "linked": linked})))
}
```

Register `.route("/api/live/snapshots", post(live::snapshot.layer(DefaultBodyLimit::max(live::LIVE_THREAD_LIMIT))))` in `api_slow`.

- [ ] **Step 5: The MCP tool and Pi**

In `crates/clax-mcp/src/tools.rs`, add to `CommentsReplyArgs`:

```rust
    /// On a live page: the page now shows the fix, so the thread is listed as
    /// addressed in the page's next snapshot. Not for artifacts (publish with
    /// `addresses` instead).
    pub addressed: Option<bool>,
```

pass `a.addressed.unwrap_or(false)` to `self.client.reply(&id, &a.thread_id, &a.text, addressed)`, whose body becomes `json!({"body": text, "author_kind": "agent", "addressed": addressed})`, and copy `res["addressed"]` into the tool's result when present. Append to the `comments_reply` tool's description: "On a live page, pass `addressed: true` once the page shows your fix." In `plugins/pi/src/clax.ts`, add `addressed: opt(bool("…the same description…"))` to `CommentsReplyArgs` and send it in the reply body; update `comments_reply`'s description in `plugins/pi/test/fixtures/contract.json`; add a Pi test that the reply body carries `addressed`. Run `python3 scripts/sync-skill-tools.py`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p clax-server --test api_live && cargo test -p clax-mcp && (cd plugins/pi && npm test -- --reporter=dot)`
Expected: PASS.

- [ ] **Step 7: Document and commit**

`docs/contract.md`: under "Live pages", the pending address, `addressed` on `comments_reply`, the agent-resolve rule, and `POST /api/live/snapshots`; in the tools table's `comments_reply` row, `addressed`.

```bash
git add crates plugins docs/contract.md
git -c commit.gpgsign=false commit -m "Address live-page threads in the page's next snapshot"
```

---

### Task 4: Scope watches, page URLs in the tools, and the live payload

**Files:**
- Create: `crates/clax-mcp/src/target.rs`
- Create: `crates/clax-server/tests/api_live_watch.rs`
- Create: `crates/clax-mcp/tests/live.rs`
- Modify: `crates/clax-core/src/store/live.rs` (scope watches and their materialization)
- Modify: `crates/clax-core/src/store/watches.rs` (a direct watch makes the row `direct`)
- Modify: `crates/clax-core/src/store/feedback.rs` (session end removes scope watches; feedback items of live pages carry `live`)
- Modify: `crates/clax-core/src/feedback.rs` (`FeedbackItem.live`, the live payload)
- Modify: `crates/clax-server/src/routes/watches.rs`, `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/feedback.rs` (`snapshot_path` beside `clip_path`)
- Modify: `crates/clax-mcp/src/lib.rs`, `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/src/client.rs`, `crates/clax-mcp/Cargo.toml` (`url.workspace = true`)
- Modify: `plugins/pi/src/clax.ts`, `plugins/pi/test/clax.test.ts`, `plugins/pi/test/fixtures/contract.json`
- Modify: `plugins/claude-code/skills/clax/SKILL.md`, `plugins/clax/skills/clax/SKILL.md`, `plugins/clax-grok/skills/clax/SKILL.md`, `plugins/pi/skills/clax/SKILL.md`
- Modify: `docs/contract.md`

**Interfaces:**
- Consumes: Task 1's `PageKey::covered_by`, `ensure_live_page`; Task 2's `routes::live::{page_url, page_view}`.
- Produces:
  - `clax_core::store::live::LiveWatch {session_id, origin, path, replies_armed, created_at}`
  - `Store::live_watch(sid: &str, scope: &PageKey, replies_armed: bool) -> Result<(LiveWatch, Vec<String>)>` (the covered artifact IDs)
  - `Store::live_unwatch(sid: &str, scope: &PageKey) -> Result<Vec<String>>` (the artifact IDs whose scope-made watch went)
  - `pub(super) fn live_page_of_conn(c: &Connection, aid: &str) -> Result<Option<LivePage>>` in `store/live.rs`
  - `EnsuredPage.scoped_sessions` filled when a page is created.
  - `PUT /api/sessions/<sid>/live-watches` `{url, replies_armed?}` → `{live_watch: {origin, path, scope, replies_armed}, page, covered}`; `DELETE /api/sessions/<sid>/live-watches?url=` → `{removed}`.
  - Thread views with the token carry `snapshot_path` (the thread's version's `index.html`) for live pages.
  - `clax_core::feedback::LiveRef {page_url: String, snapshot_path: String}`, `FeedbackItem.live: Option<LiveRef>`.
  - `clax_mcp::target::{Target::{Artifact {id, version}, Page(String)}, target(url_or_id: &str, daemon_base: &str) -> Result<Target, CallToolResult>}`.
  - `DaemonClient::{live_watch(&self, url: &str, replies: bool) -> Result<Value>, live_unwatch(&self, url: &str) -> Result<Value>, live_page(&self, url: &str) -> Result<Value>}`.

- [ ] **Step 1: Write the failing tests**

`crates/clax-server/tests/api_live_watch.rs`:

```rust
mod common;
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use serde_json::{Value, json};

async fn live_watch(ts: &TestServer, sid: &str, url: &str) -> Value {
    let res = ts
        .authed(ts.client.put(format!("{}/api/sessions/{sid}/live-watches", ts.base)))
        .json(&json!({"url": url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

async fn comment(ts: &TestServer, cookie: &str, url: &str) -> Value {
    let form = reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "P")
        .text("anchor", json!({"kind": "element", "selector": "body", "file": "index.html"}).to_string())
        .text("body", "Look")
        .text("snapshot", "<!doctype html><p>x")
        .part("clip", reqwest::multipart::Part::bytes(FAKE_PNG.to_vec()).mime_str("image/png").unwrap());
    ts.client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={cookie}"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn watches(ts: &TestServer, sid: &str) -> Vec<String> {
    let w: Value = ts.get_authed(&format!("/api/sessions/{sid}/watches")).await.json().await.unwrap();
    w["watches"].as_array().unwrap().iter().map(|w| w["artifact_id"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn scope_watch_covers_pages_created_later() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-1").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/").await;
    assert_eq!(w["live_watch"]["scope"], "http://localhost:5173/*");
    assert_eq!(w["page"]["current_version"], 1, "the root page exists with its placeholder");
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/settings").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap();
    assert!(watches(&ts, sid).await.contains(&aid.to_string()), "the new page is watched");
    let tid = c["thread"]["id"].as_str().unwrap();
    ts.send_thread(aid, tid).await;
    let fb: Value = ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=wait&wait=1")).await.json().await.unwrap();
    let text = fb["text"].as_str().unwrap();
    assert!(text.contains("live page http://localhost:5173/settings"), "{text}");
    assert!(text.contains("Snapshot: "), "{text}");
    assert!(text.contains("(snapshot v1)"), "{text}");
}

#[tokio::test]
async fn a_scope_on_a_path_covers_that_path_and_below_only() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-2").await;
    let sid = s["id"].as_str().unwrap();
    live_watch(&ts, sid, "http://localhost:5173/docs").await;
    let v = ts.viewer(Some("Alex")).await;
    let inside = comment(&ts, &v.cookie, "http://localhost:5173/docs/intro").await;
    let outside = comment(&ts, &v.cookie, "http://localhost:5173/settings").await;
    let w = watches(&ts, sid).await;
    assert!(w.contains(&inside["page"]["artifact_id"].as_str().unwrap().to_string()));
    assert!(!w.contains(&outside["page"]["artifact_id"].as_str().unwrap().to_string()));
}

#[tokio::test]
async fn removing_a_scope_keeps_direct_watches() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-3").await;
    let sid = s["id"].as_str().unwrap();
    let w = live_watch(&ts, sid, "http://localhost:5173/").await;
    let root = w["page"]["artifact_id"].as_str().unwrap().to_string();
    let v = ts.viewer(Some("Alex")).await;
    let other = comment(&ts, &v.cookie, "http://localhost:5173/x").await["page"]["artifact_id"].as_str().unwrap().to_string();
    ts.authed(ts.client.put(format!("{}/api/sessions/{sid}/watches/{other}", ts.base))).send().await.unwrap();
    let res = ts
        .authed(ts.client.delete(format!("{}/api/sessions/{sid}/live-watches?url=http%3A%2F%2Flocalhost%3A5173%2F", ts.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let left = watches(&ts, sid).await;
    assert!(!left.contains(&root));
    assert!(left.contains(&other), "the direct watch stays");
}

#[tokio::test]
async fn ending_the_session_ends_its_scope_watches() {
    let ts = TestServer::spawn().await;
    let s = ts.register_session("claude", "w-4").await;
    let sid = s["id"].as_str().unwrap().to_string();
    live_watch(&ts, &sid, "http://localhost:5173/").await;
    end_session(&ts, &sid).await;
    let s2 = ts.register_session("claude", "w-5").await;
    let v = ts.viewer(Some("Alex")).await;
    let c = comment(&ts, &v.cookie, "http://localhost:5173/y").await;
    let aid = c["page"]["artifact_id"].as_str().unwrap();
    assert!(!watches(&ts, s2["id"].as_str().unwrap()).await.contains(&aid.to_string()));
}
```

Add `end_session(&TestServer, &str)` to the file, sending the session-end `PATCH /api/sessions/<id>` exactly as `crates/clax-server/tests/api_sessions.rs` does.

In `crates/clax-mcp/src/target.rs`, the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str = "http://localhost:7480";

    fn art(s: &str) -> Option<(String, Option<u32>)> {
        match target(s, BASE).ok()? {
            Target::Artifact { id, version } => Some((id, version)),
            Target::Page(_) => None,
        }
    }
    fn page(s: &str) -> Option<String> {
        match target(s, BASE).ok()? {
            Target::Page(u) => Some(u),
            Target::Artifact { .. } => None,
        }
    }

    #[test]
    fn ids_and_the_daemons_urls_are_artifacts() {
        assert_eq!(art("7q3k9mzx2b4t"), Some(("7q3k9mzx2b4t".into(), None)));
        assert_eq!(art("http://localhost:7480/a/7q3k9mzx2b4t/v/3"), Some(("7q3k9mzx2b4t".into(), Some(3))));
        assert_eq!(art("http://127.0.0.1:7480/c/7q3k9mzx2b4t/v/2/x.html"), Some(("7q3k9mzx2b4t".into(), Some(2))));
        assert_eq!(art("http://192.168.1.20:7480/a/7q3k9mzx2b4t"), Some(("7q3k9mzx2b4t".into(), None)));
        assert_eq!(art("http://7q3k9mzx2b4t.localhost:7480/v/1/"), Some(("7q3k9mzx2b4t".into(), Some(1))));
        assert_eq!(art("localhost:7480/a/7q3k9mzx2b4t"), Some(("7q3k9mzx2b4t".into(), None)));
    }

    #[test]
    fn every_other_http_url_is_a_page() {
        assert_eq!(page("http://localhost:5173/"), Some("http://localhost:5173/".into()));
        assert_eq!(
            page("http://localhost:5173/a/7q3k9mzx2b4t"),
            Some("http://localhost:5173/a/7q3k9mzx2b4t".into()),
            "another port's /a/ path is a page"
        );
        assert_eq!(page("https://example.com/x?y#/z"), Some("https://example.com/x?y#/z".into()));
    }

    #[test]
    fn nonsense_is_invalid_id() {
        assert!(target("nope", BASE).is_err());
        assert!(target("ftp://localhost:7480/a/7q3k9mzx2b4t", BASE).is_err());
    }
}
```

`crates/clax-mcp/tests/live.rs`: with `session_tools` (copy it from `tests/comments.rs`), (1) call `tools.watch(Parameters(WatchArgs { url_or_id: "http://localhost:5173/".into(), on: None, replies: None }))` and assert the JSON's `page_url == "http://localhost:5173/"`, `scope == "http://localhost:5173/*"`, `watching == true`; (2) post a thread on `http://localhost:5173/settings` as a named viewer with `POST /api/live/threads` (the multipart of `api_live_watch.rs`) and send it; (3) call `wait_for_feedback` with `url_or_id: Some("http://localhost:5173/settings".into())` and `timeout_s: Some(5)` and assert its one item names the thread; (4) call `comments_read` with the page URL and assert `threads[0].page_url == "http://localhost:5173/settings"` and that `threads[0].snapshot_path` ends with `/versions/1/index.html`; (5) call `comments_read` with `http://localhost:5173/never` and assert the error code `invalid_id`; (6) call `watch` with the URL and `on: Some(false)` and assert `watching == false`.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p clax-server --test api_live_watch; cargo test -p clax-mcp`
Expected: FAIL: 404 on `/live-watches`; module `target` does not exist.

- [ ] **Step 3: Scope watches in the store**

Append to `crates/clax-core/src/store/live.rs`:

```rust
/// A session's scope watch (spec L2): it covers the live pages of `origin`
/// whose path is `path` or below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveWatch {
    pub session_id: String,
    pub origin: String,
    pub path: String,
    pub replies_armed: bool,
    pub created_at: String,
}

/// Makes `sid` a watcher of `aid` through a scope watch, unless it already
/// watches it (a direct watch keeps its own row and arming).
fn scope_row(tx: &Connection, sid: &str, aid: &str, armed: bool) -> Result<()> {
    tx.execute(
        "INSERT INTO watches (session_id, artifact_id, replies_armed, created_at, source)
         VALUES (?1, ?2, ?3, ?4, 'scope') ON CONFLICT(session_id, artifact_id) DO NOTHING",
        params![sid, aid, armed, Store::now()],
    )?;
    Ok(())
}

/// The live pages of `origin` (not deleted).
fn pages_of(tx: &Connection, origin: &str) -> Result<Vec<LivePage>> {
    let mut st = tx.prepare(
        "SELECT p.artifact_id, p.origin, p.path FROM live_pages p JOIN artifacts a ON a.id = p.artifact_id
         WHERE p.origin = ?1 AND a.deleted_at IS NULL",
    )?;
    let rows = st.query_map(params![origin], row_to_page)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The live page whose artifact is `aid`, on a given connection.
pub(super) fn live_page_of_conn(c: &Connection, aid: &str) -> Result<Option<LivePage>> {
    Ok(c.query_row(
        "SELECT artifact_id, origin, path FROM live_pages WHERE artifact_id = ?1",
        params![aid],
        row_to_page,
    )
    .optional()?)
}

/// Makes every live session whose scope watch covers `key` a watcher of the
/// new page `aid`; returns those sessions.
pub(super) fn materialize(tx: &Connection, aid: &str, key: &PageKey) -> Result<Vec<String>> {
    let rows: Vec<(String, String, bool)> = {
        let mut st = tx.prepare(
            "SELECT lw.session_id, lw.path, lw.replies_armed FROM live_watches lw
             JOIN sessions s ON s.id = lw.session_id
             WHERE lw.origin = ?1 AND s.ended_at IS NULL ORDER BY lw.created_at",
        )?;
        st.query_map(params![key.origin], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut out: Vec<String> = Vec::new();
    for (sid, path, armed) in rows {
        let scope = PageKey { origin: key.origin.clone(), path };
        if key.covered_by(&scope) && !out.contains(&sid) {
            scope_row(tx, &sid, aid, armed)?;
            out.push(sid);
        }
    }
    Ok(out)
}

impl Store {
    /// Creates or updates live session `sid`'s scope watch on `scope` and
    /// makes it a watcher of every live page the scope covers. Returns the
    /// watch and the covered pages' artifact IDs.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session.
    pub fn live_watch(&self, sid: &str, scope: &PageKey, replies_armed: bool) -> Result<(LiveWatch, Vec<String>)> {
        self.with_tx(|tx| {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND ended_at IS NULL)",
                params![sid],
                |r| r.get(0),
            )?;
            if !live {
                return Err(CoreError::invalid("unknown_session", format!("no live session {sid}")));
            }
            tx.execute(
                "INSERT INTO live_watches (session_id, origin, path, replies_armed, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id, origin, path) DO UPDATE SET replies_armed = excluded.replies_armed",
                params![sid, scope.origin, scope.path, replies_armed, Store::now()],
            )?;
            let covered: Vec<String> = pages_of(tx, &scope.origin)?
                .into_iter()
                .filter(|p| PageKey { origin: p.origin.clone(), path: p.path.clone() }.covered_by(scope))
                .map(|p| p.artifact_id)
                .collect();
            for aid in &covered {
                scope_row(tx, sid, aid, replies_armed)?;
            }
            let w = tx.query_row(
                "SELECT session_id, origin, path, replies_armed, created_at FROM live_watches
                 WHERE session_id = ?1 AND origin = ?2 AND path = ?3",
                params![sid, scope.origin, scope.path],
                |r| {
                    Ok(LiveWatch {
                        session_id: r.get(0)?,
                        origin: r.get(1)?,
                        path: r.get(2)?,
                        replies_armed: r.get::<_, i64>(3)? != 0,
                        created_at: r.get(4)?,
                    })
                },
            )?;
            Ok((w, covered))
        })
    }

    /// Removes `sid`'s scope watch on `scope` and the scope-made watches it
    /// alone justified (another scope of the session covering a page keeps
    /// its row; direct watches are never removed). Returns the pages unwatched.
    pub fn live_unwatch(&self, sid: &str, scope: &PageKey) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM live_watches WHERE session_id = ?1 AND origin = ?2 AND path = ?3",
                params![sid, scope.origin, scope.path],
            )?;
            let others: Vec<String> = {
                let mut st = tx.prepare("SELECT path FROM live_watches WHERE session_id = ?1 AND origin = ?2")?;
                st.query_map(params![sid, scope.origin], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            let mut removed = Vec::new();
            for p in pages_of(tx, &scope.origin)? {
                let key = PageKey { origin: p.origin.clone(), path: p.path.clone() };
                let kept = others.iter().any(|o| key.covered_by(&PageKey { origin: p.origin.clone(), path: o.clone() }));
                if key.covered_by(scope) && !kept {
                    let n = tx.execute(
                        "DELETE FROM watches WHERE session_id = ?1 AND artifact_id = ?2 AND source = 'scope'",
                        params![sid, p.artifact_id],
                    )?;
                    if n > 0 {
                        removed.push(p.artifact_id);
                    }
                }
            }
            Ok(removed)
        })
    }
}
```

In `ensure_live_page`'s creating transaction, after inserting the `live_pages` row, call `let scoped = materialize(tx, id.as_str(), key)?;`, return it with the ID (`Ok((id, true, scoped))`, and `Vec::new()` for an existing page), and put it in `EnsuredPage.scoped_sessions`. Add a core test:

```rust
    #[test]
    fn a_new_page_is_watched_by_the_scopes_that_cover_it() {
        let (_d, st) = store();
        let sid = st.register_session_for_test("claude");
        st.live_watch(&sid, &key("/"), true).unwrap();
        let e = st.ensure_live_page(&key("/new"), "n", None).unwrap();
        assert_eq!(e.scoped_sessions, vec![sid.clone()]);
        let w = st.list_watches(&sid).unwrap();
        assert!(w.iter().any(|w| w.artifact_id == e.artifact.id));
    }
```

(`register_session_for_test` exists in `store/sessions.rs` tests or add one that inserts a live `claude` session row and returns its ID.) In `Store::watch` (`store/watches.rs`), make the upsert `ON CONFLICT(session_id, artifact_id) DO UPDATE SET replies_armed = excluded.replies_armed, source = 'direct'`. In `store/feedback.rs`, beside `tx.execute("DELETE FROM watches WHERE session_id = ?1", params![sid])?;`, add `tx.execute("DELETE FROM live_watches WHERE session_id = ?1", params![sid])?;`.

- [ ] **Step 4: The routes**

Append to `crates/clax-server/src/routes/watches.rs`:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveWatchBody {
    url: String,
    replies_armed: Option<bool>,
}

/// The pages a scope watch on `origin` + `path` covers, as the tool shows it.
fn scope_label(origin: &str, path: &str) -> String {
    if path.ends_with('/') {
        format!("{origin}{path}*")
    } else {
        format!("{origin}{path} and {origin}{path}/*")
    }
}

/// `PUT /api/sessions/<sid>/live-watches` (W): a scope watch on the page
/// `url` names (spec L2). The page is created (with its placeholder) when
/// it does not exist; the session watches every live page the scope covers
/// and is handed their comments that were waiting untargeted.
pub async fn live_put(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    b: Result<Json<LiveWatchBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let b = super::artifacts::body(b)?;
    let pu = super::live::page_url(&s, &b.url)?;
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let live_ids = s.live_ids.clone();
    let (w, page, artifact, covered) = s
        .store_call(move |st| {
            let title = pu.key.page_url().split_once("://").map(|(_, r)| r.to_string()).unwrap_or_default();
            let e = st.ensure_live_page(&pu.key, &title, None)?;
            live_ids.insert(&e.artifact.id);
            if e.new_version {
                events.publish(clax_core::Event::Version {
                    artifact_id: e.artifact.id.clone(),
                    n: e.version.n,
                    by_page: false,
                    title: Some(e.artifact.title.clone()),
                    at: Some(e.version.created_at.clone()),
                });
            }
            let (w, covered) = st.live_watch(&sid, &pu.key, b.replies_armed.unwrap_or(true))?;
            for aid in &covered {
                let touched = st.retarget_untargeted(&clax_core::ArtifactId::parse(aid)?, &sid)?;
                apply(&ctx, st, &touched);
            }
            let id = clax_core::ArtifactId::parse(&e.artifact.id)?;
            let page = st.live_page_of(&id)?.ok_or(clax_core::CoreError::NotFound)?;
            Ok((w, page, e.artifact, covered))
        })
        .await?;
    Ok(Json(json!({
        "live_watch": {"origin": w.origin, "path": w.path, "scope": scope_label(&w.origin, &w.path), "replies_armed": w.replies_armed},
        "page": super::live::page_view(&s, &page, &artifact),
        "covered": covered,
    })))
}

#[derive(Deserialize)]
pub struct LiveUnwatchQuery {
    url: String,
}

/// `DELETE /api/sessions/<sid>/live-watches?url=` (W): removes the scope watch.
pub async fn live_delete(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    q: Result<axum::extract::Query<LiveUnwatchQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let axum::extract::Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = super::live::page_url(&s, &q.url)?;
    let removed = s.store_call(move |st| st.live_unwatch(&sid, &pu.key)).await?;
    Ok(Json(json!({"removed": removed})))
}
```

Register `.route("/api/sessions/{id}/live-watches", axum::routing::put(watches::live_put).delete(watches::live_delete))` in `api_fast`. In `crate::feedback::thread_view`, where `clip_path` is added for token holders, also add for a live page `view["snapshot_path"] = json!(st.home().version_dir(&id, t.version_n).join("index.html").display().to_string())`.

- [ ] **Step 5: The live payload**

In `crates/clax-core/src/feedback.rs`, add:

```rust
/// Where a comment on a live page was made, for the payload (spec 2026-10-05 §9.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveRef {
    /// The page's URL with the thread's route.
    pub page_url: String,
    /// The snapshot's `index.html` on disk.
    pub snapshot_path: String,
}
```

and `#[serde(default)] pub live: Option<LiveRef>,` on `FeedbackItem`. At the top of `render_item`, after `clip` is computed:

```rust
    if let Some(l) = &i.live {
        return format!(
            "[clax] Comment sent to you{resent} on {title} (live page {page}; Clax view {url}), thread {tid}\n\
             Anchored on: {anchor}  (snapshot v{v})\n\
             Clip: {clip}\n\
             Snapshot: {snapshot}\n\
             {author}{by_page}: {body}\n\
             Reply with comments_reply (addressed: true once the page shows the fix), then comments_resolve when done.",
            title = quoted(&i.artifact_title),
            page = one_line(&l.page_url),
            url = i.url,
            tid = i.thread_id,
            anchor = one_line(&i.anchor.summary()),
            v = i.version,
            snapshot = one_line(&l.snapshot_path),
            author = display_name(&i.author),
            by_page = if i.via_page { " (written by the page)" } else { "" },
            body = quoted(&i.body),
        );
    }
```

Add a unit test beside the existing payload tests:

```rust
    #[test]
    fn a_live_page_payload_names_the_page_and_the_snapshot() {
        let mut i = item();
        i.anchor.route = Some("?tab=billing".into());
        i.live = Some(LiveRef {
            page_url: "http://localhost:5173/settings?tab=billing\u{2028}x".into(),
            snapshot_path: "/h/.clax/artifacts/a/versions/3/index.html".into(),
        });
        let out = render_item(&i);
        assert!(out.contains("(live page http://localhost:5173/settings?tab=billing\\u2028x; Clax view "));
        assert!(out.contains("Anchored on: ?tab=billing › "));
        assert!(out.contains("Snapshot: /h/.clax/artifacts/a/versions/3/index.html"));
        assert_eq!(out.lines().count(), 6);
    }
```

(`item()` is the module's test item builder, or write one; it gets `live: None`.) Where feedback items are built from rows (`grep -n "FeedbackItem {" crates/clax-core/src/store/feedback.rs`), fill `live`:

```rust
            live: match super::live::live_page_of_conn(c, &artifact_id)? {
                Some(p) => Some(crate::feedback::LiveRef {
                    page_url: format!("{}{}{}", p.origin, p.path, anchor.route.as_deref().unwrap_or("")),
                    snapshot_path: self
                        .home
                        .version_dir(&ArtifactId::parse(&artifact_id)?, version)
                        .join("index.html")
                        .display()
                        .to_string(),
                }),
                None => None,
            },
```

(adapt `c`, `artifact_id`, `version` and `anchor` to the builder's own names).

- [ ] **Step 6: Targets in the MCP tools**

Above the tests in `crates/clax-mcp/src/target.rs`:

```rust
//! What a tool's `url_or_id` names (spec 2026-10-05 §6.2): an artifact (its
//! ID, or a URL of this daemon's) or a page (any other http(s) URL).

use crate::render;
use clax_core::ArtifactId;
use rmcp::model::CallToolResult;
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Artifact { id: String, version: Option<u32> },
    Page(String),
}

fn invalid(s: &str) -> CallToolResult {
    render::error(
        "invalid_id",
        format!("'{s}' is not an artifact ID, a Clax URL, or an http(s) page URL"),
        json!({}),
    )
}

/// The artifact in a Clax URL: `/a/<id>[/v/<n>]…`, `/c/<id>/v/<n>/…`, or a
/// `<id>.localhost` host with `/v/<n>/…`.
fn artifact_in(host: &str, path: &str) -> Option<(String, Option<u32>)> {
    let parse = |c: &str| ArtifactId::parse(c).ok().map(|id| id.as_str().to_string());
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let version_in = |segs: &[&str]| match segs {
        ["v", n, ..] => n.parse::<u32>().ok(),
        _ => None,
    };
    let hostname = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if let Some(id) = hostname.strip_suffix(".localhost").and_then(parse) {
        return Some((id, version_in(&segs)));
    }
    for (i, seg) in segs.iter().enumerate() {
        if matches!(*seg, "a" | "c")
            && let Some(id) = segs.get(i + 1).and_then(|c| parse(c))
        {
            return Some((id, version_in(&segs[i + 2..])));
        }
    }
    None
}

/// What `url_or_id` names. An http(s) URL on the daemon's port is an
/// artifact reference; any other http(s) URL is a page. Text without a
/// scheme is read as before (an ID, or a daemon path such as
/// `localhost:7480/a/<id>`).
pub fn target(url_or_id: &str, daemon_base: &str) -> Result<Target, CallToolResult> {
    let s = url_or_id.trim();
    if let Ok(id) = ArtifactId::parse(s) {
        return Ok(Target::Artifact { id: id.as_str().to_string(), version: None });
    }
    if s.contains("://") {
        let u = url::Url::parse(s).map_err(|_| invalid(s))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(invalid(s));
        }
        let daemon_port = url::Url::parse(daemon_base).ok().and_then(|d| d.port_or_known_default());
        if u.port_or_known_default() == daemon_port {
            let host = u.host_str().unwrap_or("");
            return artifact_in(host, u.path())
                .map(|(id, version)| Target::Artifact { id, version })
                .ok_or_else(|| invalid(s));
        }
        return Ok(Target::Page(s.to_string()));
    }
    let bare = s.split(['?', '#']).next().unwrap_or("");
    let (host, path) = bare.split_once('/').unwrap_or((bare, ""));
    artifact_in(host, path)
        .map(|(id, version)| Target::Artifact { id, version })
        .ok_or_else(|| invalid(s))
}
```

Add `pub mod target;` to `crates/clax-mcp/src/lib.rs`. In `tools.rs`, delete `artifact_ref` and `artifact_id`, and add methods:

```rust
    /// The artifact `url_or_id` names, a page URL resolved to its live page.
    async fn artifact_ref(&self, url_or_id: &str) -> Result<(String, Option<u32>), CallToolResult> {
        match crate::target::target(url_or_id, &self.browser_base())? {
            crate::target::Target::Artifact { id, version } => Ok((id, version)),
            crate::target::Target::Page(url) => {
                let res = self.client.live_page(&url).await.map_err(|e| self.fail(e))?;
                match res["page"]["artifact_id"].as_str() {
                    Some(id) => Ok((id.to_string(), None)),
                    None => Err(render::error(
                        "invalid_id",
                        format!("no live page at {url} yet: watch it, or comment on it in Chrome with the Clax extension first"),
                        json!({}),
                    )),
                }
            }
        }
    }

    async fn artifact_id(&self, url_or_id: &str) -> Result<String, CallToolResult> {
        self.artifact_ref(url_or_id).await.map(|(id, _)| id)
    }
```

and change every `artifact_id(&a.url_or_id)?` call to `self.artifact_id(&a.url_or_id).await?` (and `artifact_ref` likewise). `do_watch` branches on the target:

```rust
    async fn do_watch(&self, a: WatchArgs) -> Outcome {
        let url = match crate::target::target(&a.url_or_id, &self.browser_base())? {
            crate::target::Target::Page(url) => url,
            crate::target::Target::Artifact { .. } => return self.do_watch_artifact(a).await,
        };
        self.require_session().await?;
        if a.on.unwrap_or(true) {
            let res = self.client.live_watch(&url, a.replies.unwrap_or(true)).await.map_err(|e| self.fail(e))?;
            Ok(json!({
                "artifact_id": res["page"]["artifact_id"],
                "url": res["page"]["url"],
                "page_url": res["page"]["page_url"],
                "scope": res["live_watch"]["scope"],
                "watching": true,
                "replies_armed": res["live_watch"]["replies_armed"],
            }))
        } else {
            self.client.live_unwatch(&url).await.map_err(|e| self.fail(e))?;
            Ok(json!({"page_url": url, "watching": false, "replies_armed": false}))
        }
    }
```

(`do_watch_artifact` is today's `do_watch` body.) In `comments_read`'s per-thread view, copy `page_url` (from the artifact's `live.page_url` plus the anchor's `route`) and `snapshot_path` (from the thread view) for live pages. In `client.rs`:

```rust
    /// `PUT /api/sessions/<sid>/live-watches`: a scope watch on the page `url`.
    pub async fn live_watch(&self, url: &str, replies: bool) -> Result<Value> {
        let body = json!({"url": url, "replies_armed": replies});
        self.json(|c| c.request(reqwest::Method::PUT, &format!("{}/live-watches", c.session_path())).json(&body))
            .await
    }

    /// `DELETE /api/sessions/<sid>/live-watches?url=`.
    pub async fn live_unwatch(&self, url: &str) -> Result<Value> {
        self.json(|c| {
            c.request(reqwest::Method::DELETE, &format!("{}/live-watches", c.session_path()))
                .query(&[("url", url)])
        })
        .await
    }

    /// `GET /api/live/pages?url=`: `{page, route}`.
    pub async fn live_page(&self, url: &str) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::GET, "/api/live/pages").query(&[("url", url)])).await
    }
```

Update the `watch` tool's description: "Watch an artifact, or a web page by its URL (your dev server's, such as `http://localhost:5173/`, which covers every page under it), so comments sent to the agent on it reach this session (`on`, default true; `on: false` stops). …". Make the same changes in `plugins/pi/src/clax.ts`: a `target()` with the same rules as Rust's (export it), the live-watch and page-lookup calls, the new `watch` description (also in `plugins/pi/test/fixtures/contract.json`), and tests in `plugins/pi/test/clax.test.ts` for `target()`'s cases above and for `watch` with a URL against the fake API.

- [ ] **Step 7: Skill text**

Add the "Live pages" section of spec §15 to each of the four `skills/clax/SKILL.md` (tools named as each skill names them: `watch` and `comments_reply` in Claude Code's, `mcp__clax__watch` in Codex's, `clax_grok__watch` through `use_tool` in Grok's, `clax_watch` in Pi's), right after the `wait_for_feedback` bullet, and extend the `watch` bullet with "or a page URL (your dev server's), covering every page under it". Run `python3 scripts/sync-skill-tools.py` and `scripts/test-plugins.sh`.

- [ ] **Step 8: Run the tests**

Run: `cargo test -p clax-core && cargo test -p clax-server --test api_live_watch --test api_live --test api_watches && cargo test -p clax-mcp && (cd plugins/pi && npm test -- --reporter=dot) && scripts/test-plugins.sh`
Expected: PASS.

- [ ] **Step 9: Document and commit**

`docs/contract.md`: the `watch` row (`url_or_id` may be a page URL; the result's `page_url` and `scope`), "Watch semantics" for scope watches, the live payload under "Payload", `page_url` and `snapshot_path` in `comments_read`, and the rule that only URLs on the daemon's port are Clax URLs.

```bash
git add crates plugins docs/contract.md
git -c commit.gpgsign=false commit -m "Watch a page by its URL: scope watches, page targets in the tools, and the live payload"
```

---

### Task 5: The extension's identity and credentials

**Files:**
- Create: `web/extension/manifest.json`
- Create: `crates/clax-core/src/extension.rs`
- Create: `crates/clax-core/src/store/extension.rs`
- Create: `crates/clax-server/src/extension.rs` (the credential cache; Task 6 adds the gateway)
- Create: `crates/clax-server/src/routes/extension.rs`
- Create: `crates/clax-server/tests/api_extension.rs`
- Modify: `crates/clax-core/src/store/migrations.rs` (migration 16), `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/lib.rs`, `crates/clax-core/Cargo.toml` (`sha2 = "0.11"`)
- Modify: `crates/clax-server/src/state.rs`, `daemon.rs`, `testing.rs`, `lib.rs`, `routes/mod.rs`

**Interfaces:**
- Consumes:
  - Optionally, the committed public key `web/extension/key/key.pub.b64` (one line of base64 SubjectPublicKeyInfo DER, spec L15). Absent until the owner runs Task 17's `scripts/extension-pubkey.sh`; nothing here waits for it.
  - From the owner identity on main (Global Constraints, "Owner identity"; placeholder names): `owner_viewer(&Store) -> Result<Viewer>`; `TestServer::owner_viewer()` (added here over `owner_viewer` if main has none).
- Produces:
  - `clax_core::extension::{MANIFEST: &str, PUBLIC_KEY: Option<&str> (include_str of `web/extension/key/key.pub.b64` via build.rs when present), extension_id_from_key(&str) -> Option<String>, extension_id_from_path(&Path) -> String, extension_id_in_effect(home: &Path) -> String, extension_origin(id: &str) -> String, HOST_NAME: &str = "dev.empathic.clax", CREDENTIAL_PREFIX: &str = "cxe_", CREDENTIAL_TTL_DAYS: i64 = 30, MAX_CREDENTIALS: usize = 8, new_credential() -> String, credential_hash(&str) -> String, is_credential(&str) -> bool}`
  - `clax_core::store::extension::{MintedCredential {credential, hash}, LiveCredential {hash, extension_id, last_used_at}}`
  - `Store::{mint_extension_credential(&str) -> Result<MintedCredential>, live_extension_credentials() -> Result<Vec<LiveCredential>>, touch_extension_credential(hash: &str) -> Result<()>, revoke_extension_credentials() -> Result<usize>}`
  - `clax_server::extension::{Credentials, Cred {extension_id}}` with `Credentials::{load(&Store) -> Result<Credentials>, get(&self, hash) -> Option<Cred>, insert(&self, hash, Cred), replace_with(&self, Credentials), due_for_touch(&self, hash) -> bool}`
  - `AppState.ext_creds: Arc<Credentials>`, `AppState.extension_id: String` (the ID in effect for the daemon's home); `TestServer::extension_id() -> String`
  - `POST /api/extension/credentials` (W) `{extension_id}` → `{credential, viewer, expires_in_s}`; `GET /api/extension` (W) → `{extension_id, live_credentials, last_used_at, viewer}`; in both, `viewer` is the owner viewer (`owner_viewer`), serialized as `Viewer` is (never its cookie value). `DELETE /api/extension/credentials` (W) → `{revoked}`.

> **Amendment (owner decision 2026-10-05, spec L15).** There is no production key in this task and nothing waits on the owner. Replace the `EXTENSION_ID` constant everywhere in this task's code with the per-home ID in effect (Global Constraints): `extension_id_from_path` implements Chromium's unpacked rule, `extension_id_from_key` the key rule, and `extension_id_in_effect(home)` picks the key's ID when `PUBLIC_KEY` is `Some`, else the path's. Pin both derivations with tests: path `/Users/alex/.clax/extension` → `bhhldgpcjhfhmcfjjnelbbdcefnocaln`; key bytes `000000` (base64 `AAAA`) → `hajoiamiieihkcebbobooenpljpcckig`. Canonicalize the path (resolve symlinks, e.g. macOS `/var` → `/private/var`) before hashing, and say in a doc comment that Chromium hashes the path it loaded. `POST /api/extension/credentials` refuses any ID but `state.extension_id`; the native host and `clax extension install` use `extension_id_in_effect(home)`.

- [ ] **Step 1: The manifest, without a key**

Create `web/extension/manifest.json` (no `key`; Task 8's build adds `"key"` from `web/extension/key/key.pub.b64` only when that file exists):

```json
{
  "manifest_version": 3,
  "name": "Clax",
  "version": "0.0.0",
  "description": "Comment on any page and send your comments to your coding agent.",
  "minimum_chrome_version": "116",
  "action": {"default_title": "Comment with Clax", "default_icon": {"16": "icons/16.png", "32": "icons/32.png"}},
  "icons": {"16": "icons/16.png", "32": "icons/32.png", "48": "icons/48.png", "128": "icons/128.png"},
  "background": {"service_worker": "sw.js", "type": "module"},
  "side_panel": {"default_path": "sidepanel.html"},
  "permissions": ["activeTab", "scripting", "sidePanel", "storage", "nativeMessaging", "contextMenus"],
  "optional_host_permissions": ["http://*/*", "https://*/*"],
  "commands": {"comment": {"suggested_key": {"default": "Alt+Shift+C"}, "description": "Comment on this page"}},
  "web_accessible_resources": [{"resources": ["composer.html"], "matches": ["http://*/*", "https://*/*"], "use_dynamic_url": true}],
  "content_security_policy": {"extension_pages": "script-src 'self'; object-src 'none'; connect-src 'self' http://localhost:* http://127.0.0.1:*; img-src 'self' blob: data: http://localhost:* http://127.0.0.1:*"}
}
```

(`version` is replaced by Task 8's build. Unpacked, this manifest's ID is the one Chromium derives from the install path, which `extension_id_in_effect` computes.)

- [ ] **Step 2: Write the failing tests**

`crates/clax-core/src/extension.rs`, test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_the_manifest_keys() {
        let m: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
        let key = m["key"].as_str().unwrap();
        assert_eq!(extension_id_from_key(key).as_deref(), Some(EXTENSION_ID));
        assert_eq!(EXTENSION_ID.len(), 32);
        assert!(EXTENSION_ID.bytes().all(|b| (b'a'..=b'p').contains(&b)));
    }

    #[test]
    fn credentials_have_their_shape_and_hash_without_revealing_themselves() {
        let c = new_credential();
        assert!(is_credential(&c), "{c}");
        assert_eq!(c.len(), 4 + 43);
        assert_ne!(new_credential(), c);
        let h = credential_hash(&c);
        assert_eq!(h.len(), 64);
        assert!(!h.contains(&c[4..]));
        assert!(!is_credential("cxe_short"));
        assert!(!is_credential("Bearer x"));
    }
}
```

`crates/clax-core/src/store/extension.rs`, test module first:

```rust
#[cfg(test)]
mod tests {
    use crate::extension::{EXTENSION_ID, MAX_CREDENTIALS, credential_hash};
    use crate::{Home, Store};

    #[test]
    fn minting_past_the_cap_revokes_the_oldest_and_no_credential_carries_a_viewer() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let viewers = || -> i64 { st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM viewers", [], |r| r.get(0))?)).unwrap() };
        let before = viewers();
        let first = st.mint_extension_credential(EXTENSION_ID).unwrap();
        assert_eq!(first.hash, credential_hash(&first.credential));
        let mut last = first.clone();
        for _ in 0..MAX_CREDENTIALS {
            last = st.mint_extension_credential(EXTENSION_ID).unwrap();
        }
        assert_eq!(viewers(), before, "minting creates no viewer: the extension is the owner (spec L6)");
        let live = st.live_extension_credentials().unwrap();
        assert_eq!(live.len(), MAX_CREDENTIALS);
        assert!(!live.iter().any(|c| c.hash == first.hash), "the oldest was revoked");
        assert!(live.iter().any(|c| c.hash == last.hash));
        assert_eq!(st.revoke_extension_credentials().unwrap(), MAX_CREDENTIALS);
        assert!(st.live_extension_credentials().unwrap().is_empty());
    }
}
```

`crates/clax-server/tests/api_extension.rs`:

```rust
mod common;
use clax_core::extension::EXTENSION_ID;
use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn minting_needs_the_token_and_the_known_extension() {
    let ts = TestServer::spawn().await;
    let url = format!("{}/api/extension/credentials", ts.base);
    let body = json!({"extension_id": EXTENSION_ID});
    assert_eq!(ts.client.post(&url).json(&body).send().await.unwrap().status(), 401);
    let res = ts.authed(ts.client.post(&url)).json(&json!({"extension_id": "a".repeat(32)})).send().await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "unknown_extension");
    let res = ts.authed(ts.client.post(&url)).json(&body).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let cred = v["credential"].as_str().unwrap().to_string();
    assert!(cred.starts_with("cxe_"));
    let owner = ts.owner_viewer().await;
    assert_eq!(v["viewer"]["public_id"], owner.public_id.as_str(), "the extension pairs as the owner");
    assert!(v["viewer"].get("id").is_none(), "never the owner's cookie value");
    assert_eq!(v["expires_in_s"], 30 * 86_400);
    let st: Value = ts.get_authed("/api/extension").await.json().await.unwrap();
    assert_eq!(st["live_credentials"], 1);
    assert_eq!(st["extension_id"], EXTENSION_ID);
    assert_eq!(st["viewer"]["public_id"], owner.public_id.as_str());
    assert!(!st.to_string().contains(&cred), "status never shows a credential");
    let r: Value = ts.authed(ts.client.delete(&url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["revoked"], 1);
}
```

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo test -p clax-core --lib extension; cargo test -p clax-server --test api_extension`
Expected: FAIL to compile: no module `extension`.

- [ ] **Step 4: Migration 16 and the core**

Append to `MIGRATIONS`:

```rust
    // 16: the Clax Chrome extension (spec 2026-10-05-chrome-overlay-design
    // §5.3): its credentials as hashes. A credential names no viewer: the
    // extension acts as the owner identity.
    "CREATE TABLE extension_credentials (
        id TEXT PRIMARY KEY,
        extension_id TEXT NOT NULL,
        secret_sha256 TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        last_used_at TEXT NOT NULL,
        revoked_at TEXT
    );
    CREATE INDEX extension_credentials_by_extension ON extension_credentials(extension_id, created_at);",
```

`crates/clax-core/src/extension.rs`, above the tests:

```rust
//! The Clax Chrome extension's identity and credentials (spec
//! 2026-10-05-chrome-overlay-design §5.3, §9). The extension ID is fixed by
//! the `key` in its manifest; the daemon admits that origin only.

use base64::Engine as _;
use rand::RngCore as _;
use sha2::{Digest, Sha256};

/// The extension's manifest, as the repository holds it.
pub const MANIFEST: &str = include_str!("../../../web/extension/manifest.json");
/// The committed public key (`web/extension/key/key.pub.b64`), when there is one;
/// `build.rs` sets `CLAX_EXTENSION_PUBLIC_KEY` from it.
pub const PUBLIC_KEY: Option<&str> = option_env!("CLAX_EXTENSION_PUBLIC_KEY");
/// The native messaging host's name.
pub const HOST_NAME: &str = "dev.empathic.clax";
/// Every credential starts with this.
pub const CREDENTIAL_PREFIX: &str = "cxe_";
/// A credential unused for this long is no longer accepted.
pub const CREDENTIAL_TTL_DAYS: i64 = 30;
/// Live credentials kept per extension ID; minting past it revokes the oldest.
pub const MAX_CREDENTIALS: usize = 8;

/// `chrome-extension://<id>`, the `Origin` the extension's requests carry.
pub fn extension_origin(id: &str) -> String {
    format!("chrome-extension://{id}")
}

/// Chromium's ID for an unpacked extension loaded from `dir` with no `key`:
/// the first 128 bits of the SHA-256 of the canonicalized path's bytes, each
/// nibble written `a`–`p`. Chromium hashes the path it loaded, so `dir` is
/// canonicalized first (symlinks resolved).
pub fn extension_id_from_path(dir: &std::path::Path) -> String {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    nibbles_a_to_p(&Sha256::digest(dir.as_os_str().as_encoded_bytes()))
}

/// The ID in effect for the Clax home `home`: the committed public key's,
/// else the one Chromium gives `<home>/extension` unpacked.
pub fn extension_id_in_effect(home: &std::path::Path) -> String {
    PUBLIC_KEY
        .and_then(extension_id_from_key)
        .unwrap_or_else(|| extension_id_from_path(&home.join("extension")))
}

/// Chrome's extension ID for a manifest `key` (base64 SubjectPublicKeyInfo):
/// the first 32 hex digits of the key's SHA-256, each digit `0`–`f` written
/// as the letter `a`–`p`.
pub fn extension_id_from_key(key_b64: &str) -> Option<String> {
    let der = base64::engine::general_purpose::STANDARD.decode(key_b64.trim()).ok()?;
    let digest = Sha256::digest(&der);
    Some(digest[..16].iter().flat_map(|b| [b >> 4, b & 0xf]).map(|n| char::from(b'a' + n)).collect())
}

/// A new credential: the prefix and 32 random bytes in unpadded base64url.
pub fn new_credential() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{CREDENTIAL_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// A credential's SHA-256 in lowercase hex: what the store and the daemon keep.
pub fn credential_hash(c: &str) -> String {
    Sha256::digest(c.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `c` has a credential's shape (the prefix and 43 base64url characters).
pub fn is_credential(c: &str) -> bool {
    c.strip_prefix(CREDENTIAL_PREFIX)
        .is_some_and(|r| r.len() == 43 && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}
```

`nibbles_a_to_p` is the helper `extension_id_from_key` already uses (factor it out). Replace the test `the_id_is_the_manifest_keys` with the two pinned vectors from this task's amendment (path and `AAAA` key), plus a test that `extension_id_in_effect` uses the path rule when `PUBLIC_KEY` is `None`. Use the `rand` API of the version the crate depends on (`rand::rng()` is 0.9; `rand::thread_rng()` is 0.8).

`crates/clax-core/src/store/extension.rs`, above the tests:

```rust
//! The extension's credentials (spec 2026-10-05 §5.3). A credential names
//! no viewer: every live one acts as the owner identity (spec L6).

use super::Store;
use crate::extension::{CREDENTIAL_TTL_DAYS, MAX_CREDENTIALS, credential_hash, new_credential};
use crate::{Result, new_ulid};
use rusqlite::params;

#[derive(Clone, Debug)]
pub struct MintedCredential {
    /// The credential itself: handed to the extension once, never stored.
    pub credential: String,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveCredential {
    pub hash: String,
    pub extension_id: String,
    pub last_used_at: String,
}

/// The oldest `last_used_at` still live.
fn cutoff() -> String {
    (chrono::Utc::now() - chrono::Duration::days(CREDENTIAL_TTL_DAYS)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Store {
    /// Mints a credential for `extension_id`; past [`MAX_CREDENTIALS`] live
    /// ones, the oldest are revoked.
    pub fn mint_extension_credential(&self, extension_id: &str) -> Result<MintedCredential> {
        let credential = new_credential();
        let hash = credential_hash(&credential);
        self.with_tx(|tx| {
            let now = Store::now();
            tx.execute(
                "INSERT INTO extension_credentials (id, extension_id, secret_sha256, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![new_ulid(), extension_id, hash, now],
            )?;
            tx.execute(
                "UPDATE extension_credentials SET revoked_at = ?3
                 WHERE extension_id = ?1 AND revoked_at IS NULL AND id NOT IN (
                    SELECT id FROM extension_credentials WHERE extension_id = ?1 AND revoked_at IS NULL
                    ORDER BY created_at DESC, id DESC LIMIT ?2)",
                params![extension_id, MAX_CREDENTIALS as i64, now],
            )?;
            Ok(())
        })?;
        Ok(MintedCredential { credential, hash })
    }

    /// Every credential neither revoked nor unused for [`CREDENTIAL_TTL_DAYS`].
    pub fn live_extension_credentials(&self) -> Result<Vec<LiveCredential>> {
        self.with_read(|c| {
            let mut st = c.prepare(
                "SELECT secret_sha256, extension_id, last_used_at FROM extension_credentials
                 WHERE revoked_at IS NULL AND last_used_at >= ?1 ORDER BY created_at",
            )?;
            let rows = st
                .query_map(params![cutoff()], |r| {
                    Ok(LiveCredential { hash: r.get(0)?, extension_id: r.get(1)?, last_used_at: r.get(2)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Records a use of the credential whose hash is `hash`.
    pub fn touch_extension_credential(&self, hash: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE extension_credentials SET last_used_at = ?2 WHERE secret_sha256 = ?1",
                params![hash, Store::now()],
            )?;
            Ok(())
        })
    }

    /// Revokes every live credential; returns how many.
    pub fn revoke_extension_credentials(&self) -> Result<usize> {
        self.with_tx(|tx| {
            Ok(tx.execute("UPDATE extension_credentials SET revoked_at = ?1 WHERE revoked_at IS NULL", params![Store::now()])?)
        })
    }
}
```

Add `pub mod extension;` to `crates/clax-core/src/lib.rs` and `store/mod.rs`, and `sha2 = "0.11"` to clax-core's dependencies.

- [ ] **Step 5: The server's credential cache and routes**

`crates/clax-server/src/extension.rs`:

```rust
//! The extension's credentials on the daemon (spec 2026-10-05 §5.3, §10):
//! an in-memory map of live credential hashes, loaded on start and replaced
//! after every mint and revoke, so authenticating a request reads no store.
//! A live credential is the owner identity (spec L6); it names no viewer.

use clax_core::Store;
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

/// How often one credential's `last_used_at` is written.
const TOUCH_EVERY: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cred {
    pub extension_id: String,
}

#[derive(Default)]
pub struct Credentials {
    map: RwLock<HashMap<String, Cred>>,
    touched: Mutex<HashMap<String, Instant>>,
}

impl Credentials {
    /// The live credentials the store holds now.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<Credentials> {
        let c = Credentials::default();
        for k in st.live_extension_credentials()? {
            c.insert(&k.hash, Cred { extension_id: k.extension_id });
        }
        Ok(c)
    }

    pub fn get(&self, hash: &str) -> Option<Cred> {
        self.map.read().expect("credentials lock").get(hash).cloned()
    }

    pub fn insert(&self, hash: &str, c: Cred) {
        self.map.write().expect("credentials lock").insert(hash.to_string(), c);
    }

    /// Takes `other`'s credentials in place of these (after a mint, which
    /// may revoke the oldest, and after a revoke).
    pub fn replace_with(&self, other: Credentials) {
        *self.map.write().expect("credentials lock") = other.map.into_inner().expect("credentials lock");
    }

    /// Whether the credential's use should be written now (at most hourly).
    pub fn due_for_touch(&self, hash: &str) -> bool {
        let mut t = self.touched.lock().expect("touch lock");
        let now = Instant::now();
        match t.get(hash) {
            Some(at) if now.duration_since(*at) < TOUCH_EVERY => false,
            _ => {
                t.insert(hash.to_string(), now);
                true
            }
        }
    }
}
```

`crates/clax-server/src/routes/extension.rs`:

```rust
//! `/api/extension` (spec 2026-10-05 §9.2): minting, reporting and revoking
//! the extension's credentials. Token only: the native host mints with the
//! token it reads from `daemon.json`. The `viewer` these answer is the owner
//! viewer, which every credential acts as (spec L6).

use super::artifacts::body;
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::extension::Credentials;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use clax_core::extension::{CREDENTIAL_TTL_DAYS, EXTENSION_ID};
// Placeholder path: the owner identity's resolver as main names it.
use clax_core::owner::owner_viewer;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintBody {
    extension_id: String,
}

/// `POST /api/extension/credentials` (W): a new credential for the Clax extension.
pub async fn mint(
    State(s): State<AppState>,
    _t: RequireToken,
    b: Result<Json<MintBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(b)?;
    if b.extension_id != EXTENSION_ID {
        return Err(ApiError::bad_request("unknown_extension", "only the Clax extension can pair"));
    }
    let creds = s.ext_creds.clone();
    let (m, owner) = s
        .store_call(move |st| {
            let m = st.mint_extension_credential(EXTENSION_ID)?;
            creds.replace_with(Credentials::load(st)?);
            Ok((m, owner_viewer(st)?))
        })
        .await?;
    Ok(Json(json!({"credential": m.credential, "viewer": owner, "expires_in_s": CREDENTIAL_TTL_DAYS * 86_400})))
}

/// `GET /api/extension` (W): how many credentials are live and when one was last used.
pub async fn status(State(s): State<AppState>, _t: RequireToken) -> Result<Json<Value>, ApiError> {
    let (live, viewer) = s
        .store_call(|st| Ok((st.live_extension_credentials()?, owner_viewer(st)?)))
        .await?;
    Ok(Json(json!({
        "extension_id": EXTENSION_ID,
        "live_credentials": live.len(),
        "last_used_at": live.iter().map(|c| c.last_used_at.clone()).max(),
        "viewer": viewer,
    })))
}

/// `DELETE /api/extension/credentials` (W): revokes every credential.
pub async fn revoke(State(s): State<AppState>, _t: RequireToken) -> Result<Json<Value>, ApiError> {
    let creds = s.ext_creds.clone();
    let n = s
        .store_call(move |st| {
            let n = st.revoke_extension_credentials()?;
            creds.replace_with(Credentials::load(st)?);
            Ok(n)
        })
        .await?;
    Ok(Json(json!({"revoked": n})))
}
```

If main has no test access to the owner viewer, add to `TestServer` in `testing.rs`: `pub async fn owner_viewer(&self) -> clax_core::model::Viewer`, opening the store at `self.home` and calling `owner_viewer` (in `spawn_blocking`). Add `pub ext_creds: Arc<crate::extension::Credentials>` to `AppState` (built with `Credentials::load(&store)?` in `daemon.rs` and `testing.rs`), `pub mod extension;` to `crates/clax-server/src/lib.rs` and `routes/mod.rs`, and in `api_fast`: `.route("/api/extension", get(extension::status)).route("/api/extension/credentials", post(extension::mint).delete(extension::revoke))`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p clax-core && cargo test -p clax-server --test api_extension`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add web/extension/manifest.json crates
git -c commit.gpgsign=false commit -m "Give the Chrome extension a fixed ID and daemon credentials"
```

---

### Task 6: The extension gateway

**Files:**
- Modify: `crates/clax-server/src/extension.rs` (the gateway)
- Modify: `crates/clax-server/src/routes/mod.rs` (the layer)
- Modify: `crates/clax-server/src/stream.rs`, `crates/clax-server/src/routes/stream.rs` (live-only streams)
- Modify: `crates/clax-server/src/testing.rs` (`EventReader::from_response`, if missing)
- Create: `crates/clax-server/tests/api_gateway.rs`
- Modify: `docs/contract.md` ("Security model")

**Interfaces:**
- Consumes: Task 5's `Credentials`, `credential_hash`, `is_credential`, `extension_origin`, `TestServer::owner_viewer`; Task 2's `LiveIds`; from the owner identity on main (placeholder names): `owner_viewer(&Store) -> Result<Viewer>` and **the owner hook**, the one place mapping a credential kind to the owner identity, where this task registers the extension credential.
- Produces:
  - `clax_server::extension::{gateway (middleware), ViaExtension (unit marker), SCHEME: &str = "Clax-Extension"}`.
  - A request the gateway admits reaches the existing handler as the **owner identity**, through the owner hook, with no `Origin`, `Sec-Fetch-Site`, `Cookie` or `Authorization` of its own; every response to the extension's origin carries `Access-Control-Allow-Origin: chrome-extension://<ID>` and `Vary: Origin`.
  - `Hub::open(caller, local, live_only, resume)`; a live-only stream refuses `gallery`, `docs:*` and other artifacts' topics with `SubError::Forbidden` (403 `forbidden`).

- [ ] **Step 1: Write the failing tests**

`crates/clax-server/tests/api_gateway.rs`:

```rust
mod common;
use clax_core::extension::{EXTENSION_ID, extension_origin};
use clax_server::testing::FAKE_PNG;
use common::TestServer;
use reqwest::Method;
use serde_json::{Value, json};

async fn credential(ts: &TestServer) -> String {
    let v: Value = ts
        .authed(ts.client.post(format!("{}/api/extension/credentials", ts.base)))
        .json(&json!({"extension_id": EXTENSION_ID}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["credential"].as_str().unwrap().to_string()
}

fn ext(ts: &TestServer, m: Method, path: &str, cred: &str) -> reqwest::RequestBuilder {
    ts.client
        .request(m, format!("{}{path}", ts.base))
        .header("origin", extension_origin())
        .header("sec-fetch-site", "cross-site")
        .header("authorization", format!("Clax-Extension {cred}"))
}

async fn live_thread(ts: &TestServer, cred: &str) -> (String, String) {
    let form = reqwest::multipart::Form::new()
        .text("url", "http://localhost:5173/")
        .text("title", "Home")
        .text("anchor", json!({"kind": "element", "selector": "body", "file": "index.html"}).to_string())
        .text("body", "From the extension")
        .text("snapshot", "<!doctype html><p>x")
        .part("clip", reqwest::multipart::Part::bytes(FAKE_PNG.to_vec()).mime_str("image/png").unwrap());
    let res = ext(ts, Method::POST, "/api/live/threads", cred).multipart(form).send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["access-control-allow-origin"], extension_origin().as_str());
    let v: Value = res.json().await.unwrap();
    (v["page"]["artifact_id"].as_str().unwrap().into(), v["thread"]["id"].as_str().unwrap().into())
}

#[tokio::test]
async fn preflights_are_answered_for_the_extension_and_allowed_routes_only() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .request(Method::OPTIONS, format!("{}/api/live/threads", ts.base))
        .header("origin", extension_origin())
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(res.headers()["access-control-allow-origin"], extension_origin().as_str());
    assert!(res.headers()["access-control-allow-headers"].to_str().unwrap().contains("authorization"));
    let res = ts
        .client
        .request(Method::OPTIONS, format!("{}/api/artifacts", ts.base))
        .header("origin", extension_origin())
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn the_extension_acts_as_the_owner_on_live_pages() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let owner = ts.owner_viewer().await;
    let me: Value = ext(&ts, Method::GET, "/api/viewers/me", &cred).send().await.unwrap().json().await.unwrap();
    assert_eq!(me["viewer"]["public_id"], owner.public_id.as_str(), "the extension is the owner viewer");
    let res = ext(&ts, Method::PUT, "/api/viewers/me", &cred).json(&json!({"display_name": "Alex"})).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers().get("set-cookie").is_none(), "the gateway never hands the extension a viewer cookie");
    assert_eq!(ts.owner_viewer().await.display_name.as_deref(), Some("Alex"), "the name is the owner's");
    let (aid, tid) = live_thread(&ts, &cred).await;
    let t: Value = ext(&ts, Method::GET, &format!("/api/artifacts/{aid}/threads/{tid}"), &cred)
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(t["thread"]["comments"][0]["author_name"], "Alex");
    assert_eq!(t["thread"]["comments"][0]["author_public_id"], owner.public_id.as_str());
    let looked = ext(&ts, Method::PUT, "/api/viewers/me/looked", &cred)
        .json(&json!({"artifact_id": aid, "thread_ids": [tid]})).send().await.unwrap();
    assert!(looked.status().is_success(), "looked-at marks are the owner's: {}", looked.status());
    for (m, path, body) in [
        (Method::POST, format!("/api/artifacts/{aid}/threads/{tid}/comments"), json!({"body": "more"})),
        (Method::POST, format!("/api/artifacts/{aid}/threads/{tid}/send"), json!({})),
        (Method::POST, format!("/api/artifacts/{aid}/threads/{tid}/resolve"), json!({})),
        (Method::POST, format!("/api/artifacts/{aid}/threads/{tid}/reopen"), json!({})),
        (Method::GET, format!("/api/artifacts/{aid}/threads"), Value::Null),
        (Method::GET, format!("/api/artifacts/{aid}/working"), Value::Null),
        (Method::GET, format!("/api/artifacts/{aid}"), Value::Null),
        (Method::GET, "/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F".to_string(), Value::Null),
    ] {
        let mut r = ext(&ts, m.clone(), &path, &cred);
        if !body.is_null() {
            r = r.json(&body);
        }
        let st = r.send().await.unwrap().status();
        assert!(st.is_success(), "{m} {path}: {st}");
    }
}

#[tokio::test]
async fn everything_else_is_refused() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap();
    let (aid, _) = live_thread(&ts, &cred).await;
    for (m, path, want) in [
        (Method::GET, "/api/artifacts".to_string(), 403),
        (Method::POST, "/api/artifacts".to_string(), 403),
        (Method::POST, format!("/api/artifacts/{aid}/versions"), 403),
        (Method::DELETE, format!("/api/artifacts/{aid}"), 403),
        (Method::GET, "/api/sessions".to_string(), 403),
        (Method::GET, "/api/token".to_string(), 403),
        (Method::GET, format!("/api/artifacts/{aid}/docs"), 403),
        (Method::GET, format!("/api/artifacts/{hid}"), 404),
        (Method::GET, format!("/api/artifacts/{hid}/threads"), 404),
    ] {
        let st = ext(&ts, m.clone(), &path, &cred).send().await.unwrap().status();
        assert_eq!(st.as_u16(), want, "{m} {path}");
    }
}

#[tokio::test]
async fn credentials_and_origins_are_checked() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let path = "/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F";
    let unknown = ext(&ts, Method::GET, path, "cxe_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").send().await.unwrap();
    assert_eq!(unknown.status(), 401);
    let bearer = ts.client.get(format!("{}{path}", ts.base))
        .header("origin", extension_origin())
        .header("authorization", format!("Bearer {}", ts.token))
        .send().await.unwrap();
    assert_eq!(bearer.status(), 403, "the token is never accepted from the extension's origin");
    let other = ts.client.get(format!("{}{path}", ts.base))
        .header("origin", format!("chrome-extension://{}", "b".repeat(32)))
        .header("authorization", format!("Clax-Extension {cred}"))
        .send().await.unwrap();
    assert_eq!(other.status(), 403, "another extension is a foreign origin");
    ts.authed(ts.client.delete(format!("{}/api/extension/credentials", ts.base))).send().await.unwrap();
    let revoked = ext(&ts, Method::GET, path, &cred).send().await.unwrap();
    assert_eq!(revoked.status(), 401);
}

#[tokio::test]
async fn the_extensions_stream_is_live_only() {
    let ts = TestServer::spawn().await;
    let cred = credential(&ts).await;
    let (aid, _) = live_thread(&ts, &cred).await;
    let html = ts.publish("T", &[("index.html", "<p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap().to_string();
    let res = ext(&ts, Method::GET, "/api/stream", &cred).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut events = clax_server::testing::EventReader::from_response(res);
    let ready = events.next_named("ready").await;
    let sid = ready["stream"].as_str().unwrap().to_string();
    let sub = |topic: String| {
        ext(&ts, Method::POST, &format!("/api/stream/{sid}"), &cred).json(&json!({"subscribe": [topic]}))
    };
    assert_eq!(sub(format!("artifact:{aid}")).send().await.unwrap().status(), 200);
    assert_eq!(sub("gallery".into()).send().await.unwrap().status(), 403);
    assert_eq!(sub(format!("artifact:{hid}")).send().await.unwrap().status(), 403);
    assert_eq!(sub(format!("docs:{aid}")).send().await.unwrap().status(), 403);
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p clax-server --test api_gateway`
Expected: FAIL: the extension's requests are refused 403 `forbidden_origin` by `SameOrigin`, and preflights carry no CORS headers.

- [ ] **Step 3: Implement the gateway**

Append to `crates/clax-server/src/extension.rs`:

```rust
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use clax_core::extension::{credential_hash, extension_origin, is_credential};

/// The `Authorization` scheme of the extension's requests.
pub const SCHEME: &str = "Clax-Extension";

/// Marks a request the gateway admitted, for handlers with rules of their
/// own (a stream opened through it is live-only). Who it is comes from the
/// owner hook, not from here.
#[derive(Clone, Copy, Debug)]
pub struct ViaExtension;

/// What a route needs besides the credential: nothing more, or that the
/// artifact in its path is a live page.
enum Rule<'a> {
    Any,
    Live(&'a str),
}

/// The routes the extension may use (spec 2026-10-05 §9.2).
fn rule<'a>(m: &Method, path: &'a str) -> Option<Rule<'a>> {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (get, post, put, del) = (m == Method::GET, m == Method::POST, m == Method::PUT, m == Method::DELETE);
    match segs.as_slice() {
        ["api", "live", "pages"] if get => Some(Rule::Any),
        ["api", "live", "threads" | "snapshots"] if post => Some(Rule::Any),
        ["api", "viewers", "me"] if get || put => Some(Rule::Any),
        ["api", "viewers", "me", "looked" | "presence"] if put => Some(Rule::Any),
        ["api", "stream"] if get => Some(Rule::Any),
        ["api", "stream", _] if post => Some(Rule::Any),
        ["api", "artifacts", aid] if get => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads" | "working" | "presence"] if get => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads:send"] if post => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads", _] if get || del => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads", _, "clip"] if get => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads", _, "comments" | "send" | "resolve" | "reopen"] if post => Some(Rule::Live(aid)),
        _ => None,
    }
}

fn cors(res: &mut Response) {
    let h = res.headers_mut();
    if let Ok(o) = HeaderValue::from_str(&extension_origin()) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
    }
    h.append(header::VARY, HeaderValue::from_static("Origin"));
}

fn refuse(e: ApiError) -> Response {
    let mut r = e.into_response();
    cors(&mut r);
    r
}

/// The credential in `Authorization: Clax-Extension <credential>`.
fn presented(h: &HeaderMap) -> Option<&str> {
    let v = h.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, c) = v.split_once(' ')?;
    let c = c.trim();
    (scheme.eq_ignore_ascii_case(SCHEME) && is_credential(c)).then_some(c)
}

/// Middleware (spec 2026-10-05 L5, L6, §10 item 6). A request whose
/// `Origin` is the extension's is admitted only to [`rule`]'s routes, only
/// with a live credential, and only for live pages; it then reaches the
/// route's handler as the owner identity (`Origin`, `Sec-Fetch-Site`,
/// `Cookie` and `Authorization` removed; the owner presented through the
/// owner hook), and its response carries the extension's origin in
/// `Access-Control-Allow-Origin`. A bearer token from that origin is
/// refused. Requests from any other origin pass untouched.
pub async fn gateway(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let origin = extension_origin();
    let ours = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(origin.as_str());
    if !ours {
        return next.run(req).await;
    }
    if req.method() == Method::OPTIONS {
        let asked = req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|v| Method::from_bytes(v.as_bytes()).ok());
        if asked.as_ref().and_then(|m| rule(m, req.uri().path())).is_none() {
            return refuse(ApiError::forbidden("forbidden", "the extension may not use this route"));
        }
        let mut r = StatusCode::NO_CONTENT.into_response();
        let h = r.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, PUT, DELETE"));
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization, content-type, last-event-id"));
        h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        cors(&mut r);
        return r;
    }
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("bearer ")));
    if bearer {
        return refuse(ApiError::forbidden("forbidden", "the daemon token is not accepted from the extension"));
    }
    let path = req.uri().path().to_string();
    let Some(r) = rule(req.method(), &path) else {
        return refuse(ApiError::forbidden("forbidden", "the extension may not use this route"));
    };
    let unknown = || refuse(ApiError::new(StatusCode::UNAUTHORIZED, "unknown_credential", "pair the extension again"));
    let Some(hash) = presented(req.headers()).map(credential_hash) else { return unknown() };
    if s.ext_creds.get(&hash).is_none() {
        return unknown();
    }
    if let Rule::Live(aid) = r
        && !s.live_ids.contains(aid)
    {
        return refuse(ApiError::from(clax_core::CoreError::NotFound));
    }
    if s.ext_creds.due_for_touch(&hash) {
        let store = s.store.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = store.touch_extension_credential(&hash) {
                tracing::warn!(error = %e, "could not record an extension credential's use");
            }
        });
    }
    let h = req.headers_mut();
    for name in [header::ORIGIN, header::COOKIE, header::AUTHORIZATION] {
        h.remove(name);
    }
    h.remove("sec-fetch-site");
    // The owner hook (spec §2.1): present this request as the owner identity
    // in the one way main's owner identity defines. Placeholder below.
    if let Err(e) = crate::owner::present_as_owner(&s, &mut req).await {
        return refuse(e);
    }
    req.extensions_mut().insert(ViaExtension);
    let mut res = next.run(req).await;
    res.headers_mut().remove(header::SET_COOKIE);
    cors(&mut res);
    res
}
```

`crate::owner::present_as_owner` stands for the owner hook. Implement this step against what main has, adding the extension credential as one more kind the hook maps to the owner, in that one place, not in the handlers:

- If the owner identity is a viewer cookie the existing handlers read (the owner viewer's ID), resolve it with `owner_viewer` (in `store_call`; cache it on `AppState` if main does not already) and insert `Cookie: clax_viewer=<owner viewer ID>`, as the shell's own requests carry it.
- If it is a principal the handlers read from the request (for example a `Principal::Owner` in the request's extensions, or an extractor over a credential enum), insert that principal, or add the extension credential's arm to the enum and the extractor.

Either way: the extension's requests never carry the token, `SET_COOKIE` never reaches the extension (the owner's cookie value is the owner's credential), and `the_extension_acts_as_the_owner_on_live_pages` passes.

In `routes/mod.rs`, add the gateway as the outermost layer of the final router, after `hide_live_pages`, so it runs first: `r.layer(axum::middleware::from_fn_with_state(state.clone(), crate::extension::gateway))`.

- [ ] **Step 4: Live-only streams**

In `routes/stream.rs::open`, take `via: Option<axum::Extension<crate::extension::ViaExtension>>` and pass `via.is_some()` as `live_only` to `s.stream.open(caller, local, live_only, resume)`. The hub keeps `live_only` on the stream; `Hub::update`, for a live-only stream, refuses `Topic::Gallery`, every `Topic::Docs(_)`, and every topic whose artifact is not in `self.live`, with a new `SubError::Forbidden` that `routes::stream::update` answers 403 `forbidden`. Add a hub unit test:

```rust
    #[test]
    fn a_live_only_stream_takes_live_pages_topics_alone() {
        let live = Arc::new(crate::live::LiveIds::default());
        live.insert("7q3k9mzx2b4t");
        let hub = Hub::new_for_test(live);
        let c = viewer(Level::Interact, Some("u_x"));
        let o = hub.open(c.clone(), true, true, None);
        assert!(hub.update(&o.id, &c, &[Topic::Artifact("7q3k9mzx2b4t".into())], &[]).is_ok());
        assert!(matches!(hub.update(&o.id, &c, &[Topic::Gallery], &[]), Err(SubError::Forbidden)));
        assert!(matches!(
            hub.update(&o.id, &c, &[Topic::Artifact("aaaaaaaaaaaa".into())], &[]),
            Err(SubError::Forbidden)
        ));
    }
```

(Every other `open` call in the module's tests gains `, false` for `live_only`.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p clax-server`
Expected: PASS: the gateway tests, and every earlier test unchanged (requests without the extension's origin pass the gateway untouched).

- [ ] **Step 6: Document and commit**

`docs/contract.md` "Security model": one paragraph on the extension (spec §10 items 2, 3, 6 and 7): the credential and where it lives, that it acts as the owner identity (its name, marks and actions are the owner's) within the gateway's allowlist and live-only rule, CORS for the one origin, the refused bearer token, live-only streams.

```bash
git add crates docs/contract.md
git -c commit.gpgsign=false commit -m "Admit the Chrome extension through a credential gateway, to live pages only"
```

---

### Task 7: `clax native-host`

**Files:**
- Create: `crates/clax-cli/src/commands/native_host.rs`
- Create: `crates/clax-cli/tests/native_host.rs`
- Modify: `crates/clax-cli/src/commands/mod.rs`, `crates/clax-cli/src/main.rs` (the subcommand, hidden from `--help`)

**Interfaces:**
- Consumes: Task 5's `EXTENSION_ID`, `extension_origin()`, `POST /api/extension/credentials`; `crate::client::Client::{connect, post, browser_url}`.
- Produces:
  - `clax native-host <origin> [chrome's other arguments]`: reads one native message, writes one, exits 0 (1 for a wrong origin). The `paired` reply's `viewer` is the mint's `viewer`, the owner viewer (spec L6), passed through unchanged.
  - `commands::native_host::{MAX_MESSAGE: usize = 65536, read_message(&mut impl Read) -> Result<Value, HostError>, write_message(&mut impl Write, &Value) -> std::io::Result<()>, answer(&Value, impl FnOnce() -> Result<Value, HostError>) -> Value, HostError {code: &'static str, message: String}}`.

- [ ] **Step 1: Write the failing tests**

Unit tests at the bottom of `crates/clax-cli/src/commands/native_host.rs` (create it with just these and `use super::*;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn framed(v: &serde_json::Value) -> Vec<u8> {
        let mut out = Vec::new();
        write_message(&mut out, v).unwrap();
        out
    }

    #[test]
    fn messages_round_trip_with_a_native_endian_length() {
        let v = json!({"type": "pair", "v": 1});
        let bytes = framed(&v);
        let n = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(n, bytes.len() - 4);
        assert_eq!(read_message(&mut bytes.as_slice()).unwrap(), v);
    }

    #[test]
    fn oversized_truncated_and_malformed_messages_are_bad_requests() {
        let mut big = ((MAX_MESSAGE + 1) as u32).to_ne_bytes().to_vec();
        big.extend(std::iter::repeat_n(b' ', 16));
        assert_eq!(read_message(&mut big.as_slice()).unwrap_err().code, "bad_request");
        let mut short = 10u32.to_ne_bytes().to_vec();
        short.extend(b"{}");
        assert_eq!(read_message(&mut short.as_slice()).unwrap_err().code, "bad_request");
        let mut junk = 3u32.to_ne_bytes().to_vec();
        junk.extend(b"{{{");
        assert_eq!(read_message(&mut junk.as_slice()).unwrap_err().code, "bad_request");
    }

    #[test]
    fn only_pair_v1_is_answered() {
        let paired = || Ok(json!({"type": "paired"}));
        assert_eq!(answer(&json!({"type": "pair", "v": 1}), paired)["type"], "paired");
        assert_eq!(answer(&json!({"type": "pair", "v": 2}), paired)["code"], "unsupported_version");
        assert_eq!(answer(&json!({"type": "other", "v": 1}), paired)["code"], "bad_request");
        let down = || Err(HostError { code: "daemon_unavailable", message: "no".into() });
        assert_eq!(answer(&json!({"type": "pair", "v": 1}), down)["code"], "daemon_unavailable");
    }
}
```

`crates/clax-cli/tests/native_host.rs`:

```rust
//! `clax native-host` against a real daemon on a scratch home.

use assert_cmd::Command;
use clax_core::extension::{EXTENSION_ID, extension_origin};
use serde_json::{Value, json};

fn frame(v: &Value) -> Vec<u8> {
    let b = serde_json::to_vec(v).unwrap();
    let mut out = (b.len() as u32).to_ne_bytes().to_vec();
    out.extend(b);
    out
}

/// The one framed message in `stdout`; fails when it holds anything else.
fn only_message(stdout: &[u8]) -> Value {
    let n = u32::from_ne_bytes(stdout[..4].try_into().unwrap()) as usize;
    assert_eq!(stdout.len(), 4 + n, "stdout holds exactly one message");
    serde_json::from_slice(&stdout[4..]).unwrap()
}

fn host(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("clax").unwrap();
    c.env("CLAX_HOME", home).env("CLAX_NO_OPEN", "1");
    c
}

#[test]
fn pairing_starts_a_daemon_and_returns_a_credential() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 0\n").unwrap();
    let out = host(dir.path())
        .args(["native-host", &format!("{}/", extension_origin())])
        .write_stdin(frame(&json!({"type": "pair", "v": 1, "extension_version": "0.0.0"})))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v = only_message(&out.stdout);
    assert_eq!(v["type"], "paired", "{v}");
    assert!(v["credential"].as_str().unwrap().starts_with("cxe_"));
    assert!(v["daemon"].as_str().unwrap().starts_with("http://localhost:"));
    assert_eq!(v["clax_version"], env!("CARGO_PKG_VERSION"));
    let st = Command::cargo_bin("clax").unwrap().env("CLAX_HOME", dir.path()).args(["stop"]).output().unwrap();
    assert!(st.status.success());
    let _ = EXTENSION_ID;
}

#[test]
fn a_wrong_origin_gets_one_error_and_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = host(dir.path())
        .args(["native-host", "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/"])
        .write_stdin(frame(&json!({"type": "pair", "v": 1})))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(only_message(&out.stdout)["code"], "wrong_origin");
    assert!(!dir.path().join("daemon.json").exists(), "no daemon was started");
}
```

(Check how `crates/clax-cli/tests/cli.rs` gives a scratch daemon its port and stops it; follow it if `port = 0` in `config.toml` is not how it does so.)

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p clax-cli --test native_host`
Expected: FAIL: `clax native-host` is not a subcommand.

- [ ] **Step 3: Implement**

Above the tests in `crates/clax-cli/src/commands/native_host.rs`:

```rust
//! `clax native-host` (spec 2026-10-05-chrome-overlay-design §9.1): the
//! Chrome native messaging host that pairs the Clax extension with the
//! daemon. Chrome runs it once per message, with the caller's origin as the
//! first argument; it reads one length-prefixed JSON message from stdin,
//! writes one reply to stdout, and exits. Nothing else ever goes to stdout.

use crate::client::Client;
use clax_core::Home;
use clax_core::extension::{EXTENSION_ID, extension_origin};
use serde_json::{Value, json};
use std::io::{Read, Write};

/// The largest message read or written.
pub const MAX_MESSAGE: usize = 64 * 1024;

#[derive(clap::Args)]
pub struct Args {
    /// The caller's origin, as Chrome passes it (`chrome-extension://<ID>/`).
    pub origin: Option<String>,
    /// Further arguments Chrome may pass; ignored.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

#[derive(Debug)]
pub struct HostError {
    pub code: &'static str,
    pub message: String,
}

fn bad(message: impl Into<String>) -> HostError {
    HostError { code: "bad_request", message: message.into() }
}

/// Reads one message: a 32-bit native-endian length, then that many bytes of JSON.
pub fn read_message(r: &mut impl Read) -> Result<Value, HostError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).map_err(|e| bad(format!("no message: {e}")))?;
    let n = u32::from_ne_bytes(len) as usize;
    if n > MAX_MESSAGE {
        return Err(bad(format!("a message is at most {MAX_MESSAGE} bytes")));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).map_err(|e| bad(format!("short message: {e}")))?;
    serde_json::from_slice(&buf).map_err(|e| bad(format!("not JSON: {e}")))
}

/// Writes one message in the same framing and flushes it.
pub fn write_message(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(std::io::Error::other("reply too large"));
    }
    w.write_all(&(bytes.len() as u32).to_ne_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

fn error(code: &str, message: &str) -> Value {
    json!({"type": "error", "v": 1, "code": code, "message": message})
}

/// The reply to `msg`: `pair` (version 1) runs `pair`; anything else is an error reply.
pub fn answer(msg: &Value, pair: impl FnOnce() -> Result<Value, HostError>) -> Value {
    if msg["v"] != 1 {
        return error("unsupported_version", "this clax speaks version 1 of the pairing protocol");
    }
    match msg["type"].as_str() {
        Some("pair") => pair().unwrap_or_else(|e| error(e.code, &e.message)),
        _ => error("bad_request", "unknown message type"),
    }
}

/// Ensures a daemon and mints a credential with the token.
fn pair(cli: &crate::Cli, home: &Home) -> Result<Value, HostError> {
    let down = |e: anyhow::Error| HostError {
        code: "daemon_unavailable",
        message: format!("{e:#}; see {}", home.log_path().display()),
    };
    let port = cli.port_for(home).map_err(down)?;
    let c = Client::connect(home, port).map_err(down)?;
    let v = c
        .post("/api/extension/credentials", &json!({"extension_id": EXTENSION_ID}))
        .map_err(down)?;
    Ok(json!({
        "type": "paired",
        "v": 1,
        "daemon": c.browser_url("").trim_end_matches('/'),
        "credential": v["credential"],
        "clax_version": env!("CARGO_PKG_VERSION"),
        "viewer": v["viewer"],
    }))
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    if a.origin.as_deref() != Some(format!("{}/", extension_origin()).as_str()) {
        write_message(&mut out, &error("wrong_origin", "clax native-host answers only the Clax extension"))?;
        anyhow::bail!("clax native-host was started for another origin");
    }
    let reply = match read_message(&mut std::io::stdin().lock()) {
        Ok(m) => answer(&m, || pair(cli, home)),
        Err(e) => error(e.code, &e.message),
    };
    write_message(&mut out, &reply)?;
    Ok(())
}
```

Add `NativeHost(commands::native_host::Args)` to `Cmd` with `#[command(hide = true)]` and the doc comment "The Chrome native messaging host for the Clax extension (Chrome runs it).", and dispatch it in `main`. Check `main.rs`'s logging setup: nothing may be written to stdout for this command (tracing must go to stderr or the log file); if `main` prints errors to stdout, route this command's to stderr.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p clax-cli native_host && cargo test -p clax-cli --test native_host`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-cli
git -c commit.gpgsign=false commit -m "Add clax native-host, the extension's pairing host"
```

---

### Task 8: The extension package and its build

**Files:**
- Create: `web/extension/composer.html`, `web/extension/sidepanel.html`
- Create: `web/extension/src/messages.ts`, `web/extension/src/messages.test.ts`
- Create: `web/extension/src/sw/main.ts`, `web/extension/src/content/loader.ts`, `web/extension/src/content/overlay.ts`, `web/extension/src/composer/main.ts`, `web/extension/src/panel/main.ts` (each a minimal placeholder module that later tasks fill: `export {};` plus the doc comment naming the task)
- Create: `web/extension/test/fake-chrome.ts`
- Create: `web/scripts/build-extension.mjs`, `web/scripts/extension-icons.mjs`
- Create: `web/dist-extension/.gitkeep`
- Modify: `web/package.json` (build script; `@types/chrome` dev dependency), `web/tsconfig.json`, `web/vitest.config.ts`, `.gitignore`
- Modify: `web/scripts/bundle-size.mjs`, `web/perf/bundle-budget.json`

**Interfaces:**
- Consumes: Task 5's `web/extension/manifest.json`; `web/bridge/src/protocol.ts` (`Anchor`, `AnchorResult`); `web/shell/src/threads.ts` (`Thread`).
- Produces:
  - `npm run build` writes `web/dist-extension/` (release) and `web/dist-extension-test/` (test build: `host_permissions: ["<all_urls>"]`, `__CLAX_EXT_TEST__` true).
  - `web/extension/src/messages.ts`: the message types of spec §9.4 and the validators `isFromOverlay(m): m is OverlayToWorker`, `isFromWorker(m): m is WorkerToOverlay`, `isFromComposer(m): m is ComposerToWorker`, `isFromPanel(m): m is PanelToWorker`, plus `isAnchor(v): v is Anchor`, `MAX_URL = 4096`, `MAX_BODY = 10_000`, `MAX_SNAPSHOT_CHARS = 8 * 1024 * 1024`.
  - `web/extension/test/fake-chrome.ts`: `fakeChrome(): FakeChrome`, installed as `globalThis.chrome` in tests.

- [ ] **Step 1: Write the failing validator tests**

`web/extension/src/messages.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { MAX_BODY, MAX_URL, isAnchor, isFromComposer, isFromOverlay, isFromPanel } from "./messages";

const anchor = { kind: "element", selector: "main > button", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };

describe("messages", () => {
  it("takes the overlay's well-formed messages", () => {
    expect(isFromOverlay({ t: "hello", url: "http://localhost:5173/" })).toBe(true);
    expect(isFromOverlay({ t: "capture", rect: { x: 1, y: 2, w: 3, h: 4 }, dpr: 2 })).toBe(true);
    expect(isFromOverlay({ t: "pick", pickId: "a".repeat(32), anchor, url: "http://x/", title: "T", snapshot: "<p>", snapshotError: null })).toBe(true);
  });

  it("drops anything else", () => {
    expect(isFromOverlay(null)).toBe(false);
    expect(isFromOverlay({ t: "hello" })).toBe(false);
    expect(isFromOverlay({ t: "hello", url: "x".repeat(MAX_URL + 1) })).toBe(false);
    expect(isFromOverlay({ t: "capture", rect: { x: Number.NaN, y: 0, w: 1, h: 1 }, dpr: 1 })).toBe(false);
    expect(isFromOverlay({ t: "pick", pickId: "short", anchor, url: "http://x/", title: "T", snapshot: null, snapshotError: "too_large" })).toBe(false);
    expect(isFromOverlay({ t: "steal", url: "http://x/" })).toBe(false);
    expect(isFromComposer({ t: "post", body: "x".repeat(MAX_BODY + 1) })).toBe(false);
    expect(isFromPanel({ t: "send", threadId: "not a ulid" })).toBe(false);
  });

  it("checks anchors field by field", () => {
    expect(isAnchor(anchor)).toBe(true);
    expect(isAnchor({ ...anchor, kind: "script" })).toBe(false);
    expect(isAnchor({ ...anchor, selector: "x".repeat(1025) })).toBe(false);
    expect(isAnchor({ ...anchor, file: "../etc" })).toBe(false);
    expect(isAnchor({ ...anchor, route: "?a" })).toBe(false); // the daemon sets routes
  });
});
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cd web && npx vitest run extension/src/messages.test.ts`
Expected: FAIL: the test file is not included (add `"extension/src/**/*.test.ts"` to `include` in `web/vitest.config.ts` first), then FAIL: cannot resolve `./messages`.

- [ ] **Step 3: Implement the messages**

`web/extension/src/messages.ts`:

```ts
// Every message between the extension's parts (spec 2026-10-05 §9.4), and
// one validator per receiver. A receiver drops anything its validator
// refuses: content scripts share the page's DOM, so the worker treats what
// they send as checked input, never as trusted.
import type { Anchor, AnchorResult } from "../../bridge/src/protocol";
import type { Thread } from "../../shell/src/threads";

export const MAX_URL = 4096;
export const MAX_BODY = 10_000;
export const MAX_TITLE = 1000;
export const MAX_SNAPSHOT_CHARS = 8 * 1024 * 1024;
export const PICK_ID = /^[0-9a-f]{32}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const HANDLE = /^a_[0-9a-f]{22}$/;

export type Rect = { x: number; y: number; w: number; h: number };
export type PageView = { artifact_id: string; origin: string; path: string; page_url: string; title: string; current_version: number; url: string };

export type OverlayToWorker =
  | { t: "hello"; url: string }
  | { t: "route"; url: string }
  | { t: "capture"; rect: Rect; dpr: number }
  | { t: "pick"; pickId: string; anchor: Anchor; url: string; title: string; snapshot: string | null; snapshotError: string | null }
  | { t: "quiet"; url: string; title: string; snapshot: string }
  | { t: "resolved"; results: AnchorResult[] }
  | { t: "comment-mode"; on: boolean }
  | { t: "cancel"; pickId: string | null }
  | { t: "pin"; threadId: string }
  | { t: "removed" }
  | { t: "ping" };

export type WorkerToOverlay =
  | { t: "state"; page: PageView | null; route: string | null; threads: Thread[]; commentMode: boolean; pending: boolean }
  | { t: "comment-mode"; on: boolean }
  | { t: "close-composer"; pickId: string; posted: boolean }
  | { t: "scroll-to"; threadId: string }
  | { t: "focus"; threadId: string | null }
  | { t: "snapshot-now" };

export type ComposerToWorker = { t: "ready" } | { t: "post"; body: string } | { t: "cancel" };
/** `clipUrl` is a `data:image/png` URL (a service worker cannot make object URLs). */
export type WorkerToComposer =
  | { t: "draft"; anchor: Anchor; clipUrl: string | null; clipError: string | null; capturing: boolean }
  | { t: "posted"; threadId: string }
  | { t: "failed"; message: string };

/** What the side panel shows for its window's active tab. */
export type PanelState = {
  tabId: number | null;
  url: string | null;
  page: PageView | null;
  route: string | null;
  threads: Thread[];
  resolved: Record<string, AnchorResult>;
  versions: import("../../shell/src/api").Version[];
  working: import("../../shell/src/view/working-model").Working[];
  participants: import("../../shell/src/api").Participants | null;
  viewer: { public_id: string; display_name: string | null } | null;
  commentMode: boolean;
  enabled: boolean;
  selected: string | null;
  error: { code: string; message: string } | null;
};
export type WorkerToPanel =
  | { t: "tab"; state: PanelState }
  | { t: "failed"; code: string; message: string }
  | { t: "ping" };

export type PanelToWorker =
  | { t: "watch-tab"; tabId: number }
  | { t: "send"; threadId: string; to: string | null }
  | { t: "send-batch"; threadIds: string[]; note: string | null; to: string | null }
  | { t: "reply"; threadId: string; body: string }
  | { t: "resolve"; threadId: string }
  | { t: "reopen"; threadId: string }
  | { t: "delete"; threadId: string }
  | { t: "looked"; threadIds: string[] }
  | { t: "set-name"; name: string }
  | { t: "select"; threadId: string | null }
  | { t: "comment-mode"; on: boolean }
  | { t: "navigate"; route: string | null }
  | { t: "turn-off" }
  | { t: "retry" }
  | { t: "ping" };

type Obj = Record<string, unknown>;
const obj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown, max: number): v is string => typeof v === "string" && v.length <= max;
const strOrNull = (v: unknown, max: number) => v === null || str(v, max);
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const bool = (v: unknown): v is boolean => typeof v === "boolean";
const url = (v: unknown) => str(v, MAX_URL) && /^https?:\/\//.test(v as string);
const ulid = (v: unknown) => typeof v === "string" && ULID.test(v);
const rect = (v: unknown): v is Rect => obj(v) && num(v.x) && num(v.y) && num(v.w) && num(v.h) && (v.w as number) >= 0 && (v.h as number) >= 0;

const KINDS = new Set(["element", "range", "area"]);
/** An anchor as the overlay builds one (spec main §9 "Anchors"); the daemon validates it again. */
export function isAnchor(v: unknown): v is Anchor {
  if (!obj(v) || !KINDS.has(v.kind as string) || v.file !== "index.html" || "route" in v) return false;
  if (!strOrNull(v.selector, 1024) || !strOrNull(v.quote, 2000) || !strOrNull(v.prefix, 64) || !strOrNull(v.suffix, 64)) return false;
  if (!strOrNull(v.html_hash, 80) || v.custom_name !== null) return false;
  if (v.rect !== null && !(obj(v.rect) && ["x", "y", "w", "h", "scrollX", "scrollY", "viewportW"].every(k => num((v.rect as Obj)[k])))) return false;
  if (v.area !== undefined && !(obj(v.area) && ["x", "y", "w", "h"].every(k => num((v.area as Obj)[k])))) return false;
  return true;
}

export function isFromOverlay(m: unknown): m is OverlayToWorker {
  if (!obj(m)) return false;
  switch (m.t) {
    case "hello": case "route": return url(m.url);
    case "capture": return rect(m.rect) && num(m.dpr) && (m.dpr as number) > 0 && (m.dpr as number) <= 8;
    case "pick": return typeof m.pickId === "string" && PICK_ID.test(m.pickId) && isAnchor(m.anchor) && url(m.url) && str(m.title, MAX_TITLE)
      && strOrNull(m.snapshot, MAX_SNAPSHOT_CHARS) && strOrNull(m.snapshotError, 40) && (m.snapshot !== null || m.snapshotError !== null);
    case "quiet": return url(m.url) && str(m.title, MAX_TITLE) && str(m.snapshot, MAX_SNAPSHOT_CHARS);
    case "resolved": return Array.isArray(m.results) && m.results.length <= 500 && m.results.every(r => obj(r) && ulid(r.id) && bool(r.found));
    case "comment-mode": return bool(m.on);
    case "cancel": return m.pickId === null || (typeof m.pickId === "string" && PICK_ID.test(m.pickId));
    case "pin": return ulid(m.threadId);
    case "removed": case "ping": return true;
    default: return false;
  }
}

export function isFromWorker(m: unknown): m is WorkerToOverlay {
  if (!obj(m)) return false;
  switch (m.t) {
    case "state": return Array.isArray(m.threads) && bool(m.commentMode) && bool(m.pending);
    case "comment-mode": return bool(m.on);
    case "close-composer": return typeof m.pickId === "string" && bool(m.posted);
    case "scroll-to": return ulid(m.threadId);
    case "focus": return m.threadId === null || ulid(m.threadId);
    case "snapshot-now": return true;
    default: return false;
  }
}

export function isFromComposer(m: unknown): m is ComposerToWorker {
  if (!obj(m)) return false;
  switch (m.t) {
    case "ready": case "cancel": return true;
    case "post": return str(m.body, MAX_BODY) && (m.body as string).trim().length > 0;
    default: return false;
  }
}

export function isFromPanel(m: unknown): m is PanelToWorker {
  if (!obj(m)) return false;
  const to = (v: unknown) => v === null || (typeof v === "string" && HANDLE.test(v));
  switch (m.t) {
    case "watch-tab": return num(m.tabId);
    case "send": return ulid(m.threadId) && to(m.to);
    case "send-batch": return Array.isArray(m.threadIds) && m.threadIds.length >= 1 && m.threadIds.length <= 20 && m.threadIds.every(ulid) && strOrNull(m.note, 280) && to(m.to);
    case "reply": return ulid(m.threadId) && str(m.body, MAX_BODY) && (m.body as string).trim().length > 0;
    case "resolve": case "reopen": case "delete": return ulid(m.threadId);
    case "looked": return Array.isArray(m.threadIds) && m.threadIds.length <= 50 && m.threadIds.every(ulid);
    case "set-name": return str(m.name, 64);
    case "select": return m.threadId === null || ulid(m.threadId);
    case "comment-mode": return bool(m.on);
    case "navigate": return m.route === null || str(m.route, 512);
    case "turn-off": case "retry": case "ping": return true;
    default: return false;
  }
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cd web && npx vitest run extension/src/messages.test.ts`
Expected: PASS.

- [ ] **Step 5: The build, the icons and the fake `chrome`**

`web/extension/composer.html`:

```html
<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Clax comment</title></head>
<body><div id="app"></div><script type="module" src="./src/composer/main.ts"></script></body>
</html>
```

`web/extension/sidepanel.html`: the same with `<title>Clax threads</title>` and `./src/panel/main.ts`.

`web/scripts/extension-icons.mjs`:

```js
// Draws the Echo mark (two half-discs facing an ink dot; red-orange people
// on the left, green agents on the right) as the extension's PNG icons, so
// no binary image lives in the repository. 4×4 supersampling per pixel.
import { mkdirSync, writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const CRC = new Uint32Array(256).map((_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc32 = buf => { let c = 0xffffffff; for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}
function png(size, rgba) {
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) { raw[y * (size * 4 + 1)] = 0; rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4); }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4); ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}
// The mark in its 30×24 viewBox (web/shell/src/ui/Mark.svelte).
const PEOPLE = [0xe0, 0x53, 0x2f], AGENTS = [0x2f, 0x6f, 0x2a], INK = [0x1c, 0x1b, 0x19];
function colourAt(x, y) {
  const d = (cx, cy) => Math.hypot(x - cx, y - cy);
  if (x >= 2 && Math.abs(d(2, 12) - 9.5) <= 2.1) return PEOPLE;
  if (x <= 28 && Math.abs(d(28, 12) - 9.5) <= 2.1) return AGENTS;
  if (d(15, 12) <= 2.6) return INK;
  return null;
}
export function drawIcons(dir) {
  mkdirSync(dir, { recursive: true });
  for (const size of [16, 32, 48, 128]) {
    const rgba = Buffer.alloc(size * size * 4);
    const scale = 30 / size;
    for (let py = 0; py < size; py++) for (let px = 0; px < size; px++) {
      const sum = [0, 0, 0, 0];
      for (let sy = 0; sy < 4; sy++) for (let sx = 0; sx < 4; sx++) {
        const c = colourAt((px + (sx + 0.5) / 4) * scale, (py + (sy + 0.5) / 4) * scale - 3);
        if (c) { sum[0] += c[0]; sum[1] += c[1]; sum[2] += c[2]; sum[3] += 1; }
      }
      const i = (py * size + px) * 4;
      if (sum[3]) { rgba[i] = sum[0] / sum[3]; rgba[i + 1] = sum[1] / sum[3]; rgba[i + 2] = sum[2] / sum[3]; rgba[i + 3] = Math.round((sum[3] / 16) * 255); }
    }
    writeFileSync(`${dir}/${size}.png`, png(size, rgba));
  }
}
```

`web/scripts/build-extension.mjs`:

```js
// Builds the Clax Chrome extension (spec 2026-10-05 §6.4) twice: into
// dist-extension/ (the release build `clax extension install` embeds) and
// dist-extension-test/ (the browser tests' build, which adds
// `host_permissions: ["<all_urls>"]` and the worker's test hook). The
// worker is one ES module; the loader and the overlay are classic scripts
// (content scripts cannot be modules); the composer and the side panel are
// HTML pages. The manifest's version is the workspace's.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { build } from "vite";
import { drawIcons } from "./extension-icons.mjs";

const web = fileURLToPath(new URL("..", import.meta.url));
const root = `${web}extension/`;
const cargo = readFileSync(`${web}../Cargo.toml`, "utf8");
const version = cargo.match(/^version\s*=\s*"(\d+\.\d+\.\d+)/m)?.[1];
if (!version) throw new Error("no workspace version in Cargo.toml");

async function variant(out, test) {
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const shared = { configFile: false, logLevel: "warn", define: { __CLAX_EXT_TEST__: JSON.stringify(test), __CLAX_TEST_CLOCK__: "false" } };
  const scripts = [["sw", "sw/main.ts", "es"], ["loader", "content/loader.ts", "iife"], ["overlay", "content/overlay.ts", "iife"]];
  for (const [name, entry, format] of scripts) {
    await build({ ...shared, build: { outDir: out, emptyOutDir: false, minify: true, sourcemap: false,
      lib: { entry: `${root}src/${entry}`, formats: [format], name: `clax_${name}`, fileName: () => `${name}.js` } } });
  }
  await build({ ...shared, root, base: "./", plugins: [svelte({ configFile: `${web}svelte.config.js` })],
    build: { outDir: out, emptyOutDir: false, minify: true, sourcemap: false,
      rollupOptions: { input: { composer: `${root}composer.html`, sidepanel: `${root}sidepanel.html` } } } });
  const manifest = JSON.parse(readFileSync(`${root}manifest.json`, "utf8"));
  manifest.version = version;
  if (test) manifest.host_permissions = ["<all_urls>"];
  writeFileSync(`${out}/manifest.json`, `${JSON.stringify(manifest, null, 2)}\n`);
  drawIcons(`${out}/icons`);
}

await variant(`${web}dist-extension`, false);
await variant(`${web}dist-extension-test`, true);
```

In `web/package.json`: `"build": "… && vite build -c vite.shell.config.ts && node scripts/build-extension.mjs"`, add `"@types/chrome": "^0.0.300"` to `devDependencies` (run `npm install` so `package-lock.json` updates), and `extension` to the `lint` paths. In `web/tsconfig.json`: `"types": ["vite/client", "node", "chrome"]` and `"extension"` in `include`. Add `declare const __CLAX_EXT_TEST__: boolean;` to `web/extension/src/env.d.ts`. In `.gitignore`: `web/dist-extension/*`, `!web/dist-extension/.gitkeep`, `web/dist-extension-test/`.

`web/extension/test/fake-chrome.ts`:

```ts
// A fake `chrome` for the extension's unit tests: storage areas in memory,
// recorded calls for scripting, permissions, tabs and runtime, and events
// tests can fire. Only what the extension uses is here.
type Listener<A extends unknown[]> = (...a: A) => unknown;
export class FakeEvent<A extends unknown[]> {
  listeners: Listener<A>[] = [];
  addListener(l: Listener<A>) { this.listeners.push(l); }
  removeListener(l: Listener<A>) { this.listeners = this.listeners.filter(x => x !== l); }
  fire(...a: A) { return this.listeners.map(l => l(...a)); }
}
function area() {
  const data: Record<string, unknown> = {};
  return {
    data,
    async get(keys?: string | string[]) { const ks = keys === undefined ? Object.keys(data) : Array.isArray(keys) ? keys : [keys]; return Object.fromEntries(ks.filter(k => k in data).map(k => [k, structuredClone(data[k])])); },
    async set(items: Record<string, unknown>) { Object.assign(data, structuredClone(items)); },
    async remove(keys: string | string[]) { for (const k of Array.isArray(keys) ? keys : [keys]) delete data[k]; },
  };
}
export function fakeChrome() {
  const calls: { api: string; args: unknown[] }[] = [];
  const rec = (api: string, ret: unknown = undefined) => async (...args: unknown[]) => { calls.push({ api, args }); return typeof ret === "function" ? (ret as (...a: unknown[]) => unknown)(...args) : ret; };
  const granted = new Set<string>();
  const native: { reply: unknown } = { reply: { type: "error", v: 1, code: "daemon_unavailable", message: "not set" } };
  return {
    calls, granted, native,
    runtime: { id: "test-extension", getManifest: () => ({ version: "0.9.0" }), getURL: (p: string) => `chrome-extension://test-extension/${p}`,
      reload: rec("runtime.reload"), sendNativeMessage: rec("runtime.sendNativeMessage", () => native.reply),
      onMessage: new FakeEvent<[unknown, chrome.runtime.MessageSender, (r: unknown) => void]>(), onConnect: new FakeEvent<[chrome.runtime.Port]>() },
    storage: { local: area(), session: area() },
    permissions: { request: rec("permissions.request", (p: { origins: string[] }) => { p.origins.forEach(o => granted.add(o)); return true; }),
      contains: rec("permissions.contains", (p: { origins: string[] }) => p.origins.every(o => granted.has(o))),
      remove: rec("permissions.remove", (p: { origins: string[] }) => { p.origins.forEach(o => granted.delete(o)); return true; }) },
    scripting: { registerContentScripts: rec("scripting.registerContentScripts"), unregisterContentScripts: rec("scripting.unregisterContentScripts"),
      getRegisteredContentScripts: rec("scripting.getRegisteredContentScripts", []), executeScript: rec("scripting.executeScript", [{ result: undefined }]) },
    tabs: { captureVisibleTab: rec("tabs.captureVisibleTab", "data:image/png;base64,"), sendMessage: rec("tabs.sendMessage"), update: rec("tabs.update"), get: rec("tabs.get") },
    sidePanel: { open: rec("sidePanel.open") },
    action: { onClicked: new FakeEvent<[chrome.tabs.Tab]>() },
    commands: { onCommand: new FakeEvent<[string, chrome.tabs.Tab]>() },
    contextMenus: { create: rec("contextMenus.create"), onClicked: new FakeEvent<[chrome.contextMenus.OnClickData, chrome.tabs.Tab]>() },
  };
}
export type FakeChrome = ReturnType<typeof fakeChrome>;
```

In `web/scripts/bundle-size.mjs`, after the bridge's budgets, measure the extension's release build:

```js
const ext = new URL("../dist-extension/", import.meta.url);
const extGz = f => gzipSync(readFileSync(new URL(f, ext)), { level: 9 }).length;
const sizes = { ...measured };
sizes.extLoader = extGz("loader.js");
sizes.extOverlay = extGz("overlay.js");
sizes.extWorker = extGz("sw.js");
// The pages: the HTML and every script and stylesheet in assets/ it names.
for (const [key, html] of [["extComposer", "composer.html"], ["extPanel", "sidepanel.html"]]) {
  const text = readFileSync(new URL(html, ext), "utf8");
  const files = [...text.matchAll(/(?:src|href)="\.\/(assets\/[^"]+)"/g)].map(m => m[1]);
  sizes[key] = extGz(html) + files.reduce((n, f) => n + extGz(f), 0);
}
```

(adapt `measured` to the script's own variable of measured sizes) and add to `web/perf/bundle-budget.json`: `"extLoader": 2048, "extOverlay": 32768, "extWorker": 24576, "extComposer": 28672, "extPanel": 65536`.

- [ ] **Step 6: Build and check**

Run: `cd web && npm run build && ls dist-extension dist-extension-test && node scripts/bundle-size.mjs && npm run lint && npm run typecheck && npx vitest run extension`
Expected: both directories hold `manifest.json`, `sw.js`, `loader.js`, `overlay.js`, `composer.html`, `sidepanel.html`, `icons/`; the test build's manifest has `host_permissions`; every gate passes.

- [ ] **Step 7: Commit**

```bash
git add .gitignore web/extension web/scripts web/package.json web/package-lock.json web/tsconfig.json web/vitest.config.ts web/perf/bundle-budget.json web/dist-extension/.gitkeep
git -c commit.gpgsign=false commit -m "Add the Chrome extension package: its messages, build, icons and budgets"
```

---

### Task 9: `clax extension install`, and `init`, `uninit` and `doctor`

**Files:**
- Create: `crates/clax-cli/src/extension_files.rs`
- Create: `crates/clax-cli/src/commands/extension.rs`
- Create: `crates/clax-cli/tests/extension.rs`
- Create: `plugins/claude-code/commands/extension.md`
- Modify: `crates/clax-cli/build.rs` (rebuild when `web/dist-extension` changes), `crates/clax-cli/src/main.rs`, `crates/clax-cli/src/commands/mod.rs`, `crates/clax-cli/src/commands/init.rs`, `crates/clax-cli/src/commands/doctor.rs`
- Modify: `docs/contract.md` ("Installation and the wrapper"), `plugins/claude-code/README.md`

**Interfaces:**
- Consumes: Task 5's `HOST_NAME`, `extension_origin()`; Task 7's `clax native-host`; Task 8's `web/dist-extension/`; `crate::plugins::files()` (the wrapper copy).
- Produces:
  - `clax extension install | uninstall | status` (`--json` as every command).
  - `commands::extension::{install(home: &Home) -> anyhow::Result<Value>, uninstall(home: &Home) -> anyhow::Result<Value>, status(home: &Home) -> Value}`.
  - `extension_files::{files() -> Vec<(String, Vec<u8>)>, host_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<HostDir>, HostDir {browser: String, dir: PathBuf}}`.
  - `clax init` output gains `extension`; `clax uninit` removes it and revokes credentials; `clax doctor` gains the `extension` check.

- [ ] **Step 1: Write the failing tests**

`crates/clax-cli/tests/extension.rs`:

```rust
//! `clax extension install|uninstall|status` against scratch browser
//! directories (`CLAX_NATIVE_HOST_DIRS`) and a fixture build
//! (`CLAX_EXTENSION_DIST`); real browser profiles are never touched.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("icons")).unwrap();
        std::fs::write(dist.join("manifest.json"), clax_core::extension::MANIFEST).unwrap();
        std::fs::write(dist.join("sw.js"), "export {}").unwrap();
        std::fs::write(dist.join("icons/16.png"), [137u8, 80, 78, 71]).unwrap();
        for b in ["chrome", "brave"] {
            std::fs::create_dir_all(dir.path().join(b)).unwrap();
        }
        Env { dir }
    }
    fn p(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }
    fn cmd(&self, args: &[&str]) -> Value {
        let out = Command::cargo_bin("clax")
            .unwrap()
            .env("HOME", self.dir.path())
            .env("CLAX_HOME", self.p("ax"))
            .env("CLAX_EXTENSION_DIST", self.p("dist"))
            .env(
                "CLAX_NATIVE_HOST_DIRS",
                format!("chrome={}:brave={}:edge={}", self.p("chrome").display(), self.p("brave").display(), self.p("edge").display()),
            )
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn host_manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("dev.empathic.clax.json")).unwrap()).unwrap()
}

#[test]
fn install_writes_the_extension_the_launcher_and_a_manifest_per_existing_browser() {
    let e = Env::new();
    let out = e.cmd(&["extension", "install"]);
    assert!(e.p("ax/extension/manifest.json").exists());
    assert!(e.p("ax/extension/sw.js").exists());
    let launch = std::fs::read_to_string(e.p("ax/extension/host/launch.sh")).unwrap();
    assert!(launch.contains("exec native-host"), "{launch}");
    assert!(launch.contains(&format!("CLAX_HOME='{}'", e.p("ax").display())));
    assert!(e.p("ax/extension/host/ensure-clax.sh").exists());
    for b in ["chrome", "brave"] {
        let m = host_manifest(&e.p(b));
        assert_eq!(m["name"], "dev.empathic.clax");
        assert_eq!(m["type"], "stdio");
        assert_eq!(m["path"], e.p("ax/extension/host/launch.sh").display().to_string());
        assert_eq!(m["allowed_origins"], serde_json::json!([format!("{}/", clax_core::extension::extension_origin())]));
    }
    assert!(!e.p("edge").exists(), "a browser that is not installed is left alone");
    assert!(out["load_unpacked"].as_str().unwrap().contains("chrome://extensions"));
    let st = e.cmd(&["extension", "status"]);
    assert_eq!(st["files"], "current");
    assert_eq!(st["hosts"].as_array().unwrap().iter().filter(|h| h["status"] == "installed").count(), 2);
}

#[test]
fn uninstall_removes_only_what_install_wrote() {
    let e = Env::new();
    e.cmd(&["extension", "install"]);
    std::fs::create_dir_all(e.p("edge")).unwrap();
    std::fs::write(e.p("edge/dev.empathic.clax.json"), r#"{"name":"dev.empathic.clax","path":"/elsewhere/launch.sh"}"#).unwrap();
    e.cmd(&["extension", "uninstall"]);
    assert!(!e.p("ax/extension").exists());
    assert!(!e.p("chrome/dev.empathic.clax.json").exists());
    assert!(e.p("edge/dev.empathic.clax.json").exists(), "another install's manifest is kept");
}

#[test]
fn reinstalling_drops_files_the_new_build_lacks() {
    let e = Env::new();
    e.cmd(&["extension", "install"]);
    std::fs::write(e.p("ax/extension/old.js"), "x").unwrap();
    e.cmd(&["extension", "install"]);
    assert!(!e.p("ax/extension/old.js").exists());
}
```

Add to `crates/clax-cli/tests/init.rs` a test that `clax init` with the `CLAX_EXTENSION_DIST` and `CLAX_NATIVE_HOST_DIRS` of the fixture above reports `extension.status == "installed"` and that `clax uninit` reports `extension.status == "removed"`.

Unit test in `crates/clax-cli/src/extension_files.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_directories_follow_each_os() {
        let env = |k: &str| match k {
            "HOME" => Some("/h".to_string()),
            _ => None,
        };
        let dirs = host_dirs(&env);
        let paths: Vec<String> = dirs.iter().map(|d| d.dir.display().to_string()).collect();
        if cfg!(target_os = "macos") {
            assert!(paths.contains(&"/h/Library/Application Support/Google/Chrome/NativeMessagingHosts".to_string()));
            assert!(paths.contains(&"/h/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts".to_string()));
        } else {
            assert!(paths.contains(&"/h/.config/google-chrome/NativeMessagingHosts".to_string()));
            assert!(paths.contains(&"/h/.config/chromium/NativeMessagingHosts".to_string()));
        }
        let over = |k: &str| (k == "CLAX_NATIVE_HOST_DIRS").then(|| "chrome=/a:brave=/b".to_string());
        assert_eq!(host_dirs(&over).len(), 2);
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p clax-cli --test extension`
Expected: FAIL: `extension` is not a subcommand.

- [ ] **Step 3: Implement the files and directories**

`crates/clax-cli/src/extension_files.rs`:

```rust
//! The Chrome extension built into this binary (`web/dist-extension`), and
//! where each supported browser looks for native messaging hosts (spec
//! 2026-10-05 §5.4).

use rust_embed::RustEmbed;
use std::path::PathBuf;

#[derive(RustEmbed)]
#[folder = "../../web/dist-extension/"]
#[exclude = ".gitkeep"]
struct Dist;

/// Every file of the extension: (path, contents). A debug build reads
/// `CLAX_EXTENSION_DIST` instead when it is set (the tests' fixture).
pub fn files() -> Vec<(String, Vec<u8>)> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("CLAX_EXTENSION_DIST") {
        return walk(&PathBuf::from(dir));
    }
    Dist::iter()
        .map(|p| {
            let f = Dist::get(&p).expect("an embedded file lists itself");
            (p.to_string(), f.data.into_owned())
        })
        .collect()
}

#[cfg(debug_assertions)]
fn walk(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let (Ok(rel), Ok(bytes)) = (p.strip_prefix(root), std::fs::read(&p)) {
                out.push((rel.to_string_lossy().replace('\\', "/"), bytes));
            }
        }
    }
    out.sort();
    out
}

/// One browser's native messaging hosts directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDir {
    pub browser: String,
    pub dir: PathBuf,
}

const MAC: &[(&str, &str)] = &[
    ("chrome", "Google/Chrome"),
    ("chrome-beta", "Google/Chrome Beta"),
    ("chrome-dev", "Google/Chrome Dev"),
    ("chrome-canary", "Google/Chrome Canary"),
    ("chromium", "Chromium"),
    ("brave", "BraveSoftware/Brave-Browser"),
    ("edge", "Microsoft Edge"),
];
const LINUX: &[(&str, &str)] = &[
    ("chrome", "google-chrome"),
    ("chrome-beta", "google-chrome-beta"),
    ("chrome-dev", "google-chrome-unstable"),
    ("chromium", "chromium"),
    ("brave", "BraveSoftware/Brave-Browser"),
    ("edge", "microsoft-edge"),
];

/// Every browser's hosts directory, whether or not the browser is
/// installed (`install` writes only where the profile directory, the
/// directory's parent, exists). `CLAX_NATIVE_HOST_DIRS`
/// (`<browser>=<dir>:<browser>=<dir>…`) replaces the table, and then the
/// directories themselves stand for the profiles.
pub fn host_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<HostDir> {
    if let Some(list) = env("CLAX_NATIVE_HOST_DIRS") {
        return list
            .split(':')
            .filter_map(|e| e.split_once('='))
            .map(|(b, d)| HostDir { browser: b.to_string(), dir: PathBuf::from(d) })
            .collect();
    }
    let home = PathBuf::from(env("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        let base = home.join("Library/Application Support");
        MAC.iter().map(|(b, d)| HostDir { browser: b.to_string(), dir: base.join(d).join("NativeMessagingHosts") }).collect()
    } else {
        let base = env("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        LINUX.iter().map(|(b, d)| HostDir { browser: b.to_string(), dir: base.join(d).join("NativeMessagingHosts") }).collect()
    }
}
```

In `build.rs`, add `"web/dist-extension"` to `DIRS` so a rebuilt extension rebuilds the binary.

- [ ] **Step 4: Implement the command**

`crates/clax-cli/src/commands/extension.rs`:

```rust
//! `clax extension install | uninstall | status` (spec 2026-10-05 §6.6):
//! the unpacked extension under `<home>/extension`, its native host
//! launcher, and a host manifest for each installed browser.

use crate::extension_files::{HostDir, files, host_dirs};
use clax_core::Home;
use clax_core::extension::{HOST_NAME, extension_origin};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Write the extension to <home>/extension and register its native host with each installed browser.
    Install,
    /// Remove what install wrote.
    Uninstall,
    /// Report the extension's files and each browser's host registration.
    Status,
}

const LOAD_UNPACKED: &str = "Chrome: open chrome://extensions, turn on Developer mode, choose Load unpacked, and pick";

fn ext_dir(home: &Home) -> PathBuf {
    home.root().join("extension")
}
fn launcher(home: &Home) -> PathBuf {
    ext_dir(home).join("host/launch.sh")
}
fn manifest_file(d: &HostDir) -> PathBuf {
    d.dir.join(format!("{HOST_NAME}.json"))
}
fn env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}
/// Quotes `s` for a POSIX shell's single quotes.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
/// The profile directory a hosts directory lives in, which tells whether
/// the browser is installed; with `CLAX_NATIVE_HOST_DIRS`, the directory itself.
fn profile_of(d: &HostDir) -> PathBuf {
    if env("CLAX_NATIVE_HOST_DIRS").is_some() { d.dir.clone() } else { d.dir.parent().map(Path::to_path_buf).unwrap_or_default() }
}

fn write_exec(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

pub fn install(home: &Home) -> anyhow::Result<Value> {
    let fs = files();
    if !fs.iter().any(|(p, _)| p == "manifest.json") {
        anyhow::bail!("this clax was built without the extension (run scripts/build-web.sh, then rebuild)");
    }
    let dir = ext_dir(home);
    let staging = home.root().join(format!(".extension-{}", clax_core::new_ulid()));
    for (p, bytes) in &fs {
        let dest = staging.join(p);
        std::fs::create_dir_all(dest.parent().expect("a file has a parent"))?;
        std::fs::write(dest, bytes)?;
    }
    std::fs::create_dir_all(staging.join("host"))?;
    let wrapper = crate::plugins::files()
        .into_iter()
        .find(|(p, _)| p == "plugins/claude-code/scripts/ensure-clax.sh")
        .map(|(_, b)| b)
        .ok_or_else(|| anyhow::anyhow!("the plugins' wrapper is missing from this build"))?;
    write_exec(&staging.join("host/ensure-clax.sh"), &wrapper)?;
    let launch = launcher(home);
    write_exec(
        &staging.join("host/launch.sh"),
        format!(
            "#!/bin/sh\n# Launches the Clax native messaging host for the Clax Chrome extension.\nCLAX_HOME={home}\nexport CLAX_HOME\nexec {wrapper} exec native-host \"$@\"\n",
            home = sh_quote(&home.root().display().to_string()),
            wrapper = sh_quote(&dir.join("host/ensure-clax.sh").display().to_string()),
        )
        .as_bytes(),
    )?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::rename(&staging, &dir)?;
    let host = json!({
        "name": HOST_NAME,
        "description": "Clax: pairs the Clax extension with the local Clax daemon",
        "path": launch.display().to_string(),
        "type": "stdio",
        "allowed_origins": [format!("{}/", extension_origin())],
    });
    let mut hosts = Vec::new();
    let mut written = Vec::new();
    for d in host_dirs(&env) {
        if !profile_of(&d).is_dir() {
            hosts.push(json!({"browser": d.browser, "status": "skipped", "detail": "not installed"}));
            continue;
        }
        std::fs::create_dir_all(&d.dir)?;
        std::fs::write(manifest_file(&d), serde_json::to_vec_pretty(&host)?)?;
        written.push(manifest_file(&d).display().to_string());
        hosts.push(json!({"browser": d.browser, "status": "installed", "path": manifest_file(&d)}));
    }
    std::fs::write(
        dir.join("installed.json"),
        serde_json::to_vec_pretty(&json!({"version": env!("CARGO_PKG_VERSION"), "hosts": written}))?,
    )?;
    Ok(json!({
        "status": "installed",
        "dir": dir,
        "hosts": hosts,
        "load_unpacked": format!("{LOAD_UNPACKED} {} (once).", dir.display()),
    }))
}

pub fn uninstall(home: &Home) -> anyhow::Result<Value> {
    let dir = ext_dir(home);
    let launch = launcher(home).display().to_string();
    let recorded: Vec<String> = std::fs::read(dir.join("installed.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| serde_json::from_value(v["hosts"].clone()).ok())
        .unwrap_or_default();
    let mut removed = Vec::new();
    for path in recorded {
        let ours = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|m| m["path"] == launch.as_str());
        if ours && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    let had = dir.exists();
    if had {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(json!({
        "status": if had { "removed" } else { "absent" },
        "hosts_removed": removed,
        "note": "Remove the unpacked Clax extension at chrome://extensions too.",
    }))
}

pub fn status(home: &Home) -> Value {
    let dir = ext_dir(home);
    let launch = launcher(home).display().to_string();
    let files_state = if !dir.join("manifest.json").exists() {
        "missing"
    } else if files().iter().all(|(p, b)| std::fs::read(dir.join(p)).ok().as_deref() == Some(b.as_slice())) {
        "current"
    } else {
        "stale"
    };
    let hosts: Vec<Value> = host_dirs(&env)
        .iter()
        .filter(|d| profile_of(d).is_dir())
        .map(|d| {
            let m = std::fs::read(manifest_file(d)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let status = match &m {
                None => "missing",
                Some(m) if m["path"] == launch.as_str() && m["allowed_origins"][0] == format!("{}/", extension_origin()).as_str() => "installed",
                Some(_) => "stale",
            };
            json!({"browser": d.browser, "status": status, "path": manifest_file(d)})
        })
        .collect();
    json!({"dir": dir, "files": files_state, "hosts": hosts})
}

pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let out = match cmd {
        Cmd::Install => install(home)?,
        Cmd::Uninstall => uninstall(home)?,
        Cmd::Status => status(home),
    };
    super::print(cli, out, |j| serde_json::to_string_pretty(j).unwrap_or_default())
}
```

(Give `print`'s text form proper lines: the status, then one line per browser, then `load_unpacked`.) Register `#[command(subcommand)] Extension(commands::extension::Cmd)` in `Cmd` ("Install, remove or check the Clax Chrome extension and its native messaging host.") and dispatch it.

- [ ] **Step 5: `init`, `uninit` and `doctor`**

In `init.rs::run`, after the marketplace step:

```rust
    let extension = if install {
        super::extension::install(home).unwrap_or_else(|e| json!({"status": "failed", "detail": format!("{e:#}")}))
    } else {
        let revoked = revoke_extension_credentials(home);
        let mut v = super::extension::uninstall(home).unwrap_or_else(|e| json!({"status": "failed", "detail": format!("{e:#}")}));
        v["credentials_revoked"] = revoked;
        v
    };
```

with

```rust
/// Revokes the extension's credentials: through the daemon when one runs
/// (so its cache forgets them), else in the store directly.
fn revoke_extension_credentials(home: &Home) -> Value {
    if let Some(c) = crate::client::Client::discover(home) {
        return c.delete("/api/extension/credentials").map(|_| json!("revoked")).unwrap_or_else(|e| json!(format!("failed: {e:#}")));
    }
    if !home.db_path().exists() {
        return json!(0);
    }
    match clax_core::Store::open(home).and_then(|st| st.revoke_extension_credentials()) {
        Ok(n) => json!(n),
        Err(e) => json!(format!("failed: {e}")),
    }
}
```

and add `"extension": extension` to the printed object, with the text line `extension: <status>` and, after install, the `load_unpacked` line. A failed extension step is reported and never fails `init`. In `doctor.rs::run`, push:

```rust
    let ext = super::extension::status(home);
    let ok = ext["files"] == "current" && ext["hosts"].as_array().is_some_and(|h| !h.is_empty() && h.iter().all(|h| h["status"] == "installed"));
    checks.push(if ok {
        check("extension", true, format!("{} ({} browser(s))", ext["dir"].as_str().unwrap_or(""), ext["hosts"].as_array().map_or(0, Vec::len)))
    } else {
        warn("extension", format!("not set up for Chrome: run `clax extension install` (or /clax:extension in Claude Code); {ext}"))
    });
```

`plugins/claude-code/commands/extension.md`:

```markdown
---
description: Set up the Clax Chrome extension, to comment on any web page (such as your dev server)
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*)
---

## Context

- Install: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec extension install --json`

## Your task

Tell the user the result above: which browsers got the native messaging host, and the `load_unpacked` instruction, which they follow once in Chrome. If it failed, explain the `detail`. Then remind them to click the Clax toolbar button on their dev server's tab to start commenting, and that you can `watch` that URL to receive their comments.
```

Run `scripts/test-plugins.sh` and add the command wherever it checks the plugin's command list. Document `clax extension`, the host manifests and `/clax:extension` in `docs/contract.md` ("Installation and the wrapper") and the Claude Code plugin README, which also says that Snap and Flatpak Chromium on Linux cannot run the native host.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p clax-cli && scripts/test-plugins.sh`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/clax-cli plugins/claude-code docs/contract.md
git -c commit.gpgsign=false commit -m "Install the Chrome extension and its native host with clax init"
```

---

### Task 10: The service worker: pairing, the API, origins and the stream

**Files:**
- Create: `web/extension/src/sw/pairing.ts`, `web/extension/src/sw/pairing.test.ts`
- Create: `web/extension/src/sw/api.ts`, `web/extension/src/sw/api.test.ts`
- Create: `web/extension/src/sw/origins.ts`, `web/extension/src/sw/origins.test.ts`
- Create: `web/extension/src/sw/tabs.ts`, `web/extension/src/sw/tabs.test.ts`
- Modify: `web/extension/src/sw/main.ts`, `web/extension/src/content/loader.ts`

**Interfaces:**
- Consumes: Task 8's `messages.ts` and `fake-chrome.ts`; `web/shell/src/stream-hub.ts` (`Hub`, `HubEnv`, `TabMsg`, `HubMsg`); `web/shell/src/view/deltas.ts` (`applyThread`, `ThreadDelta`); Task 7's native host protocol; Task 6's gateway routes.
- Produces:
  - `pairing.ts`: `HOST = "dev.empathic.clax"`, `REPAIR_MS = 10_000`, `type Pairing = {daemon, credential, claxVersion}`, `class PairError(code, message)`, `interface PairEnv`, `class Pairer {current(): Promise<Pairing>; pair(): Promise<Pairing>; forget(): Promise<void>}`.
  - `api.ts`: `class ApiFailure(code, message, status)`, `class Api(pairer, fetchFn?)` with `request(path, init?)`, `json<T>(path, init?)`, `lookup(url)`, `artifact(aid)`, `threads(aid)`, `working(aid)`, `postThread(form)`, `postSnapshot(form)`, `comment(aid, tid, body)`, `sendThread(aid, tid, to)`, `sendBatch(aid, ids, note, to)`, `resolve(aid, tid)`, `reopen(aid, tid)`, `remove(aid, tid)`, `me()` (the owner viewer), `setName(name)` (the owner's name), `looked(aid, ids)` (the owner's marks), `presence(aid)` (reports the owner `here`; never `away`, spec §9.5).
  - `origins.ts`: `originOf(url) -> string | null`, `patternOf(origin)`, `scriptId(origin)`, `ask(env, origin): Promise<boolean>` (call it synchronously inside the gesture), `remember(env, origin)`, `forget(env, origin)`, `injectOverlay(env, tabId)`, `type OriginsEnv`.
  - `tabs.ts`: `class Tabs` with `state(tabId): TabState`, `hello(tabId, url)`, `route(tabId, url)`, `toggle(tabId, url)`, `fromHub(ids, msg)`, `fromOverlay(tabId, windowId, m)`, `attachPanel(port, onMessage: (tabId: number | null, m: unknown) => void)`, `panelState(tabId): PanelState`; `emptyTab(tabId, url): TabState`; `applyEvent(s: TabState, name: string, data: Record<string, unknown>): TabState`.
  - The worker's test hook (test build only): `globalThis.claxTest = {comment(tabId, url), state(tabId), pairer}`.

- [ ] **Step 1: Write the failing tests**

`web/extension/src/sw/pairing.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { PairError, Pairer, REPAIR_MS, type PairEnv } from "./pairing";

const CRED = `cxe_${"A".repeat(43)}`;
function env(replies: unknown[], version = "0.9.0") {
  const store: Record<string, unknown> = {};
  const local: Record<string, unknown> = {};
  let now = 0;
  const e = {
    sent: [] as unknown[], reloads: 0,
    tick(ms: number) { now += ms; },
    sendNative: async (_h: string, m: object) => { e.sent.push(m); return replies.shift(); },
    session: { get: async (k: string) => (k in store ? { [k]: store[k] } : {}), set: async (v: Record<string, unknown>) => { Object.assign(store, v); }, remove: async (k: string) => { delete store[k]; } },
    local: { get: async (k: string) => (k in local ? { [k]: local[k] } : {}), set: async (v: Record<string, unknown>) => { Object.assign(local, v); }, remove: async (k: string) => { delete local[k]; } },
    manifestVersion: version,
    reload: () => { e.reloads++; },
    now: () => now,
  };
  return e;
}
const paired = (daemon = "http://localhost:7480", v = "0.9.0") => ({ type: "paired", v: 1, daemon, credential: CRED, clax_version: v });

describe("Pairer", () => {
  it("pairs once and keeps the pairing in session storage", async () => {
    const e = env([paired()]);
    const p = new Pairer(e as unknown as PairEnv);
    expect((await p.current()).daemon).toBe("http://localhost:7480");
    expect((await p.current()).credential).toBe(CRED);
    expect(e.sent).toEqual([{ type: "pair", v: 1, extension_version: "0.9.0" }]);
  });

  it("shares one pairing between concurrent callers and refuses another within REPAIR_MS", async () => {
    const e = env([paired(), paired("http://localhost:7481")]);
    const p = new Pairer(e as unknown as PairEnv);
    const [a, b] = await Promise.all([p.pair(), p.pair()]);
    expect(a).toEqual(b);
    await expect(p.pair()).rejects.toMatchObject({ code: "paired_recently" });
    e.tick(REPAIR_MS);
    expect((await p.pair()).daemon).toBe("http://localhost:7481");
  });

  it("reports the host's errors and refuses replies that are not a pairing", async () => {
    const e = env([{ type: "error", v: 1, code: "daemon_unavailable", message: "see the log" }, { ...paired(), daemon: "http://evil.example:80" }]);
    const p = new Pairer(e as unknown as PairEnv);
    await expect(p.pair()).rejects.toEqual(new PairError("daemon_unavailable", "see the log"));
    e.tick(REPAIR_MS);
    await expect(p.pair()).rejects.toMatchObject({ code: "bad_reply" });
  });

  it("reloads the extension once for a daemon of another version", async () => {
    const e = env([paired(undefined, "1.0.0"), paired(undefined, "1.0.0")]);
    const p = new Pairer(e as unknown as PairEnv);
    await p.pair();
    e.tick(REPAIR_MS);
    await p.pair();
    expect(e.reloads).toBe(1);
  });
});
```

`web/extension/src/sw/api.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { Api, ApiFailure } from "./api";
import type { Pairing } from "./pairing";

const A: Pairing = { daemon: "http://localhost:7480", credential: `cxe_${"A".repeat(43)}`, claxVersion: "0.9.0" };
const B: Pairing = { daemon: "http://localhost:7481", credential: `cxe_${"B".repeat(43)}`, claxVersion: "0.9.0" };

function setup(results: (Response | Error)[]) {
  const calls: { url: string; init: RequestInit }[] = [];
  let pairs = 0;
  const pairer = { current: async () => A, pair: async () => { pairs++; return B; } };
  const fetchFn = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    const r = results.shift()!;
    if (r instanceof Error) throw r;
    return r;
  }) as unknown as typeof fetch;
  return { api: new Api(pairer, fetchFn), calls, pairs: () => pairs };
}
const ok = () => new Response(JSON.stringify({ page: null, route: null }), { status: 200 });

describe("Api", () => {
  it("sends the credential and never cookies", async () => {
    const s = setup([ok()]);
    await s.api.lookup("http://localhost:5173/");
    expect(s.calls[0].url).toBe("http://localhost:7480/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F");
    expect(new Headers(s.calls[0].init.headers).get("authorization")).toBe(`Clax-Extension ${A.credential}`);
    expect(s.calls[0].init.credentials).toBe("omit");
  });

  it("re-pairs once on 401 and on a network error", async () => {
    let s = setup([new Response("{}", { status: 401 }), ok()]);
    await s.api.lookup("http://localhost:5173/");
    expect(s.pairs()).toBe(1);
    expect(s.calls[1].url.startsWith(B.daemon)).toBe(true);
    expect(new Headers(s.calls[1].init.headers).get("authorization")).toBe(`Clax-Extension ${B.credential}`);
    s = setup([new TypeError("Failed to fetch"), ok()]);
    await s.api.lookup("http://localhost:5173/");
    expect(s.pairs()).toBe(1);
    s = setup([new Response("{}", { status: 401 }), new Response(JSON.stringify({ error: { code: "unknown_credential", message: "pair" } }), { status: 401 })]);
    await expect(s.api.lookup("http://x/")).rejects.toEqual(new ApiFailure("unknown_credential", "pair", 401));
    expect(s.pairs()).toBe(1);
  });

  it("refuses artifact and thread IDs that are not IDs", async () => {
    const s = setup([]);
    await expect(s.api.threads("../token")).rejects.toMatchObject({ code: "invalid_id" });
    await expect(s.api.resolve("7q3k9mzx2b4t", "x/../y")).rejects.toMatchObject({ code: "invalid_id" });
  });
});
```

`web/extension/src/sw/origins.test.ts`:

```ts
import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { ask, forget, injectOverlay, originOf, remember, scriptId, type OriginsEnv } from "./origins";

let c: FakeChrome;
const env = () => ({ permissions: c.permissions, scripting: c.scripting, local: c.storage.local }) as unknown as OriginsEnv;
beforeEach(() => { c = fakeChrome(); });

describe("origins", () => {
  it("reads only http and https origins", () => {
    expect(originOf("http://localhost:5173/a?b#c")).toBe("http://localhost:5173");
    expect(originOf("chrome://extensions")).toBeNull();
    expect(originOf("file:///x")).toBeNull();
    expect(originOf("nonsense")).toBeNull();
  });

  it("asks for the origin, registers the loader once, and forgets both", async () => {
    expect(await ask(env(), "http://localhost:5173")).toBe(true);
    await remember(env(), "http://localhost:5173");
    const reg = c.calls.find(x => x.api === "scripting.registerContentScripts")!.args[0] as chrome.scripting.RegisteredContentScript[];
    expect(reg[0]).toMatchObject({ id: scriptId("http://localhost:5173"), matches: ["http://localhost:5173/*"], js: ["loader.js"], runAt: "document_idle", allFrames: false, persistAcrossSessions: true });
    expect(c.storage.local.data.origins).toEqual(["http://localhost:5173"]);
    await forget(env(), "http://localhost:5173");
    expect(c.calls.some(x => x.api === "scripting.unregisterContentScripts")).toBe(true);
    expect(c.granted.has("http://localhost:5173/*")).toBe(false);
    expect(c.storage.local.data.origins).toEqual([]);
  });

  it("injects the overlay into the top frame only", async () => {
    await injectOverlay(env(), 7);
    expect(c.calls.find(x => x.api === "scripting.executeScript")!.args[0]).toEqual({ target: { tabId: 7, allFrames: false }, files: ["overlay.js"] });
  });
});
```

`web/extension/src/sw/tabs.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { applyEvent, emptyTab } from "./tabs";

const thread = (id: string, body: string) => ({ id, artifact_id: "7q3k9mzx2b4t", status: "open", comment_count: 1, last_comment: { id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "A", via_harness: null, body, created_at: "t" }, anchor: { kind: "element", selector: "body", file: "index.html" } });

describe("applyEvent", () => {
  it("applies thread deltas, deletions, feedback states and working lists", () => {
    let s = emptyTab(1, "http://localhost:5173/");
    s = applyEvent(s, "thread", { thread: thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "one") });
    expect(s.threads).toHaveLength(1);
    s = applyEvent(s, "feedback_state", { thread_id: "01J9AAAAAAAAAAAAAAAAAAAAAA", state: "delivered", tier: "wait", since: "t", resends: 0, exhausted: false });
    expect(s.threads[0].feedback_state?.state).toBe("delivered");
    s = applyEvent(s, "working", { working: [{ key: "k", harness: "claude", message: null, thread_ids: [], started_at: "t", last_heartbeat: "t" }] });
    expect(s.working).toHaveLength(1);
    s = applyEvent(s, "thread_deleted", { thread_id: "01J9AAAAAAAAAAAAAAAAAAAAAA" });
    expect(s.threads).toHaveLength(0);
  });

  it("marks the tab pending while an open thread waits for a snapshot", () => {
    let s = emptyTab(1, "http://localhost:5173/");
    s = applyEvent(s, "thread", { thread: { ...thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "x"), addressed_pending: { harness: "claude", at: "t" } } });
    expect(s.pending).toBe(true);
  });
});
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cd web && npx vitest run extension/src/sw`
Expected: FAIL: cannot resolve `./pairing`, `./api`, `./origins`, `./tabs`.

- [ ] **Step 3: Implement `pairing.ts`**

```ts
// Pairing with the daemon through the native host (spec 2026-10-05 §9.1).
// The pairing (the daemon's URL and the credential) lives only in
// chrome.storage.session, in memory and out of content scripts' reach. A
// daemon of another Clax version reloads the extension once for that
// version: `clax extension install` has rewritten the unpacked files.
export const HOST = "dev.empathic.clax";
/** The least time between two pairings a failure asked for. */
export const REPAIR_MS = 10_000;

export type Pairing = { daemon: string; credential: string; claxVersion: string };

export class PairError extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void>; remove(k: string): Promise<void> };
export interface PairEnv {
  sendNative(host: string, msg: object): Promise<unknown>;
  session: Area;
  local: Area;
  manifestVersion: string;
  reload(): void;
  now(): number;
}

const numeric = (v: string) => /^\d+\.\d+\.\d+/.exec(v)?.[0] ?? v;

function parse(reply: unknown): Pairing {
  const r = (typeof reply === "object" && reply !== null ? reply : {}) as Record<string, unknown>;
  if (r.type === "paired" && r.v === 1 && typeof r.daemon === "string" && /^http:\/\/(localhost|127\.0\.0\.1):\d{1,5}$/.test(r.daemon)
    && typeof r.credential === "string" && /^cxe_[A-Za-z0-9_-]{43}$/.test(r.credential) && typeof r.clax_version === "string") {
    return { daemon: r.daemon, credential: r.credential, claxVersion: r.clax_version };
  }
  if (r.type === "error" && typeof r.code === "string") throw new PairError(r.code, typeof r.message === "string" ? r.message : r.code);
  throw new PairError("bad_reply", "the native host answered something other than a pairing");
}

export class Pairer {
  private inflight: Promise<Pairing> | null = null;
  private last = Number.NEGATIVE_INFINITY;
  constructor(private readonly env: PairEnv) {}

  /** The stored pairing, else a new one. */
  async current(): Promise<Pairing> {
    const stored = (await this.env.session.get("pairing")).pairing as Pairing | undefined;
    return stored ?? this.pair();
  }

  /** A new pairing; concurrent callers share it, and another within
   * `REPAIR_MS` of the last is refused (`paired_recently`). */
  pair(): Promise<Pairing> {
    if (this.inflight) return this.inflight;
    if (this.env.now() - this.last < REPAIR_MS) return Promise.reject(new PairError("paired_recently", "Clax paired a moment ago; try again shortly"));
    this.last = this.env.now();
    this.inflight = (async () => {
      try {
        const p = parse(await this.env.sendNative(HOST, { type: "pair", v: 1, extension_version: this.env.manifestVersion }));
        await this.env.session.set({ pairing: p });
        await this.reloadFor(p.claxVersion);
        return p;
      } finally {
        this.inflight = null;
      }
    })();
    return this.inflight;
  }

  async forget(): Promise<void> {
    await this.env.session.remove("pairing");
  }

  private async reloadFor(v: string): Promise<void> {
    if (numeric(v) === numeric(this.env.manifestVersion)) return;
    if ((await this.env.local.get("reloadedFor")).reloadedFor === numeric(v)) return;
    await this.env.local.set({ reloadedFor: numeric(v) });
    this.env.reload();
  }
}
```

- [ ] **Step 4: Implement `api.ts`**

```ts
// The daemon's API as the extension uses it (spec 2026-10-05 §9.2): every
// request carries the credential and no cookie; a 401 or an unreachable
// daemon pairs again and retries once.
import type { Thread, Viewer } from "../../../shell/src/threads";
import type { PageView } from "../messages";
import type { Pairer } from "./pairing";

export class ApiFailure extends Error {
  constructor(readonly code: string, message: string, readonly status = 0) { super(message); }
}

const AID = /^[0-9a-z]{12}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
function ids(aid: string, tid?: string): void {
  if (!AID.test(aid) || (tid !== undefined && !ULID.test(tid))) throw new ApiFailure("invalid_id", "not an artifact or thread ID");
}

export class Api {
  constructor(private readonly pairer: Pick<Pairer, "current" | "pair">, private readonly fetchFn: typeof fetch = (...a) => fetch(...a)) {}

  async request(path: string, init: RequestInit = {}): Promise<Response> {
    let p = await this.pairer.current();
    for (let attempt = 0; ; attempt++) {
      const headers = new Headers(init.headers);
      headers.set("authorization", `Clax-Extension ${p.credential}`);
      try {
        const res = await this.fetchFn(p.daemon + path, { ...init, headers, credentials: "omit" });
        if (res.status !== 401 || attempt > 0) return res;
      } catch (e) {
        if (attempt > 0 || init.signal?.aborted) throw new ApiFailure("daemon_unreachable", String(e));
      }
      p = await this.pairer.pair();
    }
  }

  async json<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await this.request(path, init);
    const body = await res.json().catch(() => null) as { error?: { code?: string; message?: string } } | null;
    if (!res.ok) throw new ApiFailure(body?.error?.code ?? `http_${res.status}`, body?.error?.message ?? res.statusText, res.status);
    return body as T;
  }

  private send<T>(method: string, path: string, body?: unknown): Promise<T> {
    return this.json<T>(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  }

  lookup(url: string) { return this.json<{ page: PageView | null; route: string | null }>(`/api/live/pages?url=${encodeURIComponent(url)}`); }
  async artifact(aid: string) { ids(aid); return this.json<{ artifact: Record<string, unknown>; versions: unknown[] }>(`/api/artifacts/${aid}`); }
  async threads(aid: string) { ids(aid); return (await this.json<{ threads: Thread[] }>(`/api/artifacts/${aid}/threads?include_resolved=true&limit=200`)).threads; }
  async working(aid: string) { ids(aid); return (await this.json<{ working: unknown[] }>(`/api/artifacts/${aid}/working`)).working; }
  postThread(form: FormData) { return this.json<{ thread: Thread; page: PageView; version: number; clip_error?: string }>("/api/live/threads", { method: "POST", body: form }); }
  postSnapshot(form: FormData) { return this.json<{ page: PageView; version: number; linked: string[] }>("/api/live/snapshots", { method: "POST", body: form }); }
  async comment(aid: string, tid: string, body: string) { ids(aid, tid); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/comments`, { body }); }
  async sendThread(aid: string, tid: string, to: string | null) { ids(aid, tid); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/send`, to ? { to } : {}); }
  async sendBatch(aid: string, threadIds: string[], note: string | null, to: string | null) { ids(aid); threadIds.forEach(t => ids(aid, t)); return this.send<{ threads: Thread[] }>("POST", `/api/artifacts/${aid}/threads:send`, { thread_ids: threadIds, note, to }); }
  async resolve(aid: string, tid: string) { ids(aid, tid); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/resolve`, {}); }
  async reopen(aid: string, tid: string) { ids(aid, tid); return this.send<{ thread: Thread }>("POST", `/api/artifacts/${aid}/threads/${tid}/reopen`, {}); }
  async remove(aid: string, tid: string) { ids(aid, tid); return this.send<unknown>("DELETE", `/api/artifacts/${aid}/threads/${tid}`); }
  me() { return this.json<{ viewer: Viewer }>("/api/viewers/me"); }
  setName(name: string) { return this.send<{ viewer: Viewer }>("PUT", "/api/viewers/me", { display_name: name }); }
  async looked(aid: string, threadIds: string[]) { ids(aid); return this.send<unknown>("PUT", "/api/viewers/me/looked", { artifact_id: aid, thread_ids: threadIds }); }
  // Only `here`: the extension is the owner, and an `away` from it would mark the owner away in a shell tab too (spec §9.5).
  async presence(aid: string) { ids(aid); return this.send<unknown>("PUT", "/api/viewers/me/presence", { artifact_id: aid, state: "here" }); }
}
```

- [ ] **Step 5: Implement `origins.ts`**

```ts
// Turning Clax on per origin (spec 2026-10-05 O4, L8): the origin's
// optional host permission (Chrome's own prompt, none when it is held), a
// content script registered for it that survives restarts, and the
// overlay injected into the tab at once.
export type OriginsEnv = {
  permissions: Pick<typeof chrome.permissions, "request" | "remove">;
  scripting: Pick<typeof chrome.scripting, "registerContentScripts" | "unregisterContentScripts" | "getRegisteredContentScripts" | "executeScript">;
  local: { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
};

export function originOf(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.origin : null;
  } catch {
    return null;
  }
}
export const patternOf = (origin: string) => `${origin}/*`;
export const scriptId = (origin: string) => `clax-loader-${[...new TextEncoder().encode(origin)].map(b => b.toString(16).padStart(2, "0")).join("")}`;

/** Asks for the origin's permission. Call it before any `await` in the
 * gesture's handler, so the gesture still holds. */
export function ask(env: OriginsEnv, origin: string): Promise<boolean> {
  return env.permissions.request({ origins: [patternOf(origin)] }).catch(() => false);
}

async function list(env: OriginsEnv): Promise<string[]> {
  const v = (await env.local.get("origins")).origins;
  return Array.isArray(v) ? v.filter((o): o is string => typeof o === "string") : [];
}

/** Registers the loader for the origin (once) and records it. */
export async function remember(env: OriginsEnv, origin: string): Promise<void> {
  const id = scriptId(origin);
  const have = await env.scripting.getRegisteredContentScripts({ ids: [id] });
  if (!have.length) {
    await env.scripting.registerContentScripts([{ id, matches: [patternOf(origin)], js: ["loader.js"], runAt: "document_idle", allFrames: false, persistAcrossSessions: true }]);
  }
  const all = await list(env);
  if (!all.includes(origin)) await env.local.set({ origins: [...all, origin] });
}

/** "Turn off on this site": the loader, the permission and the record go. */
export async function forget(env: OriginsEnv, origin: string): Promise<void> {
  await env.scripting.unregisterContentScripts({ ids: [scriptId(origin)] }).catch(() => {});
  await env.permissions.remove({ origins: [patternOf(origin)] }).catch(() => false);
  await env.local.set({ origins: (await list(env)).filter(o => o !== origin) });
}

export async function injectOverlay(env: OriginsEnv, tabId: number): Promise<void> {
  await env.scripting.executeScript({ target: { tabId, allFrames: false }, files: ["overlay.js"] });
}
```

- [ ] **Step 6: Implement `tabs.ts`**

```ts
// What the worker knows about each tab with Clax (spec 2026-10-05 §9.4,
// §9.5): its URL, live page, route, threads, working list and comment mode;
// the stream's deltas applied to it; the overlay and the side panels told
// of every change.
import { type ThreadDelta, applyThread } from "../../../shell/src/view/deltas";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../../../shell/src/threads";
import type { Working } from "../../../shell/src/view/working-model";
import type { OverlayToWorker, PageView, PanelState, WorkerToOverlay, WorkerToPanel } from "../messages";
import type { Api } from "./api";

export type TabState = {
  tabId: number; url: string; page: PageView | null; route: string | null; threads: Thread[]; working: Working[];
  resolved: Record<string, AnchorResult>; commentMode: boolean; overlay: boolean; pending: boolean; selected: string | null;
  error: { code: string; message: string } | null;
};

export const emptyTab = (tabId: number, url: string): TabState => ({
  tabId, url, page: null, route: null, threads: [], working: [], resolved: {}, commentMode: false, overlay: false, pending: false, selected: null, error: null,
});

const pendingOf = (threads: Thread[]) => threads.some(t => t.status === "open" && !!(t as Thread & { addressed_pending?: unknown }).addressed_pending);

/** `s` with one stream event of its live page applied. */
export function applyEvent(s: TabState, name: string, data: Record<string, unknown>): TabState {
  let threads = s.threads;
  switch (name) {
    case "thread": threads = applyThread(threads, data.thread as ThreadDelta).threads; break;
    case "thread_deleted": threads = threads.filter(t => t.id !== data.thread_id); break;
    case "feedback_state": threads = threads.map(t => (t.id === data.thread_id ? { ...t, feedback_state: data as Thread["feedback_state"] } : t)); break;
    case "working": return { ...s, working: (data.working as Working[]) ?? [] };
    default: return s;
  }
  return { ...s, threads, pending: pendingOf(threads) };
}

type Deps = {
  api: Api;
  hub: { receive(id: string, msg: TabMsg): void };
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  inject(tabId: number): Promise<void>;
};

export class Tabs {
  private tabs = new Map<number, TabState>();
  private panels = new Map<string, { port: chrome.runtime.Port; tabId: number | null }>();
  constructor(private readonly d: Deps) {}

  state(tabId: number): TabState | undefined { return this.tabs.get(tabId); }

  private set(tabId: number, next: TabState): void {
    this.tabs.set(tabId, next);
    this.d.toOverlay(tabId, { t: "state", page: next.page, route: next.route, threads: next.threads, commentMode: next.commentMode, pending: next.pending });
    for (const [, p] of this.panels) if (p.tabId === tabId) p.port.postMessage({ t: "tab", state: this.panelState(tabId) } satisfies WorkerToPanel);
  }

  /** The page's URL changed (a load or a route change): look its live page up and follow its topics. */
  async route(tabId: number, url: string): Promise<TabState> {
    const prev = this.tabs.get(tabId) ?? emptyTab(tabId, url);
    try {
      const { page, route } = await this.d.api.lookup(url);
      const threads = page && page.artifact_id === prev.page?.artifact_id ? prev.threads : page ? await this.d.api.threads(page.artifact_id) : [];
      const next = { ...prev, url, page, route, threads, pending: pendingOf(threads), error: null };
      this.d.hub.receive(`tab:${tabId}`, { t: "topics", topics: page ? [`artifact:${page.artifact_id}`, `working:${page.artifact_id}`] : [] });
      this.set(tabId, next);
      return next;
    } catch (e) {
      const err = e as { code?: string; message?: string };
      const next = { ...prev, url, error: { code: err.code ?? "failed", message: err.message ?? String(e) } };
      this.set(tabId, next);
      return next;
    }
  }

  /** The loader greeted from a page load: the overlay comes when the page has threads. */
  async hello(tabId: number, url: string): Promise<void> {
    const s = await this.route(tabId, url);
    if (s.threads.some(t => t.status === "open") && !s.overlay) await this.injectOnce(tabId);
  }

  private async injectOnce(tabId: number): Promise<void> {
    const s = this.tabs.get(tabId);
    if (s?.overlay) return;
    await this.d.inject(tabId);
    this.set(tabId, { ...(this.tabs.get(tabId) ?? emptyTab(tabId, "")), overlay: true });
  }

  /** The icon, the command or the context menu: the overlay is injected and comment mode flips. */
  async toggle(tabId: number, url: string): Promise<void> {
    if (!this.tabs.has(tabId)) this.tabs.set(tabId, emptyTab(tabId, url));
    await this.injectOnce(tabId);
    const s = this.tabs.get(tabId)!;
    this.set(tabId, { ...s, commentMode: !s.commentMode });
    void this.route(tabId, url);
  }

  /** One event or notice from the stream hub, for the clients `ids`. */
  fromHub(ids: string[], msg: HubMsg): void {
    for (const id of ids) {
      if (!id.startsWith("tab:")) continue;
      const tabId = Number(id.slice(4));
      const s = this.tabs.get(tabId);
      if (!s) continue;
      if (msg.t === "event") this.set(tabId, applyEvent(s, msg.name, msg.data));
      else if (msg.t === "live" || msg.t === "resync") void this.route(tabId, s.url);
    }
  }

  async fromOverlay(tabId: number, _windowId: number, m: OverlayToWorker): Promise<unknown> {
    const s = this.tabs.get(tabId) ?? emptyTab(tabId, "");
    switch (m.t) {
      case "hello": return this.hello(tabId, m.url);
      case "route": return this.route(tabId, m.url);
      case "resolved": this.set(tabId, { ...s, resolved: Object.fromEntries(m.results.map(r => [r.id, r])) }); return;
      case "comment-mode": this.set(tabId, { ...s, commentMode: m.on }); return;
      case "pin": this.set(tabId, { ...s, selected: m.threadId }); return;
      case "removed": this.set(tabId, { ...s, overlay: false, error: { code: "overlay_removed", message: "The page removed Clax's overlay." } }); return;
      default: return; // capture, pick, quiet and cancel are the pick flow's (Task 13)
    }
  }

  panelState(tabId: number | null): PanelState {
    const s = tabId === null ? undefined : this.tabs.get(tabId);
    return {
      tabId, url: s?.url ?? null, page: s?.page ?? null, route: s?.route ?? null, threads: s?.threads ?? [], resolved: s?.resolved ?? {},
      versions: [], working: s?.working ?? [], participants: null, viewer: null, commentMode: s?.commentMode ?? false,
      enabled: !!s?.overlay, selected: s?.selected ?? null, error: s?.error ?? null,
    };
  }

  attachPanel(port: chrome.runtime.Port, onMessage: (tabId: number | null, m: unknown) => void): void {
    const entry = { port, tabId: null as number | null };
    this.panels.set(port.name, entry);
    port.onMessage.addListener(m => {
      if ((m as { t?: string }).t === "watch-tab") {
        entry.tabId = (m as { tabId: number }).tabId;
        port.postMessage({ t: "tab", state: this.panelState(entry.tabId) } satisfies WorkerToPanel);
      }
      onMessage(entry.tabId, m);
    });
    port.onDisconnect.addListener(() => this.panels.delete(port.name));
  }
}
```

Fill `versions`, `participants` and `viewer` in `panelState` from `api.artifact` and `api.me` when the panel asks (`watch-tab`), keeping them on the tab state (add `versions`, `participants` fields to `TabState`, refreshed in `route` when the page changes and on `version` events).

- [ ] **Step 7: The loader and the worker's wiring**

`web/extension/src/content/loader.ts`:

```ts
// The content script for each origin Clax is on (spec 2026-10-05 §12):
// under 2 KiB. It tells the worker the page's URL on load and on every
// same-document navigation; the worker injects the overlay when the page
// has threads or the person turned Clax on in this tab.
const tell = (t: "hello" | "route") => chrome.runtime.sendMessage({ t, url: location.href }).catch(() => {});
void tell("hello");
const nav = (globalThis as { navigation?: EventTarget }).navigation;
if (nav) nav.addEventListener("navigatesuccess", () => void tell("route"));
else addEventListener("popstate", () => void tell("route"));
addEventListener("hashchange", () => void tell("route"));
```

`web/extension/src/sw/main.ts`:

```ts
// The service worker (spec 2026-10-05 §6.4): the only holder of the
// credential. It pairs through the native host, talks to the daemon, keeps
// one event stream for every tab with Clax on, and answers the overlays,
// the composers and the side panels. Every listener is registered at the
// top level, so a worker Chrome restarts for an event hears it.
import { Hub } from "../../../shell/src/stream-hub";
import { isFromComposer, isFromOverlay, isFromPanel } from "../messages";
import { Api } from "./api";
import * as origins from "./origins";
import { PairError, Pairer } from "./pairing";
import { Tabs } from "./tabs";

const originsEnv: origins.OriginsEnv = { permissions: chrome.permissions, scripting: chrome.scripting, local: chrome.storage.local };
const pairer = new Pairer({
  sendNative: async (host, msg) => {
    try { return await chrome.runtime.sendNativeMessage(host, msg); }
    catch (e) { throw new PairError(/not found/i.test(String(e)) ? "host_missing" : "host_failed", String(e)); }
  },
  session: chrome.storage.session, local: chrome.storage.local,
  manifestVersion: chrome.runtime.getManifest().version,
  reload: () => chrome.runtime.reload(),
  now: () => Date.now(),
});
const api = new Api(pairer);
let tabs: Tabs;
const hub = new Hub({ send: (ids, msg) => tabs.fromHub(ids, msg), fetch: (input, init) => api.request(String(input), init), base: "" });
tabs = new Tabs({
  api, hub,
  toOverlay: (tabId, m) => void chrome.tabs.sendMessage(tabId, m, { frameId: 0 }).catch(() => {}),
  inject: tabId => origins.injectOverlay(originsEnv, tabId),
});

/** A gesture that grants activeTab (spec L8). The side panel (icon only)
 * and the origin's permission are asked for before any await. */
function gesture(tab: chrome.tabs.Tab, panel: boolean): void {
  const origin = tab.url ? origins.originOf(tab.url) : null;
  if (tab.id === undefined || !origin || !tab.url) return;
  if (panel) void chrome.sidePanel.open({ tabId: tab.id }).catch(() => {});
  const asked = origins.ask(originsEnv, origin);
  const tabId = tab.id, url = tab.url;
  void (async () => {
    if (await asked) await origins.remember(originsEnv, origin);
    await tabs.toggle(tabId, url);
  })();
}

chrome.action.onClicked.addListener(tab => gesture(tab, true));
chrome.commands.onCommand.addListener((cmd, tab) => { if (cmd === "comment" && tab) gesture(tab, false); });
chrome.runtime.onInstalled.addListener(() => chrome.contextMenus.create({ id: "clax-comment", title: "Comment with Clax", contexts: ["page", "selection", "link", "image"] }));
chrome.contextMenus.onClicked.addListener((_info, tab) => { if (tab) gesture(tab, false); });

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  if (sender.id !== chrome.runtime.id || sender.tab?.id === undefined || sender.frameId !== 0 || !isFromOverlay(m)) return false;
  void tabs.fromOverlay(sender.tab.id, sender.tab.windowId, m).then(r => reply(r ?? null), e => reply({ error: String(e) }));
  return true;
});

chrome.runtime.onConnect.addListener(port => {
  const s = port.sender;
  const page = s?.url ? new URL(s.url).pathname : "";
  if (s?.id !== chrome.runtime.id) { port.disconnect(); return; }
  if (port.name.startsWith("panel:") && page === "/sidepanel.html") {
    tabs.attachPanel(port, (tabId, m) => { if (isFromPanel(m)) void panelAction(tabId, m); });
    return;
  }
  if (port.name.startsWith("composer:") && page === "/composer.html" && s.tab?.id !== undefined) {
    // Task 13 attaches the pick flow here; until then a composer is refused.
    port.disconnect();
    return;
  }
  port.disconnect();
});

/** A side panel's action; Task 14 fills the cases. */
async function panelAction(_tabId: number | null, _m: unknown): Promise<void> {}

if (__CLAX_EXT_TEST__) {
  (globalThis as unknown as { claxTest: unknown }).claxTest = {
    comment: (tabId: number, url: string) => tabs.toggle(tabId, url),
    state: (tabId: number) => tabs.state(tabId),
    pairer,
  };
}
```

(`isFromComposer` is imported for Task 13; remove the import if the linter objects until then.)

- [ ] **Step 8: Run the tests and the build**

Run: `cd web && npx vitest run extension && npm run build && node scripts/bundle-size.mjs && npm run typecheck && npm run lint`
Expected: PASS, with `loader.js` under 2 KiB and `sw.js` under 24 KiB gzip.

- [ ] **Step 9: Commit**

```bash
git add web/extension
git -c commit.gpgsign=false commit -m "Pair the extension's worker with the daemon and follow each tab's live page"
```

---

### Task 11: The page snapshot

**Files:**
- Create: `web/extension/src/content/snapshot.ts`
- Create: `web/extension/src/content/snapshot.test.ts`

**Interfaces:**
- Consumes: `OVERLAY_TAG` (`web/bridge/src/anchor.ts`).
- Produces: `serializeSnapshot(doc: Document, opts?: {skip?: Element[]; maxElements?: number; maxBytes?: number; deadlineMs?: number; now?: () => number}) -> {html: string; error: null} | {html: string; error: "too_large"}`; `MAX_ELEMENTS = 100_000`, `MAX_BYTES = 8 * 1024 * 1024`, `DEADLINE_MS = 1500`.

- [ ] **Step 1: Write the failing tests**

`web/extension/src/content/snapshot.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { serializeSnapshot } from "./snapshot";

function doc(body: string, head = ""): Document {
  const d = document.implementation.createHTMLDocument("Page");
  d.head.insertAdjacentHTML("beforeend", head);
  d.body.innerHTML = body;
  Object.defineProperty(d, "baseURI", { value: "http://localhost:5173/app/" });
  return d;
}
const snap = (d: Document) => {
  const r = serializeSnapshot(d);
  expect(r.error).toBeNull();
  return r.html;
};

describe("serializeSnapshot", () => {
  it("drops scripts, handlers, comments and dangerous URLs", () => {
    const html = snap(doc(
      `<script>alert(1)</script><noscript>x</noscript><!-- secret --><button onclick="steal()" onmouseover="x()">Save</button>
       <a href="javascript:alert(1)">j</a><a href="/next">n</a><iframe srcdoc="<script>1</script>"></iframe><img src="x.png" onerror="y()">`,
      `<meta http-equiv="refresh" content="0;url=http://evil"><base href="http://evil/">`));
    expect(html).not.toMatch(/<script|<noscript|onclick|onmouseover|onerror|javascript:|srcdoc|secret|http-equiv|<base/i);
    expect(html).toContain(">Save</button>");
    expect(html).toContain('href="http://localhost:5173/next"');
    expect(html).toContain('src="http://localhost:5173/app/x.png"');
    expect(html).toContain('data-clax-placeholder="iframe"');
  });

  it("keeps no form values", () => {
    const d = doc(`<form action="/login"><input type="password" name="pw" value="hunter2"><input type="hidden" name="csrf" value="tok">
      <input name="q" value="typed"><textarea>draft text</textarea><select><option>a</option><option selected>b</option></select></form>`);
    (d.querySelector("input[name=q]") as HTMLInputElement).value = "typed later";
    const html = snap(d);
    expect(html).not.toMatch(/hunter2|tok|typed|draft text|csrf|action=/);
    expect(html).toMatch(/<option selected="">b<\/option>/);
  });

  it("makes URLs absolute in srcset and styles", () => {
    const html = snap(doc(`<img srcset="a.png 1x, /b.png 2x"><div style="background:url('bg.png')"></div>`));
    expect(html).toContain('srcset="http://localhost:5173/app/a.png 1x, http://localhost:5173/b.png 2x"');
    expect(html).toContain('url("http://localhost:5173/app/bg.png")');
  });

  it("inlines the CSSOM's rules, including inserted ones", () => {
    const d = doc(`<p>x</p>`, `<style>p { color: red }</style>`);
    (d.querySelector("style") as HTMLStyleElement).sheet!.insertRule("p { font-weight: 700 }", 1);
    const html = snap(d);
    expect(html).toMatch(/<style>p \{\s*color: red;?\s*\}\s*p \{\s*font-weight: 700;?\s*\}<\/style>/);
  });

  it("keeps an unreadable stylesheet as a link to its absolute URL", () => {
    const d = doc(`<p>x</p>`, `<link rel="stylesheet" href="https://cdn.example/x.css"><link rel="preload" href="y.js">`);
    const link = d.querySelector("link") as HTMLLinkElement;
    Object.defineProperty(link, "sheet", { value: { get cssRules() { throw new DOMException("cross-origin", "SecurityError"); }, href: "https://cdn.example/x.css" } });
    const html = snap(d);
    expect(html).toContain('<link rel="stylesheet" href="https://cdn.example/x.css">');
    expect(html).not.toContain("preload");
  });

  it("writes open shadow roots as declarative shadow DOM and skips Clax's own elements", () => {
    const d = doc(`<my-card></my-card><clax-overlay></clax-overlay>`);
    d.querySelector("my-card")!.attachShadow({ mode: "open" }).innerHTML = "<b>inside</b>";
    const html = snap(d);
    expect(html).toContain('<my-card><template shadowrootmode="open"><b>inside</b></template></my-card>');
    expect(html).not.toContain("clax-overlay");
  });

  it("escapes text and attributes", () => {
    const d = doc(`<p title='a"b'>1 &lt; 2 &amp; &lt;/style&gt;</p>`);
    expect(snap(d)).toContain('<p title="a&quot;b">1 &lt; 2 &amp; &lt;/style&gt;</p>');
  });

  it("gives a placeholder page past a cap", () => {
    const d = doc(`${"<i></i>".repeat(50)}`);
    d.title = "<Big>";
    const r = serializeSnapshot(d, { maxElements: 10 });
    expect(r.error).toBe("too_large");
    expect(r.html).toContain("&lt;Big&gt;");
    expect(r.html).toContain("Snapshot unavailable");
    let t = 0;
    expect(serializeSnapshot(doc("<i></i><i></i>"), { deadlineMs: 5, now: () => (t += 10) }).error).toBe("too_large");
  });
});
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cd web && npx vitest run extension/src/content/snapshot.test.ts`
Expected: FAIL: cannot resolve `./snapshot`.

- [ ] **Step 3: Implement**

`web/extension/src/content/snapshot.ts`:

```ts
// The sanitized snapshot of a live page (spec 2026-10-05 §8.2): the DOM as
// HTML with no script, no event handler, no form value, no hidden input,
// no comment and no dangerous URL; styles taken from the CSSOM (so rules
// CSS-in-JS inserted are kept) with their URLs made absolute; open shadow
// roots as declarative shadow DOM; embedded content as sized placeholders.
// The daemon also serves it with a policy that runs no page script (§8.4).
import { OVERLAY_TAG } from "../../../bridge/src/anchor";

export const MAX_ELEMENTS = 100_000;
export const MAX_BYTES = 8 * 1024 * 1024;
export const DEADLINE_MS = 1500;

export type Snapshot = { html: string; error: null } | { html: string; error: "too_large" };
type Opts = { skip?: Element[]; maxElements?: number; maxBytes?: number; deadlineMs?: number; now?: () => number };

const HTML_NS = "http://www.w3.org/1999/xhtml";
const DROP = new Set(["script", "noscript", "base", "template", "portal"]);
const PLACEHOLDER = new Set(["iframe", "frame", "frameset", "object", "embed", "canvas", "video", "audio"]);
const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);
const URL_ATTRS = new Set(["href", "src", "poster", "background", "xlink:href", "cite", "longdesc"]);
const DROP_ATTRS = new Set(["srcdoc", "nonce", "integrity", "action", "formaction", "ping", "value"]);
const TOO_LARGE = Symbol("too large");

const escText = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const escAttr = (s: string) => escText(s).replace(/"/g, "&quot;");

function absolute(u: string, base: string): string | null {
  try {
    const abs = new URL(u.trim(), base).href;
    return /^(https?:|data:image\/)/i.test(abs) ? abs : null;
  } catch {
    return null;
  }
}

function cssUrls(text: string, base: string): string {
  return text.replace(/url\(\s*(['"]?)([^'")]*)\1\s*\)/gi, (_m, _q, u: string) => `url("${(absolute(u, base) ?? "").replace(/"/g, "%22")}")`);
}

function rulesOf(sheet: CSSStyleSheet | null | undefined): string | null {
  if (!sheet) return null;
  try {
    return [...sheet.cssRules].map(r => r.cssText).join("\n");
  } catch {
    return null;
  }
}

class Writer {
  private parts: string[] = [];
  private bytes = 0;
  private count = 0;
  private readonly started: number;
  constructor(private readonly o: Required<Omit<Opts, "skip">> & { skip: Set<Element> }, private readonly base: string) {
    this.started = o.now();
  }
  emit(s: string) {
    this.bytes += s.length;
    if (this.bytes > this.o.maxBytes) throw TOO_LARGE;
    this.parts.push(s);
  }
  tick() {
    if (++this.count > this.o.maxElements || this.o.now() - this.started > this.o.deadlineMs) throw TOO_LARGE;
  }
  text(): string {
    return this.parts.join("");
  }

  style(text: string, base: string) {
    this.emit(`<style>${cssUrls(text, base).replace(/<\/style/gi, "<\\/style")}</style>`);
  }

  element(el: Element): void {
    this.tick();
    const tag = el.localName;
    if (this.o.skip.has(el) || tag === OVERLAY_TAG || DROP.has(tag)) return;
    if (tag === "input" && (el as HTMLInputElement).type === "hidden") return;
    if (tag === "meta" && el.hasAttribute("http-equiv")) return;
    if (tag === "link") {
      if (!/\bstylesheet\b/i.test(el.getAttribute("rel") ?? "")) return;
      const sheet = (el as HTMLLinkElement).sheet as CSSStyleSheet | null;
      const href = absolute(el.getAttribute("href") ?? "", this.base);
      const rules = rulesOf(sheet);
      if (rules !== null) this.style(rules, sheet?.href ?? href ?? this.base);
      else if (href) this.emit(`<link rel="stylesheet" href="${escAttr(href)}">`);
      return;
    }
    if (tag === "style") {
      this.style(rulesOf((el as HTMLStyleElement).sheet as CSSStyleSheet | null) ?? el.textContent ?? "", this.base);
      return;
    }
    if (PLACEHOLDER.has(tag)) {
      const r = el.getBoundingClientRect();
      this.emit(`<div data-clax-placeholder="${tag}" style="display:inline-block;width:${Math.round(r.width)}px;height:${Math.round(r.height)}px;background:rgba(128,128,128,.15)"></div>`);
      return;
    }
    this.emit(`<${tag}`);
    for (const a of [...el.attributes]) {
      const name = a.name.toLowerCase();
      if (name.startsWith("on") || DROP_ATTRS.has(name)) continue;
      let value: string | null = a.value;
      if (URL_ATTRS.has(name)) value = absolute(value, this.base);
      else if (name === "srcset") value = value.split(",").map(c => { const [u, ...d] = c.trim().split(/\s+/); const abs = absolute(u, this.base); return abs ? [abs, ...d].join(" ") : null; }).filter(Boolean).join(", ") || null;
      else if (name === "style") value = cssUrls(value, this.base);
      if (value === null) continue;
      this.emit(` ${name}="${escAttr(value)}"`);
    }
    if (tag === "option" && (el as HTMLOptionElement).selected && !el.hasAttribute("selected")) this.emit(` selected=""`);
    this.emit(">");
    if (tag === "head") {
      this.emit(`<meta charset="utf-8">`);
      for (const s of el.ownerDocument.adoptedStyleSheets ?? []) this.style(rulesOf(s) ?? "", this.base);
    }
    if (el.namespaceURI === HTML_NS && VOID.has(tag)) return;
    if (tag === "textarea") { this.emit("</textarea>"); return; }
    const shadow = (el as HTMLElement).shadowRoot;
    if (shadow) {
      this.emit(`<template shadowrootmode="open">`);
      for (const s of shadow.adoptedStyleSheets ?? []) this.style(rulesOf(s) ?? "", this.base);
      this.children(shadow);
      this.emit(`</template>`);
    }
    this.children(el);
    this.emit(`</${tag}>`);
  }

  children(n: ParentNode): void {
    for (const c of n.childNodes) {
      if (c.nodeType === Node.TEXT_NODE) this.emit(escText((c as Text).data));
      else if (c.nodeType === Node.ELEMENT_NODE) this.element(c as Element);
    }
  }
}

/** A minimal page standing in for a snapshot past a cap. */
function placeholderPage(title: string): string {
  const t = escText(title.slice(0, 200));
  return `<!doctype html><html><head><meta charset="utf-8"><title>${t}</title></head><body><p>Snapshot unavailable: the page is too large. (${t})</p></body></html>`;
}

export function serializeSnapshot(doc: Document, opts: Opts = {}): Snapshot {
  const o = {
    skip: new Set(opts.skip ?? []),
    maxElements: opts.maxElements ?? MAX_ELEMENTS,
    maxBytes: opts.maxBytes ?? MAX_BYTES,
    deadlineMs: opts.deadlineMs ?? DEADLINE_MS,
    now: opts.now ?? (() => performance.now()),
  };
  const w = new Writer(o, doc.baseURI);
  try {
    w.emit("<!doctype html>");
    w.element(doc.documentElement);
    return { html: w.text(), error: null };
  } catch (e) {
    if (e === TOO_LARGE) return { html: placeholderPage(doc.title), error: "too_large" };
    throw e;
  }
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cd web && npx vitest run extension/src/content/snapshot.test.ts`
Expected: PASS. (Where jsdom serializes CSS text differently from Chrome, loosen only the whitespace in the expectation, never what it checks.)

- [ ] **Step 5: Commit**

```bash
git add web/extension/src/content/snapshot.ts web/extension/src/content/snapshot.test.ts
git -c commit.gpgsign=false commit -m "Serialize a sanitized snapshot of a live page"
```

---

### Task 12: The overlay: pins, re-resolution and comment mode in a closed shadow root

**Files:**
- Modify: `web/bridge/src/comment-mode.ts` (`shadow` option; `setVisible`)
- Modify: `web/bridge/test/comment-mode.test.ts` (or the file that tests `CommentMode`; check with `ls web/bridge/test`)
- Create: `web/extension/src/content/resolver.ts`, `web/extension/src/content/resolver.test.ts`
- Create: `web/extension/src/content/pins.ts`
- Modify: `web/extension/src/content/overlay.ts`

**Interfaces:**
- Consumes: `CommentMode`, `ModeHooks` (`comment-mode.ts`); `resolveAnchor`, `textIndex`, `buildElementAnchor`, `buildRangeAnchor` (`anchor.ts`); `buildAreaAnchor`, `placeArea` (`area.ts`); `rectOf` (`target.ts`); Task 8's messages; Task 11's `serializeSnapshot`.
- Produces:
  - `new CommentMode(doc, hooks, {trustedOnly?, shadow?: "open" | "closed"})`; `CommentMode.setVisible(on: boolean)`.
  - `resolver.ts`: `QUIET_MS = 150`, `type Placed = {id: string; n: number | null; box: Box | null}`, `type Timers = {set(fn: () => void, ms: number): unknown; clear(h: unknown): void; frame(fn: () => void): void}`, `class Resolver(doc, onPlaced, timers?, ignore?)` with `set(threads, route)`, `run(): Placed[]`, `measure(): Placed[]`, `lastMutation: number`, `stop()`.
  - `pins.ts`: `class Pins(root: ShadowRoot, onPick: (threadId: string) => void)` with `draw(placed: Placed[], selected: string | null)`.
  - The overlay posts `capture`, `pick`, `resolved`, `quiet`, `pin`, `comment-mode`, `removed` (spec §9.4) and takes `state`, `comment-mode`, `close-composer`, `scroll-to`, `focus`, `snapshot-now`.

- [ ] **Step 1: Write the failing tests**

Add to the `CommentMode` tests in `web/bridge/test/`:

```ts
  it("can keep its drawing in a closed shadow root and hide it", () => {
    const m = new CommentMode(document, hooks(), { trustedOnly: false, shadow: "closed" });
    const host = document.querySelector("clax-overlay") as HTMLElement;
    expect(host.shadowRoot).toBeNull();
    m.setVisible(false);
    expect(host.style.visibility).toBe("hidden");
    m.setVisible(true);
    expect(host.style.visibility).toBe("");
  });
```

(`hooks()` is the test file's no-op `ModeHooks`, or write one.)

`web/extension/src/content/resolver.test.ts`:

```ts
import { beforeEach, describe, expect, it } from "vitest";
import { type Placed, QUIET_MS, Resolver, type Timers } from "./resolver";

/** Timers a test advances by hand. */
function manual() {
  let now = 0;
  let queue: { at: number; fn: () => void }[] = [];
  const frames: (() => void)[] = [];
  const t: Timers & { advance(ms: number): void } = {
    set: (fn, ms) => { const e = { at: now + ms, fn }; queue.push(e); return e; },
    clear: h => { queue = queue.filter(e => e !== h); },
    frame: fn => { frames.push(fn); },
    advance(ms) {
      now += ms;
      for (const e of queue.filter(e => e.at <= now)) { queue = queue.filter(x => x !== e); e.fn(); }
      while (frames.length) frames.shift()!();
    },
  };
  return t;
}
const flush = () => new Promise(r => setTimeout(r, 0)); // lets MutationObserver records arrive
const thread = (id: string, selector: string, quote: string | null, route?: string) => ({
  id, status: "open", anchor: { kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html", ...(route ? { route } : {}) },
}) as never;

let placed: Placed[] = [];
beforeEach(() => {
  document.body.innerHTML = `<div id="app"><main><button id="save">Save</button></main></div>`;
  placed = [];
});

describe("Resolver", () => {
  it("shows only the current route's threads", () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    r.set([thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "#save", "Save"), thread("01J9BBBBBBBBBBBBBBBBBBBBBB", "#save", "Save", "?tab=billing")], null);
    t.advance(0);
    expect(placed.map(p => p.id)).toEqual(["01J9AAAAAAAAAAAAAAAAAAAAAA"]);
    r.set(r.threads, "?tab=billing");
    t.advance(0);
    expect(placed.map(p => p.id)).toEqual(["01J9BBBBBBBBBBBBBBBBBBBBBB"]);
    r.stop();
  });

  it("re-resolves after a wholesale replacement and detaches what is gone", async () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    r.set([thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "div#app > main > button", "Save")], null);
    t.advance(0);
    expect(placed[0].box).not.toBeNull();
    document.querySelector("#app")!.innerHTML = `<main><button id="save">Save changes</button></main>`;
    await flush();
    t.advance(QUIET_MS - 1);
    expect(placed[0].box).not.toBeNull();
    t.advance(1);
    expect(placed[0].box).not.toBeNull();
    document.querySelector("#app")!.innerHTML = `<main><p>No button</p></main>`;
    await flush();
    t.advance(QUIET_MS);
    expect(placed[0].box).toBeNull();
    expect(placed[0].n).toBeNull();
    r.stop();
  });

  it("ignores mutations it is told to", async () => {
    const t = manual();
    const host = document.createElement("clax-overlay");
    document.documentElement.appendChild(host);
    let runs = 0;
    const r = new Resolver(document, () => { runs++; }, t, n => host === n || host.contains(n));
    r.set([], null);
    t.advance(0);
    host.setAttribute("data-x", "1");
    await flush();
    t.advance(QUIET_MS);
    expect(runs).toBe(1);
    r.stop();
  });
});
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cd web && npx vitest run extension/src/content/resolver.test.ts bridge/test`
Expected: FAIL: cannot resolve `./resolver`; `setVisible` is not a function.

- [ ] **Step 3: The closed-shadow option in `CommentMode`**

In `web/bridge/src/comment-mode.ts`, change the constructor's options and shadow root:

```ts
  /** `opts.trustedOnly` (default true) ignores events the page dispatched
   * itself; tests that synthesise input turn it off. `opts.shadow`
   * (default "open") is the drawing's shadow root mode: the Chrome
   * extension's overlay uses "closed", so page scripts cannot reach it. */
  constructor(private readonly doc: Document, private readonly hooks: ModeHooks, opts: { trustedOnly?: boolean; shadow?: "open" | "closed" } = {}) {
    this.trustedOnly = opts.trustedOnly ?? true;
    this.host = doc.createElement(OVERLAY_TAG);
    const root = this.host.attachShadow({ mode: opts.shadow ?? "open" });
```

and add:

```ts
  /** Hides or shows everything comment mode draws (the extension hides it
   * while the tab's screenshot is taken). */
  setVisible(on: boolean): void {
    this.host.style.visibility = on ? "" : "hidden";
  }
```

Check the bridge's other uses of the host's `shadowRoot` (`grep -n "shadowRoot" web/bridge/src`): anything that reads it must keep its own reference (`root`) instead, since a closed root's `shadowRoot` is null.

- [ ] **Step 4: Implement the resolver and the pins**

`web/extension/src/content/resolver.ts`:

```ts
// Keeps the page's threads on their anchors in the live DOM (spec
// 2026-10-05 §11 "Hot reload"): after the DOM has been quiet for QUIET_MS,
// in the next animation frame, every open thread of the current route is
// resolved again; one whose anchor is gone has no box (Detached). Scrolls
// and resizes only measure the elements found last time.
import { type Resolved, resolveAnchor, textIndex, type TextIndex } from "../../../bridge/src/anchor";
import { placeArea } from "../../../bridge/src/area";
import { INDEX_FILE, type Box } from "../../../bridge/src/protocol";
import { rectOf } from "../../../bridge/src/target";
import type { Thread } from "../../../shell/src/threads";

export const QUIET_MS = 150;
export type Placed = { id: string; n: number | null; box: Box | null };
export type Timers = { set(fn: () => void, ms: number): unknown; clear(h: unknown): void; frame(fn: () => void): void };
export const realTimers: Timers = { set: (fn, ms) => setTimeout(fn, ms), clear: h => clearTimeout(h as number), frame: fn => requestAnimationFrame(() => fn()) };

export class Resolver {
  threads: Thread[] = [];
  private route: string | null = null;
  private found = new Map<string, Resolved>();
  private timer: unknown = null;
  private readonly observer: MutationObserver;
  lastMutation = 0;

  constructor(
    private readonly doc: Document,
    private readonly onPlaced: (p: Placed[]) => void,
    private readonly timers: Timers = realTimers,
    private readonly ignore: (n: Node) => boolean = () => false,
  ) {
    this.observer = new MutationObserver(records => {
      if (records.every(r => this.ignore(r.target))) return;
      this.lastMutation = performance.now();
      this.schedule(QUIET_MS);
    });
    this.observer.observe(doc.documentElement, { subtree: true, childList: true, characterData: true, attributes: true });
  }

  /** The tab's threads and route changed: resolve at once. */
  set(threads: Thread[], route: string | null): void {
    this.threads = threads;
    this.route = route;
    this.schedule(0);
  }

  private schedule(ms: number): void {
    if (this.timer !== null) this.timers.clear(this.timer);
    this.timer = this.timers.set(() => { this.timer = null; this.timers.frame(() => this.run()); }, ms);
  }

  run(): Placed[] {
    let idx: TextIndex | undefined;
    const index = () => (idx ??= textIndex(this.doc.body));
    this.found.clear();
    const here = this.threads.filter(t => t.status === "open" && (t.anchor.route ?? null) === this.route);
    for (const t of here) {
      const r = resolveAnchor(this.doc, t.anchor, new Map(), INDEX_FILE, index, false);
      if (r) this.found.set(t.id, r);
    }
    return this.measure();
  }

  /** The boxes of the threads found last time, measured now. */
  measure(): Placed[] {
    let n = 0;
    const here = this.threads.filter(t => t.status === "open" && (t.anchor.route ?? null) === this.route);
    const placed = here.map(t => {
      const r = this.found.get(t.id);
      if (!r) return { id: t.id, n: null, box: null };
      const box = t.anchor.kind === "area" ? placeArea(t.anchor, r.element) : (() => { const b = rectOf(r.range ?? r.element); return { x: b.left, y: b.top, w: b.width, h: b.height }; })();
      return { id: t.id, n: ++n, box };
    });
    this.onPlaced(placed);
    return placed;
  }

  stop(): void {
    this.observer.disconnect();
    if (this.timer !== null) this.timers.clear(this.timer);
  }
}
```

`web/extension/src/content/pins.ts`:

```ts
// The threads' pins over the page (spec 2026-10-05 O4): numbered, in the
// overlay's closed shadow root, carrying no text but their number. A press
// on a pin selects its thread in the side panel; only the viewer's own
// (trusted) presses count.
import type { Placed } from "./resolver";

export const PIN_CSS = `.pin{position:fixed;z-index:2147483647;min-width:20px;height:20px;margin:-10px 0 0 -10px;padding:0 5px;border-radius:10px 10px 10px 2px;border:1.5px solid #fff;background:#ed5439;color:#2f0b04;font:600 11px/17px ui-sans-serif,-apple-system,system-ui,sans-serif;box-shadow:0 1px 3px rgba(47,11,4,.45);cursor:pointer;box-sizing:border-box}
.pin.sel{box-shadow:0 0 0 3px #2f6f2a}`;

export class Pins {
  private els = new Map<string, HTMLButtonElement>();
  constructor(private readonly root: ShadowRoot, private readonly onPick: (threadId: string) => void) {}

  draw(placed: Placed[], selected: string | null): void {
    const live = new Set<string>();
    for (const p of placed) {
      if (!p.box || p.n === null) continue;
      live.add(p.id);
      let el = this.els.get(p.id);
      if (!el) {
        el = this.root.ownerDocument.createElement("button");
        el.type = "button";
        el.className = "pin";
        const id = p.id;
        el.addEventListener("click", e => { if (e.isTrusted) { e.preventDefault(); e.stopPropagation(); this.onPick(id); } });
        this.root.appendChild(el);
        this.els.set(p.id, el);
      }
      el.textContent = String(p.n);
      el.setAttribute("aria-label", `Thread ${p.n}`);
      el.classList.toggle("sel", p.id === selected);
      el.style.left = `${Math.round(p.box.x + p.box.w)}px`;
      el.style.top = `${Math.round(p.box.y)}px`;
    }
    for (const [id, el] of this.els) if (!live.has(id)) { el.remove(); this.els.delete(id); }
  }
}
```

- [ ] **Step 5: The overlay**

`web/extension/src/content/overlay.ts`:

```ts
// The overlay (spec 2026-10-05 §6.4, O4, L7): injected on demand into the
// page's isolated world, once per document. Pins and comment mode's drawing
// live in closed shadow roots on <html>; the composer is an extension page
// in an iframe there, so nothing typed into it reaches the page. It hears
// only the worker, and tells it only what §9.4 lists.
import { buildElementAnchor, buildRangeAnchor, OVERLAY_TAG, resolveAnchor } from "../../../bridge/src/anchor";
import { buildAreaAnchor, type AreaRect } from "../../../bridge/src/area";
import { CommentMode } from "../../../bridge/src/comment-mode";
import type { Anchor } from "../../../bridge/src/protocol";
import { rectOf } from "../../../bridge/src/target";
import { type Rect, isFromWorker, type OverlayToWorker, type WorkerToOverlay } from "../messages";
import { PIN_CSS, Pins } from "./pins";
import { type Placed, Resolver } from "./resolver";
import { serializeSnapshot } from "./snapshot";

const G = globalThis as { __claxOverlay?: boolean };
if (!G.__claxOverlay) {
  G.__claxOverlay = true;
  start();
}

function start(): void {
  const send = (m: OverlayToWorker) => chrome.runtime.sendMessage(m).catch(() => null);
  const host = document.createElement(OVERLAY_TAG);
  host.setAttribute("popover", "manual");
  host.style.cssText = "all:initial;position:fixed;inset:0;pointer-events:none;background:transparent;border:0;margin:0;padding:0;overflow:visible";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `<style>${PIN_CSS}.pin,iframe{pointer-events:auto}iframe{position:fixed;z-index:2147483647;width:360px;height:236px;border:0;border-radius:10px;box-shadow:0 8px 28px rgba(0,0,0,.28);color-scheme:normal}</style>`;
  const attach = () => { document.documentElement.appendChild(host); try { host.showPopover(); } catch { /* no popover support */ } };
  attach();
  let reattached = false;
  new MutationObserver(() => {
    if (host.isConnected) return;
    if (reattached) { void send({ t: "removed" }); return; }
    reattached = true;
    attach();
  }).observe(document.documentElement, { childList: true });

  let state: Extract<WorkerToOverlay, { t: "state" }> | null = null;
  let selected: string | null = null;
  let placed: Placed[] = [];
  const pins = new Pins(root, id => { selected = id; void send({ t: "pin", threadId: id }); pins.draw(placed, selected); });
  const resolver = new Resolver(document, p => {
    placed = p;
    pins.draw(p, selected);
    void send({ t: "resolved", results: p.map(x => ({ id: x.id, found: x.box !== null, method: null, rect: x.box })) });
  }, undefined, n => n === host || host.contains(n));
  addEventListener("scroll", () => requestAnimationFrame(() => { placed = resolver.measure(); }), { capture: true, passive: true });
  addEventListener("resize", () => requestAnimationFrame(() => { placed = resolver.measure(); }), { passive: true });

  const mode = new CommentMode(document, {
    hover: () => {},
    pickElement: el => void pick(buildElementAnchor(document, el), box(rectOf(el))),
    pickRange: r => void pick(buildRangeAnchor(document, r), box(rectOf(r))),
    pickArea: (r: AreaRect) => void pick(buildAreaAnchor(document, r), { x: r.left, y: r.top, w: r.width, h: r.height }),
    cancel: () => { mode.set(false); void send({ t: "comment-mode", on: false }); },
  }, { shadow: "closed" });

  const box = (r: DOMRect): Rect => ({ x: r.left, y: r.top, w: r.width, h: r.height });
  const frames = () => new Promise<void>(r => requestAnimationFrame(() => requestAnimationFrame(() => r())));
  let composer: HTMLIFrameElement | null = null;

  async function pick(anchor: Anchor, rect: Rect): Promise<void> {
    mode.setVisible(false);
    host.style.visibility = "hidden";
    await frames();
    const r = await send({ t: "capture", rect, dpr: devicePixelRatio }) as { pickId?: string } | null;
    mode.setVisible(true);
    host.style.visibility = "";
    mode.captured();
    if (!r?.pickId) return;
    const pickId = r.pickId;
    mode.set(false);
    openComposer(pickId, rect);
    setTimeout(() => {
      const s = serializeSnapshot(document, { skip: [host] });
      void send({ t: "pick", pickId, anchor, url: location.href, title: document.title.slice(0, 1000), snapshot: s.html, snapshotError: s.error });
    }, 0);
  }

  function openComposer(pickId: string, rect: Rect): void {
    composer?.remove();
    const f = document.createElement("iframe");
    f.src = `${chrome.runtime.getURL("composer.html")}#${pickId}`;
    f.setAttribute("allow", "");
    const left = Math.min(Math.max(8, rect.x + rect.w + 12), innerWidth - 368);
    const top = Math.min(Math.max(8, rect.y), innerHeight - 244);
    f.style.left = `${left}px`;
    f.style.top = `${top}px`;
    root.appendChild(f);
    composer = f;
  }

  // The automatic snapshot after an agent addressed a thread (spec L11):
  // visible, pending, and the DOM quiet for a second; at most every 10 s.
  let lastQuiet = 0;
  // A ping every 20 s keeps the worker up while the overlay is on (spec §9.5).
  setInterval(() => void send({ t: "ping" }), 20_000);
  setInterval(() => {
    if (!state?.pending || document.visibilityState !== "visible") return;
    const now = performance.now();
    if (now - resolver.lastMutation < 1000 || now - lastQuiet < 10_000) return;
    lastQuiet = now;
    const s = serializeSnapshot(document, { skip: [host] });
    if (!s.error) void send({ t: "quiet", url: location.href, title: document.title.slice(0, 1000), snapshot: s.html });
  }, 1000);

  chrome.runtime.onMessage.addListener((m, sender) => {
    if (sender.id !== chrome.runtime.id || sender.tab || !isFromWorker(m)) return;
    switch (m.t) {
      case "state":
        state = m;
        mode.set(m.commentMode && composer === null);
        resolver.set(m.threads, m.route);
        break;
      case "comment-mode": mode.set(m.on); break;
      case "close-composer":
        composer?.remove();
        composer = null;
        if (m.posted || state?.commentMode) mode.set(true);
        break;
      case "focus": case "scroll-to": {
        selected = m.threadId;
        pins.draw(placed, selected);
        const t = state?.threads.find(x => x.id === m.threadId);
        const r = t && resolveAnchor(document, t.anchor);
        if (m.t === "scroll-to" && r) { (r.range?.startContainer.parentElement ?? r.element).scrollIntoView({ block: "center", behavior: "smooth" }); mode.flash(r.range ?? r.element); }
        break;
      }
      case "snapshot-now": lastQuiet = 0; break;
    }
  });
  void send({ t: "route", url: location.href });
}
```

(If `CommentMode.flash` is private, make it public; `buildElementAnchor` and friends take `file` defaulting to `index.html`, which is what a live page uses.)

- [ ] **Step 6: Run the tests and the build**

Run: `cd web && npx vitest run extension bridge/test && npm run build && node scripts/bundle-size.mjs`
Expected: PASS; `overlay.js` within 32 KiB gzip. If it is over, move `serializeSnapshot` behind a dynamic import of an extension module the worker injects with `chrome.scripting.executeScript({files: ["snapshot.js"]})` and rerun; do not raise the budget.

- [ ] **Step 7: Commit**

```bash
git add web/bridge/src/comment-mode.ts web/bridge/test web/extension/src/content
git -c commit.gpgsign=false commit -m "Put pins and comment mode over live pages in a closed shadow root, re-resolving on DOM changes"
```

---

### Task 13: The pick: screenshot, composer and posting

**Files:**
- Create: `web/extension/src/sw/capture.ts`, `web/extension/src/sw/capture.test.ts`
- Create: `web/extension/src/sw/picks.ts`, `web/extension/src/sw/picks.test.ts`
- Create: `web/extension/src/composer/ComposerFrame.svelte`
- Modify: `web/extension/src/composer/main.ts`, `web/extension/src/sw/main.ts`, `web/extension/src/sw/tabs.ts` (`capture`, `pick`, `quiet`, `cancel` to the pick flow)

**Interfaces:**
- Consumes: Task 10's `Api.postThread`, `Api.postSnapshot`, `Tabs`; Task 8's messages; `Composer.svelte` and `Draft` (`web/shell/src/view/composer-model.ts`).
- Produces:
  - `capture.ts`: `MAX_SIDE = 1600`, `MAX_CLIP = 5 * 1024 * 1024`, `type CaptureEnv = {capture(windowId: number): Promise<string>; bitmap(dataUrl: string): Promise<{width: number; height: number; image: CanvasImageSource}>; canvas(w: number, h: number): {getContext(k: "2d"): CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null; convertToBlob(o: {type: string}): Promise<Blob>}}`, `captureClip(env, windowId, rect, dpr): Promise<{png: Blob} | {error: string}>`.
  - `picks.ts`: `PICK_TTL_MS = 600_000`, `class Picks(deps)` with `capture(tabId, windowId, rect, dpr): Promise<{pickId: string}>`, `attach(tabId, m)`, `attachComposer(port, tabId)`, `quiet(tabId, m)`, `cancel(tabId, pickId)`.

- [ ] **Step 1: Write the failing tests**

`web/extension/src/sw/capture.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { MAX_CLIP, captureClip, type CaptureEnv } from "./capture";

function env(opts: { sizes?: number[]; fail?: string } = {}) {
  const strokes: number[][] = [];
  const canvases: number[][] = [];
  const sizes = [...(opts.sizes ?? [1000])];
  const e: CaptureEnv = {
    capture: async () => { if (opts.fail) throw new Error(opts.fail); return "data:image/png;base64,AAAA"; },
    bitmap: async () => ({ width: 2400, height: 1600, image: {} as CanvasImageSource }),
    canvas: (w, h) => {
      canvases.push([w, h]);
      const ctx = { drawImage() {}, strokeRect: (...a: number[]) => strokes.push(a), set strokeStyle(_v: string) {}, set lineWidth(_v: number) {} };
      return { getContext: () => ctx as unknown as CanvasRenderingContext2D, convertToBlob: async () => new Blob([new Uint8Array(sizes.shift() ?? 10)]) };
    },
  };
  return { e, strokes, canvases };
}

describe("captureClip", () => {
  it("scales to 1600 px and draws the pick's outline at the same scale", async () => {
    const { e, strokes, canvases } = env();
    const r = await captureClip(e, 1, { x: 100, y: 50, w: 200, h: 40 }, 2);
    expect("png" in r).toBe(true);
    expect(canvases[0]).toEqual([1600, 1067]);
    const k = (2 * 1600) / 2400;
    [100 * k, 50 * k, 200 * k, 40 * k].forEach((want, i) => expect(strokes[0][i]).toBeCloseTo(want, 6));
  });

  it("halves the scale while the PNG is over the cap, then gives up", async () => {
    let { e, canvases } = env({ sizes: [MAX_CLIP + 1, 100] });
    expect("png" in (await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1))).toBe(true);
    expect(canvases[1]).toEqual([800, 533]);
    ({ e } = env({ sizes: [MAX_CLIP + 1, MAX_CLIP + 1, MAX_CLIP + 1, MAX_CLIP + 1] }));
    expect(await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "clip_too_large" });
  });

  it("says when the tab holds no activeTab grant", async () => {
    const { e } = env({ fail: "Either the '<all_urls>' or 'activeTab' permission is required." });
    expect(await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "no_capture_permission" });
  });
});
```

`web/extension/src/sw/picks.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { Picks } from "./picks";

const anchor = { kind: "element", selector: "#save", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
function port(name: string, tabId: number) {
  const sent: unknown[] = [];
  const listeners: ((m: unknown) => void)[] = [];
  return {
    name, sent, sender: { tab: { id: tabId } },
    postMessage: (m: unknown) => sent.push(m),
    onMessage: { addListener: (l: (m: unknown) => void) => listeners.push(l) },
    onDisconnect: { addListener: () => {} },
    disconnect: () => {},
    fire: (m: unknown) => listeners.forEach(l => l(m)),
  };
}
function setup() {
  const posted: FormData[] = [];
  const overlay: unknown[] = [];
  let n = 0;
  const picks = new Picks({
    api: { postThread: async (f: FormData) => { posted.push(f); return { thread: { id: "01J9AAAAAAAAAAAAAAAAAAAAAA" }, page: {}, version: 1 }; } } as never,
    capture: async () => ({ png: new Blob([new Uint8Array([137, 80, 78, 71])], { type: "image/png" }) }),
    toOverlay: (_t: number, m: unknown) => overlay.push(m),
    randomId: () => String(++n).padStart(32, "0"),
    now: () => 0,
  });
  return { picks, posted, overlay };
}

describe("Picks", () => {
  it("posts once the body and the snapshot are both in, whichever comes first", async () => {
    const { picks, posted, overlay } = setup();
    const { pickId } = await picks.capture(5, 1, { x: 0, y: 0, w: 1, h: 1 }, 1);
    const p = port(`composer:${pickId}`, 5);
    picks.attachComposer(p as never, 5);
    p.fire({ t: "ready" });
    expect((p.sent[0] as { t: string }).t).toBe("draft");
    p.fire({ t: "post", body: "Too wide" });
    expect(posted).toHaveLength(0);
    await picks.attach(5, { t: "pick", pickId, anchor: anchor as never, url: "http://localhost:5173/", title: "Home", snapshot: "<p>", snapshotError: null });
    await new Promise(r => setTimeout(r, 0));
    expect(posted).toHaveLength(1);
    expect(posted[0].get("body")).toBe("Too wide");
    expect(posted[0].get("url")).toBe("http://localhost:5173/");
    expect(p.sent.at(-1)).toEqual({ t: "posted", threadId: "01J9AAAAAAAAAAAAAAAAAAAAAA" });
    expect(overlay.at(-1)).toEqual({ t: "close-composer", pickId, posted: true });
  });

  it("refuses a composer for another tab or an unknown pick", async () => {
    const { picks } = setup();
    const { pickId } = await picks.capture(5, 1, { x: 0, y: 0, w: 1, h: 1 }, 1);
    const other = port(`composer:${pickId}`, 6);
    let cut = false;
    other.disconnect = () => { cut = true; };
    picks.attachComposer(other as never, 6);
    expect(cut).toBe(true);
    const unknown = port(`composer:${"f".repeat(32)}`, 5);
    let cut2 = false;
    unknown.disconnect = () => { cut2 = true; };
    picks.attachComposer(unknown as never, 5);
    expect(cut2).toBe(true);
  });
});
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cd web && npx vitest run extension/src/sw/capture.test.ts extension/src/sw/picks.test.ts`
Expected: FAIL: cannot resolve `./capture`, `./picks`.

- [ ] **Step 3: Implement the screenshot**

`web/extension/src/sw/capture.ts`:

```ts
// The pick's screenshot, the thread's clip (spec 2026-10-05 §8.1): the
// visible tab (needs activeTab, spec L8), the pick's outline drawn on in
// the pin colour, at most 1600 px on its long side, a PNG under 5 MiB
// (the scale halves up to three times to get there).
import type { Rect } from "../messages";

export const MAX_SIDE = 1600;
export const MAX_CLIP = 5 * 1024 * 1024;
const OUTLINE = "#ed5439";

export type CaptureEnv = {
  capture(windowId: number): Promise<string>;
  bitmap(dataUrl: string): Promise<{ width: number; height: number; image: CanvasImageSource }>;
  canvas(w: number, h: number): { getContext(k: "2d"): CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null; convertToBlob(o: { type: string }): Promise<Blob> };
};

export const chromeCapture: CaptureEnv = {
  capture: windowId => chrome.tabs.captureVisibleTab(windowId, { format: "png" }),
  bitmap: async url => { const b = await createImageBitmap(await (await fetch(url)).blob()); return { width: b.width, height: b.height, image: b }; },
  canvas: (w, h) => new OffscreenCanvas(w, h),
};

export async function captureClip(env: CaptureEnv, windowId: number, rect: Rect, dpr: number): Promise<{ png: Blob } | { error: string }> {
  let url: string;
  try {
    url = await env.capture(windowId);
  } catch (e) {
    return { error: /activeTab|<all_urls>|permission/i.test(String(e)) ? "no_capture_permission" : "capture_failed" };
  }
  const shot = await env.bitmap(url);
  let scale = Math.min(1, MAX_SIDE / Math.max(shot.width, shot.height));
  for (let attempt = 0; attempt < 4; attempt++, scale /= 2) {
    const w = Math.max(1, Math.round(shot.width * scale));
    const h = Math.max(1, Math.round(shot.height * scale));
    const c = env.canvas(w, h);
    const g = c.getContext("2d");
    if (!g) return { error: "capture_failed" };
    g.drawImage(shot.image, 0, 0, w, h);
    const k = dpr * scale;
    g.strokeStyle = OUTLINE;
    g.lineWidth = Math.max(2, 3 * k);
    g.strokeRect(rect.x * k, rect.y * k, rect.w * k, rect.h * k);
    const png = await c.convertToBlob({ type: "image/png" });
    if (png.size <= MAX_CLIP) return { png };
  }
  return { error: "clip_too_large" };
}
```

- [ ] **Step 4: Implement the pick flow**

`web/extension/src/sw/picks.ts`:

```ts
// One pick per tab, from the screenshot to the posted thread (spec
// 2026-10-05 §3.2, §9.4). The worker issues the pick ID (128 random bits);
// a composer port is taken only from its own tab and for a pick the worker
// issued. The thread is posted once both the body (from the composer) and
// the snapshot (from the overlay) are in.
import type { Anchor } from "../../../bridge/src/protocol";
import { type OverlayToWorker, type Rect, type WorkerToComposer, type WorkerToOverlay, isFromComposer } from "../messages";
import type { Api } from "./api";

export const PICK_TTL_MS = 600_000;

type PickState = {
  pickId: string; tabId: number; created: number; clip: Blob | null; clipError: string | null;
  anchor: Anchor | null; url: string | null; title: string; snapshot: string | null; snapshotError: string | null;
  body: string | null; port: chrome.runtime.Port | null; posting: boolean;
};
type Deps = {
  api: Pick<Api, "postThread">;
  capture(windowId: number, rect: Rect, dpr: number): Promise<{ png: Blob } | { error: string }>;
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  randomId(): string;
  now(): number;
};

async function dataUrl(b: Blob): Promise<string> {
  const bytes = new Uint8Array(await b.arrayBuffer());
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:image/png;base64,${btoa(s)}`;
}

export class Picks {
  private byTab = new Map<number, PickState>();
  constructor(private readonly d: Deps) {}

  private live(tabId: number, pickId: string): PickState | null {
    const p = this.byTab.get(tabId);
    return p && p.pickId === pickId && this.d.now() - p.created < PICK_TTL_MS ? p : null;
  }

  async capture(tabId: number, windowId: number, rect: Rect, dpr: number): Promise<{ pickId: string }> {
    const shot = await this.d.capture(windowId, rect, dpr);
    const pickId = this.d.randomId();
    this.byTab.get(tabId)?.port?.disconnect();
    this.byTab.set(tabId, {
      pickId, tabId, created: this.d.now(), clip: "png" in shot ? shot.png : null, clipError: "error" in shot ? shot.error : null,
      anchor: null, url: null, title: "", snapshot: null, snapshotError: null, body: null, port: null, posting: false,
    });
    return { pickId };
  }

  async attach(tabId: number, m: Extract<OverlayToWorker, { t: "pick" }>): Promise<void> {
    const p = this.live(tabId, m.pickId);
    if (!p) return;
    Object.assign(p, { anchor: m.anchor, url: m.url, title: m.title, snapshot: m.snapshot, snapshotError: m.snapshotError });
    await this.sendDraft(p);
    await this.maybePost(p);
  }

  attachComposer(port: chrome.runtime.Port, tabId: number): void {
    const pickId = port.name.slice("composer:".length);
    const p = this.live(tabId, pickId);
    if (!p || p.port) { port.disconnect(); return; }
    p.port = port;
    port.onMessage.addListener(m => {
      if (!isFromComposer(m)) return;
      if (m.t === "ready") void this.sendDraft(p);
      else if (m.t === "post") { p.body = m.body; void this.maybePost(p); }
      else if (m.t === "cancel") this.cancel(tabId, pickId);
    });
    port.onDisconnect.addListener(() => { if (p.port === port) p.port = null; });
  }

  cancel(tabId: number, pickId: string | null): void {
    const p = this.byTab.get(tabId);
    if (!p || (pickId !== null && p.pickId !== pickId)) return;
    this.byTab.delete(tabId);
    p.port?.disconnect();
    this.d.toOverlay(tabId, { t: "close-composer", pickId: p.pickId, posted: false });
  }

  private async sendDraft(p: PickState): Promise<void> {
    if (!p.port || !p.anchor) return;
    const m: WorkerToComposer = { t: "draft", anchor: p.anchor, clipUrl: p.clip ? await dataUrl(p.clip) : null, clipError: p.clipError, capturing: false };
    p.port.postMessage(m);
  }

  private async maybePost(p: PickState): Promise<void> {
    if (p.posting || p.body === null || !p.anchor || !p.url || (p.snapshot === null && p.snapshotError === null)) return;
    p.posting = true;
    const f = new FormData();
    f.set("url", p.url);
    f.set("title", p.title);
    f.set("anchor", JSON.stringify(p.anchor));
    f.set("body", p.body);
    f.set("snapshot", new Blob([p.snapshot ?? ""], { type: "text/html" }), "index.html");
    if (p.clip) f.set("clip", p.clip, "clip.png");
    try {
      const r = await this.d.api.postThread(f);
      p.port?.postMessage({ t: "posted", threadId: r.thread.id } satisfies WorkerToComposer);
      this.byTab.delete(p.tabId);
      this.d.toOverlay(p.tabId, { t: "close-composer", pickId: p.pickId, posted: true });
    } catch (e) {
      p.posting = false;
      p.port?.postMessage({ t: "failed", message: (e as Error).message } satisfies WorkerToComposer);
    }
  }
}
```

Wire it in `sw/main.ts`: build `const picks = new Picks({ api, capture: (w, r, d) => captureClip(chromeCapture, w, r, d), toOverlay: …, randomId: () => [...crypto.getRandomValues(new Uint8Array(16))].map(b => b.toString(16).padStart(2, "0")).join(""), now: () => Date.now() })`; in the runtime message listener, route `capture` (`picks.capture(tabId, sender.tab.windowId, m.rect, m.dpr)`), `pick` (`picks.attach`), `cancel` (`picks.cancel`) and `quiet` (a `FormData` of `url`, `title`, `snapshot` to `api.postSnapshot`, ignoring 409) there instead of to `tabs.fromOverlay`; in `onConnect`, replace the composer refusal with `picks.attachComposer(port, s.tab.id)`.

- [ ] **Step 5: The composer page**

`web/extension/src/composer/ComposerFrame.svelte`:

```svelte
<script lang="ts">
  // The composer over the page (spec 2026-10-05 L7): the shell's Composer,
  // in an extension page the overlay frames, talking to the worker over the
  // port named for its pick. Nothing typed here reaches the page.
  import Composer from "../../../shell/src/ui/Composer.svelte";
  import type { Draft } from "../../../shell/src/view/composer-model";
  import type { WorkerToComposer } from "../messages";

  let { port, pickId }: { port: chrome.runtime.Port; pickId: string } = $props();
  let draft = $state<Draft | null>(null);
  let waiting: { ok: () => void; fail: (e: Error) => void } | null = null;

  port.onMessage.addListener(async (m: WorkerToComposer) => {
    if (m.t === "draft") {
      const clip = m.clipUrl ? await (await fetch(m.clipUrl)).blob() : null;
      draft = { pickId, anchor: m.anchor, version: 0, clip, clipError: m.clipError ?? undefined, capturing: m.capturing };
    } else if (m.t === "posted") { waiting?.ok(); waiting = null; }
    else if (m.t === "failed") { waiting?.fail(new Error(m.message)); waiting = null; }
  });
  port.postMessage({ t: "ready" });

  const submit = (body: string) => new Promise<void>((ok, fail) => { waiting = { ok, fail }; port.postMessage({ t: "post", body }); });
</script>

{#if draft}
  <Composer {draft} onCancel={() => port.postMessage({ t: "cancel" })} onSubmit={submit} />
{:else}
  <p class="wait">Taking the screenshot…</p>
{/if}

<style>
  :global(body) { margin: 0; background: var(--surface, #fff); }
  .wait { margin: 16px; font: 14px/1.4 ui-sans-serif, -apple-system, system-ui, sans-serif; color: var(--muted, #6b6862); }
</style>
```

`web/extension/src/composer/main.ts`:

```ts
// The composer page's entry: its pick ID is the URL's fragment.
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { PICK_ID } from "../messages";
import ComposerFrame from "./ComposerFrame.svelte";

const pickId = location.hash.slice(1);
if (PICK_ID.test(pickId)) {
  const port = chrome.runtime.connect({ name: `composer:${pickId}` });
  mount(ComposerFrame, { target: document.getElementById("app")!, props: { port, pickId } });
}
```

- [ ] **Step 6: Run the tests and the build**

Run: `cd web && npx vitest run extension && npm run build && node scripts/bundle-size.mjs && npm run typecheck && npm run lint`
Expected: PASS; `composer.html` with its assets within 28 KiB gzip.

- [ ] **Step 7: Commit**

```bash
git add web/extension
git -c commit.gpgsign=false commit -m "Take a pick's screenshot, compose in an extension frame, and post the thread with its snapshot"
```

---

### Task 14: The side panel

**Files:**
- Create: `web/extension/src/panel/Panel.svelte`, `web/extension/src/panel/link.svelte.ts`, `web/extension/src/panel/adapt.ts`
- Create: `web/extension/src/panel/adapt.test.ts`, `web/extension/src/panel/Panel.test.ts`
- Modify: `web/extension/src/panel/main.ts`, `web/extension/src/sw/main.ts` (`panelAction`), `web/extension/src/sw/tabs.ts` (versions, participants, viewer, presence)

**Interfaces:**
- Consumes: Task 10's `Tabs`, `Api`; Task 8's `PanelState`, `PanelToWorker`, `WorkerToPanel`; `Sidebar.svelte` (props as in `web/shell/src/ui/Sidebar.svelte`), `agentName` (`web/shell/src/view/history-model.ts`).
- Produces:
  - `adapt.ts`: `asPages(threads: Thread[]): Thread[]` (a thread's route stands for its page) and `pageOfRoute(route: string | null): string`.
  - `link.svelte.ts`: `class PanelLink {state: PanelState (reactive); post(m: PanelToWorker): void}` over a `panel:<windowId>` port, following the window's active tab (`chrome.tabs.onActivated`, `chrome.tabs.onUpdated`).
  - The worker answers every `PanelToWorker` message (spec §9.4) and reports the owner's presence (`here` every 30 s while the panel shows a live page and is visible; never `away`, so hiding the panel lets the report lapse instead of marking the owner away in a shell tab; spec §9.5).
  - `PanelState.viewer` is the owner viewer (spec L6): the name the panel shows and sets is the owner's, shared with the shell and the CLI.

- [ ] **Step 1: Write the failing tests**

`web/extension/src/panel/adapt.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { asPages, pageOfRoute } from "./adapt";

describe("adapt", () => {
  it("lets the sidebar read a thread's route as its page", () => {
    const t = (route?: string) => ({ id: "x", anchor: { kind: "element", selector: "body", file: "index.html", ...(route ? { route } : {}) } }) as never;
    const [a, b] = asPages([t(), t("?tab=billing")]);
    expect(a.anchor.file).toBe("index.html");
    expect(b.anchor.file).toBe("?tab=billing");
    expect(pageOfRoute(null)).toBe("index.html");
    expect(pageOfRoute("#/users/7")).toBe("#/users/7");
  });
});
```

`web/extension/src/panel/Panel.test.ts`:

```ts
import { fireEvent, render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import type { PanelState, PanelToWorker } from "../messages";
import Panel from "./Panel.svelte";

const thread = {
  id: "01J9AAAAAAAAAAAAAAAAAAAAAA", artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: false,
  anchor: { kind: "element", selector: "#save", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
  comments: [{ id: "c1", thread_id: "01J9AAAAAAAAAAAAAAAAAAAAAA", author_kind: "viewer", author_name: "Alex", via_harness: null, body: "Too wide", created_at: "2026-10-05T10:00:00.000Z" }],
  created_at: "2026-10-05T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null, addressed_in: [],
};
function state(over: Partial<PanelState> = {}): PanelState {
  return {
    tabId: 3, url: "http://localhost:5173/", route: null, resolved: { [thread.id]: { id: thread.id, found: true, method: "exact", rect: null } },
    page: { artifact_id: "7q3k9mzx2b4t", origin: "http://localhost:5173", path: "/", page_url: "http://localhost:5173/", title: "Home", current_version: 1, url: "http://localhost:7480/a/7q3k9mzx2b4t" },
    threads: [thread as never], versions: [{ artifact_id: "7q3k9mzx2b4t", n: 1, label: null, created_at: "2026-10-05T10:00:00.000Z", files: {} }],
    working: [], participants: { people: [], agents: [{ handle: `a_${"1".repeat(22)}`, harness: "claude", live: true }] },
    viewer: { public_id: "u_x", display_name: "Alex" }, commentMode: false, enabled: true, selected: null, error: null, ...over,
  };
}
function link(s: PanelState) {
  const sent: PanelToWorker[] = [];
  return { sent, state: s, post: (m: PanelToWorker) => sent.push(m) };
}

describe("Panel", () => {
  it("lists the page's threads and sends one to the live agent", async () => {
    const l = link(state());
    render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText("Too wide")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: /Send to claude/ }), { detail: 1 });
    expect(l.sent).toContainEqual({ t: "send", threadId: thread.id, to: `a_${"1".repeat(22)}` });
  });

  it("asks for the owner's name while the owner has none", async () => {
    const l = link(state({ viewer: { public_id: "u_x", display_name: null } }));
    render(Panel, { props: { link: l as never } });
    const input = screen.getByLabelText("Your name") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "Mia" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(l.sent).toContainEqual({ t: "set-name", name: "Mia" });
  });

  it("shows a failure with a way to retry", async () => {
    const l = link(state({ error: { code: "host_missing", message: "Specified native messaging host not found." } }));
    render(Panel, { props: { link: l as never } });
    expect(screen.getByText(/clax init/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(l.sent).toContainEqual({ t: "retry" });
  });

  it("offers to start when Clax is off on the tab", () => {
    render(Panel, { props: { link: link(state({ enabled: false, page: null, threads: [] })) as never } });
    expect(screen.getByText(/Click the Clax button or press/)).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cd web && npx vitest run extension/src/panel`
Expected: FAIL: cannot resolve `./adapt`, `./Panel.svelte`.

- [ ] **Step 3: Implement**

`web/extension/src/panel/adapt.ts`:

```ts
// The shell's sidebar groups threads by page (`anchor.file`). On a live page
// the route plays that part (spec 2026-10-05 §7): threads on other routes
// read "on ?tab=billing", and the route-less view is the index.
import { INDEX_FILE } from "../../../bridge/src/protocol";
import type { Thread } from "../../../shell/src/threads";

export const pageOfRoute = (route: string | null): string => route ?? INDEX_FILE;
export const asPages = (threads: Thread[]): Thread[] => threads.map(t => ({ ...t, anchor: { ...t.anchor, file: pageOfRoute(t.anchor.route ?? null) } }));
```

`web/extension/src/panel/link.svelte.ts`:

```ts
// The side panel's link to the worker: one port per window, the state of
// the window's active tab, and a ping every 20 s that keeps the worker up
// while the panel is open (spec 2026-10-05 §9.5).
import type { PanelState, PanelToWorker, WorkerToPanel } from "../messages";

export class PanelLink {
  state = $state<PanelState | null>(null);
  private port: chrome.runtime.Port;
  constructor(windowId: number) {
    this.port = chrome.runtime.connect({ name: `panel:${windowId}` });
    this.port.onMessage.addListener((m: WorkerToPanel) => {
      if (m.t === "tab") this.state = m.state;
      else if (m.t === "failed" && this.state) this.state = { ...this.state, error: { code: m.code, message: m.message } };
    });
    // `?tab=<id>` pins the panel to one tab (the browser test opens the
    // panel's page in a tab of its own); otherwise it follows the window's
    // active tab.
    const pinned = Number(new URLSearchParams(location.search).get("tab")) || null;
    const follow = async () => {
      if (pinned !== null) { this.post({ t: "watch-tab", tabId: pinned }); return; }
      const [tab] = await chrome.tabs.query({ active: true, windowId });
      if (tab?.id !== undefined) this.post({ t: "watch-tab", tabId: tab.id });
    };
    if (pinned === null) {
      chrome.tabs.onActivated.addListener(i => { if (i.windowId === windowId) void follow(); });
      chrome.tabs.onUpdated.addListener((_id, c, tab) => { if (tab.active && tab.windowId === windowId && c.url) void follow(); });
    }
    setInterval(() => this.post({ t: "ping" }), 20_000);
    void follow();
  }
  post(m: PanelToWorker): void {
    this.port.postMessage(m);
  }
}
```

`web/extension/src/panel/Panel.svelte`:

```svelte
<script lang="ts">
  // The side panel (spec 2026-10-05 §6.4): the active tab's live page and
  // its threads in the shell's own sidebar, the Comment switch, the
  // owner's name (asked for only while the owner has none), and Clax's
  // menu for the page. Every action goes to the
  // worker, which alone talks to the daemon.
  import Sidebar from "../../../shell/src/ui/Sidebar.svelte";
  import { agentName } from "../../../shell/src/view/history-model";
  import type { PanelState, PanelToWorker } from "../messages";
  import { asPages, pageOfRoute } from "./adapt";

  let { link, now }: { link: { state: PanelState | null; post(m: PanelToWorker): void }; now?: Date } = $props();
  const s = $derived(link.state);
  let name = $state("");
  const live = $derived(s?.participants?.agents.find(a => a.live) ?? null);
  const help: Record<string, string> = {
    host_missing: "Clax is not set up for Chrome yet. Run `clax init` (or /clax:extension in Claude Code), then click Retry.",
    daemon_unavailable: "Clax could not start. See ~/.clax/logs/daemon.log, then click Retry.",
    no_capture_permission: "Click the Clax button or press ⌥⇧C to comment with a screenshot.",
  };
</script>

<main class="panel">
  {#if !s}
    <p class="hint">Connecting to Clax…</p>
  {:else}
    <header>
      <div class="title">
        <h1>{s.page?.title ?? "Clax"}</h1>
        {#if s.page}<p class="url">{s.page.page_url}{s.route ?? ""}</p>{/if}
      </div>
      {#if s.enabled}
        <button class="comment" aria-pressed={s.commentMode} onclick={() => link.post({ t: "comment-mode", on: !s.commentMode })}>Comment</button>
      {/if}
    </header>
    {#if s.error}
      <div class="notice" role="status">
        <p>{help[s.error.code] ?? s.error.message}</p>
        <button onclick={() => link.post({ t: "retry" })}>Retry</button>
      </div>
    {/if}
    {#if s.viewer && !s.viewer.display_name}
      <label class="name">Your name
        <input aria-label="Your name" maxlength="60" value={name} oninput={e => (name = e.currentTarget.value)}
          onkeydown={e => { if (e.key === "Enter" && name.trim()) link.post({ t: "set-name", name: name.trim() }); }} />
      </label>
    {/if}
    {#if !s.enabled}
      <p class="hint">Click the Clax button or press ⌥⇧C on a page to comment on it.</p>
    {:else if s.page}
      <Sidebar
        threads={asPages(s.threads)} resolved={s.resolved} selected={s.selected} file={pageOfRoute(s.route)} {now}
        versions={s.versions} shown={s.page.current_version} agent={agentName(live?.harness ?? null)}
        working={s.working} agents={s.participants?.agents} sendTo={live?.handle ?? null}
        onSelect={t => link.post({ t: "select", threadId: t.id })}
        onSend={t => link.post({ t: "send", threadId: t.id, to: live?.handle ?? null })}
        onResolve={t => link.post({ t: t.status === "open" ? "resolve" : "reopen", threadId: t.id })}
        onReply={(t, body) => link.post({ t: "reply", threadId: t.id, body })}
        onSeen={t => link.post({ t: "looked", threadIds: [t.id] })} />
      <footer>
        <a href={s.page.url} target="_blank" rel="noopener">Open in Clax</a>
        <button class="ghost" onclick={() => link.post({ t: "turn-off" })}>Turn off on {new URL(s.page.origin).host}</button>
      </footer>
    {:else}
      <p class="hint">No comments on this page yet. Press Comment, then click what you want to comment on.</p>
    {/if}
  {/if}
</main>

<style>
  .panel { display: flex; flex-direction: column; min-height: 100vh; background: var(--bg); color: var(--ink); font: 14px/1.45 ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif; }
  header { display: flex; gap: 8px; align-items: flex-start; padding: 12px 16px; border-bottom: 1px solid var(--line); }
  .title { flex: 1; min-width: 0; }
  h1 { margin: 0; font-size: 15px; font-weight: 600; overflow-wrap: anywhere; }
  .url { margin: 2px 0 0; color: var(--muted); font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; font-size: 12px; overflow-wrap: anywhere; }
  .notice, .hint, .name { margin: 12px 16px; }
  .hint { color: var(--muted); }
  footer { margin-top: auto; display: flex; justify-content: space-between; gap: 8px; padding: 12px 16px; border-top: 1px solid var(--line); }
</style>
```

(The CSS tokens `--bg`, `--ink`, `--muted`, `--line` are the shell's, from `theme.css`; check their names there and use those. The `Sidebar` props must match `web/shell/src/ui/Sidebar.svelte` exactly; add the optional ones the panel does not use only if the type checker asks.)

`web/extension/src/panel/main.ts`:

```ts
// The side panel's entry.
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { PanelLink } from "./link.svelte";
import Panel from "./Panel.svelte";

void chrome.windows.getCurrent().then(win => {
  mount(Panel, { target: document.getElementById("app")!, props: { link: new PanelLink(win.id!) } });
});
```

In `sw/main.ts`, fill `panelAction(tabId, m)`: `watch-tab` refreshes the tab (`tabs.route`) and loads `versions`/`participants` (`api.artifact`) and the viewer (`api.me`); `send` → `api.sendThread`, `send-batch` → `api.sendBatch`, `reply` → `api.comment`, `resolve`/`reopen`/`delete`, `looked`, `set-name` (then refresh the viewer), `select` (tell the overlay `scroll-to`), `comment-mode` (refuse with `failed` `no_capture_permission` when `chrome.permissions.contains({origins: ["<all_urls>"]})` is false and the tab was not given `activeTab` since its last navigation, which the worker tracks from gestures and `chrome.tabs.onUpdated` loads; otherwise toggle as the icon does), `navigate` (`chrome.tabs.update(tabId, {url: page_url + (route ?? "")})`), `turn-off` (`origins.forget`, close the overlay by reloading the tab), `retry` (`pairer.forget()` then `tabs.route`). Each failure posts `{t: "failed", code, message}` to the panel. Presence: while a panel port shows a live page and the panel is visible, the worker reports `here` (`api.presence(aid)`) every 30 s; when the port disconnects, the panel hides or the page changes it stops reporting and lets the owner's report lapse. It never reports `away` (spec §9.5).

- [ ] **Step 4: Run the tests, the build and the gates**

Run: `cd web && npx vitest run extension && npm run build && node scripts/bundle-size.mjs && npm run typecheck && npm run lint`
Expected: PASS; `sidepanel.html` with its assets within 64 KiB gzip.

- [ ] **Step 5: Commit**

```bash
git add web/extension
git -c commit.gpgsign=false commit -m "Show a live page's threads in Chrome's side panel with the shell's sidebar"
```

---

### Task 15: Live pages in the shell

**Files:**
- Modify: `web/shell/src/api.ts` (`Artifact.kind`, `Artifact.live`)
- Modify: `web/shell/src/threads.ts` (`Thread.addressed_pending`)
- Modify: `web/shell/src/view/gallery-model.ts`, `web/shell/src/view/gallery-model.test.ts` (`publisherText` for live pages)
- Modify: `web/shell/src/ui/GalleryCard.svelte` (the Live chip)
- Modify: `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/view/artifact-controller.ts` (Comment off on live pages; Open page)
- Modify: `web/shell/src/view/history-model.ts`, `web/shell/src/view/history-model.test.ts` (the pending address)
- Modify: the version menu's and the version moment's wording (`grep -rn "published" web/shell/src/ui/VersionPanel.svelte web/shell/src/view/changelog-model.ts web/shell/src/view/artifact-controller.ts` finds them)
- Test: `web/shell/src/topbar.test.ts`, `web/shell/src/gallery.test.ts`

**Interfaces:**
- Consumes: Task 2's artifact views (`kind`, `live: {origin, path, page_url}`); Task 3's `addressed_pending`.
- Produces: `publisherText(a)` returns `live page · <host><path>` for a live page; `historyOf` adds `{v: null, who: <agent>, agent: true, verb: "addressed it · waiting for a snapshot"}` for a pending address; the top bar's Comment is disabled on a live page with the title "Comment on the live page with the Clax extension", and an "Open page" link to `live.page_url` sits beside the title line.

- [ ] **Step 1: Write the failing tests**

Add to `web/shell/src/view/gallery-model.test.ts`:

```ts
  it("names a live page by its URL", () => {
    expect(publisherText(a("x", null, { kind: "live", live: { origin: "http://localhost:5173", path: "/settings", page_url: "http://localhost:5173/settings" } }))).toBe("live page · localhost:5173/settings");
  });
```

Add to `web/shell/src/view/history-model.test.ts`:

```ts
  it("shows an address waiting for a live page's next snapshot", () => {
    const t = { ...thread(), addressed_pending: { harness: "claude", at: "2026-10-05T10:05:00.000Z" } };
    const h = historyOf(t, [], by => by);
    expect(h.at(-1)).toEqual({ v: null, who: "claude", agent: true, verb: "addressed it · waiting for a snapshot" });
  });
```

(`thread()` is the test file's thread builder; use whichever it has.) Add to `web/shell/src/topbar.test.ts` a test that mounts the top bar for an artifact with `kind: "live"` (reuse the file's mounting helper and artifact fixture with `kind` and `live` added) and asserts that the Comment button is disabled, its `title` is "Comment on the live page with the Clax extension", and a link named "Open page" points at `live.page_url`. Add to `web/shell/src/gallery.test.ts` a test that a card for a live artifact shows the text "Live" and `localhost:5173/settings`.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cd web && npx vitest run shell/src/view/gallery-model.test.ts shell/src/view/history-model.test.ts shell/src/topbar.test.ts shell/src/gallery.test.ts`
Expected: FAIL on the new tests.

- [ ] **Step 3: Implement**

`web/shell/src/api.ts`, in `Artifact`:

```ts
  /** `html` (published by an agent) or `live` (a live page, made from Chrome with the Clax extension). */
  kind?: "html" | "live";
  /** A live page's key: its origin, its path, and both together. */
  live?: { origin: string; path: string; page_url: string } | null;
```

`web/shell/src/threads.ts`, in `Thread`:

```ts
  /** On a live page: an agent said the page shows its fix; the page's next snapshot will be listed as addressing the thread. */
  addressed_pending?: { harness: string; at: string } | null;
```

`web/shell/src/view/gallery-model.ts`, at the top of `publisherText`:

```ts
  if (a.kind === "live" && a.live) return `live page · ${a.live.page_url.replace(/^https?:\/\//, "")}`;
```

`web/shell/src/view/history-model.ts`, in `historyOf` before the resolved event:

```ts
  if (t.addressed_pending && t.status === "open") {
    out.push({ at: t.addressed_pending.at, e: { v: null, who: agentName(t.addressed_pending.harness), agent: true, verb: "addressed it · waiting for a snapshot" } });
  }
```

`web/shell/src/ui/GalleryCard.svelte`: inside `.mks` (render the span when the artifact is live even with no other markers):

```svelte
{#if p.a.kind === "live"}<span class="chip live">Live</span>{/if}
```

with `.chip.live` in the card's styles using the people tint tokens the other chips use. `TopbarIsland.svelte`: the Comment button becomes

```svelte
  {@const live = s.data.artifact.kind === "live"}
  <button class="comment" aria-pressed={s.commenting} disabled={s.deleted || live}
    title={live ? "Comment on the live page with the Clax extension" : undefined}
    onclick={() => ctl.toggleComment()}>Comment <span class="kc" aria-hidden="true">C</span></button>
```

and, before it, `{#if live && s.data.artifact.live}<a class="open-page hide-sm" href={s.data.artifact.live.page_url} target="_blank" rel="noopener">Open page</a>{/if}`. In `artifact-controller.ts`, make `toggleComment` and the C key do nothing on a live page (`if (this.get().data.artifact.kind === "live") return;`). Where versions are labelled "published" in the version menu and the version moment ("v4 published"), say "snapshot" for a live page ("snapshot v4", "v4 snapshot taken"). Check the names `s.data.artifact` against the controller's state shape and use what it holds.

- [ ] **Step 4: Run the tests**

Run: `cd web && npx vitest run shell && npm run typecheck && npm run lint`
Expected: PASS.

- [ ] **Step 5: Verify in the browser**

Run a scratch daemon (`CLAX_HOME=$(mktemp -d) cargo run -p clax-cli -- serve --foreground --port 0`), create a live page with a comment through `POST /api/live/threads` (the multipart of Task 2's test, with `curl -F`), open the gallery and the live page's `/a/<aid>` in a browser at desktop and phone width, light and dark: the card shows "Live" and the page URL, the top bar shows "Open page" and a disabled Comment with its hint, the version menu says "snapshot", and the pin shows on the snapshot. Take screenshots into the task's hand-off.

- [ ] **Step 6: Commit**

```bash
git add web/shell
git -c commit.gpgsign=false commit -m "Show live pages in the gallery and the artifact view"
```

---

### Task 16: End to end in Chromium against a Vite dev server, and the verification record

**Files:**
- Create: `web/e2e/live-site/index.html`, `web/e2e/live-site/main.js`
- Create: `web/e2e/extension-fixtures.ts`
- Create: `web/e2e/chrome-overlay.spec.ts`
- Modify: `web/playwright.config.ts` (the spec's project: it launches its own browsers)
- Modify: `docs/verification.md`, `docs/superpowers/specs/2026-09-28-clax-design.md` (§2 row D19 pointing to the chrome-overlay spec; §4 layout; §16 testing)

**Interfaces:**
- Consumes: everything above; `startDaemon` (`web/e2e/fixtures.ts`); `web/dist-extension-test/` (Task 8); the worker's `claxTest` hook (Task 10); `CLAX_E2E_BIN`.
- Produces: `web/e2e/chrome-overlay.spec.ts`, run by `npm run e2e` and so by `scripts/quality_gates.sh`.

- [ ] **Step 1: The fixture app**

`web/e2e/live-site/index.html`:

```html
<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Settings</title>
<style>main { font: 16px/1.5 system-ui; padding: 24px } #save { padding: 8px 16px }</style></head>
<body onload="window.loaded = true">
  <div id="app"></div>
  <script type="module" src="/main.js"></script>
</body>
</html>
```

`web/e2e/live-site/main.js`:

```js
// A dev-server app for the Chrome overlay's browser test: a form whose
// button label the test edits, so Vite's hot update re-renders #app.
const app = document.querySelector("#app");
const LABEL = "Save";
app.innerHTML = `<main><h1>Settings</h1><form><input type="password" name="pw" value="hunter2"><input type="hidden" name="csrf" value="tok123">${LABEL === "" ? "" : `<button id="save" type="button">${LABEL}</button>`}</form></main>`;
if (import.meta.hot) import.meta.hot.accept();
```

- [ ] **Step 2: The fixtures**

`web/e2e/extension-fixtures.ts`:

```ts
// The Chrome overlay's browser test fixture: a daemon of its own, a real
// Vite dev server on a copy of live-site/, and Chromium with the unpacked
// test build of the extension in a fresh profile whose NativeMessagingHosts
// names a launcher of this run's `clax native-host` for that daemon's home.
import { type BrowserContext, chromium, type Worker, test as base } from "@playwright/test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { type Daemon, startDaemon } from "./fixtures";

const web = fileURLToPath(new URL("..", import.meta.url));
export const EXT_DIR = join(web, "dist-extension-test");

export type Live = {
  daemon: Daemon; ctx: BrowserContext; sw: Worker; extId: string; site: ViteDevServer; siteDir: string; siteUrl: string;
  /** Stops the daemon and starts another on the same home and a new port. */
  restartDaemon(): Promise<void>;
};

export const test = base.extend<{ live: Live }>({
  // oxlint-disable-next-line no-empty-pattern
  live: async ({}, use) => {
    const daemon = await startDaemon();
    const siteDir = mkdtempSync(join(tmpdir(), "clax-live-site-"));
    cpSync(join(web, "e2e/live-site"), siteDir, { recursive: true });
    const site = await createServer({ root: siteDir, configFile: false, logLevel: "silent", server: { port: 0, host: "127.0.0.1" } });
    await site.listen();
    const siteUrl = site.resolvedUrls!.local[0].replace("127.0.0.1", "localhost");
    const profile = mkdtempSync(join(tmpdir(), "clax-chrome-"));
    const hostDir = join(profile, "NativeMessagingHosts");
    mkdirSync(hostDir, { recursive: true });
    const launch = join(profile, "launch.sh");
    writeFileSync(launch, `#!/bin/sh\nCLAX_HOME='${daemon.home}' exec '${process.env.CLAX_E2E_BIN}' native-host "$@"\n`);
    chmodSync(launch, 0o755);
    const ctx = await chromium.launchPersistentContext(profile, {
      channel: "chromium",
      args: [`--disable-extensions-except=${EXT_DIR}`, `--load-extension=${EXT_DIR}`],
    });
    const sw = ctx.serviceWorkers()[0] ?? (await ctx.waitForEvent("serviceworker"));
    const extId = new URL(sw.url()).host;
    writeFileSync(join(hostDir, "dev.empathic.clax.json"), JSON.stringify({ name: "dev.empathic.clax", description: "test", path: launch, type: "stdio", allowed_origins: [`chrome-extension://${extId}/`] }));
    const l: Live = {
      daemon, ctx, sw, extId, site, siteDir, siteUrl,
      async restartDaemon() {
        await l.daemon.stop({ keepHome: true });
        l.daemon = await startDaemon({ home: l.daemon.home });
      },
    };
    await use(l);
    await ctx.close();
    await site.close();
    await l.daemon.stop();
    rmSync(profile, { recursive: true, force: true });
    rmSync(siteDir, { recursive: true, force: true });
  },
});
export { expect } from "./fixtures";
```

(`home`, `base` and `token` are what `startDaemon` returns; check its return value and use its names. `extId` equals `EXTENSION_ID`, since the test build carries the same `key`; the first test asserts it.)

In `web/playwright.config.ts`, add a project `{ name: "chrome-overlay", testMatch: /chrome-overlay\.spec\.ts$/ }` and add that pattern to the `rest` project's `testIgnore`.

- [ ] **Step 3: Write the end-to-end tests**

`web/e2e/chrome-overlay.spec.ts`:

```ts
// The Chrome overlay end to end (spec 2026-10-05): the real extension in
// Chromium, the real native host and daemon, a real Vite dev server.
// Playwright cannot click the toolbar, open the side panel by its gesture
// or answer the permission prompt; the test build holds <all_urls> and the
// worker's `claxTest.comment` stands in for the icon. docs/verification.md
// lists what only a person can check.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { type Live, expect, test } from "./extension-fixtures";

const repo = fileURLToPath(new URL("../..", import.meta.url));
const EXTENSION_ID = /pub const EXTENSION_ID: &str = "([a-p]{32})"/.exec(readFileSync(join(repo, "crates/clax-core/src/extension.rs"), "utf8"))![1];

type Hook = {
  comment(tabId: number, url: string): Promise<void>;
  state(tabId: number): { commentMode: boolean; error: unknown; resolved: Record<string, { found: boolean }> } | undefined;
};
const hook = (live: Live) => ({
  comment: (tabId: number, url: string) => live.sw.evaluate(([id, u]) => (globalThis as unknown as { claxTest: Hook }).claxTest.comment(id, u), [tabId, url] as const),
  state: (tabId: number) => live.sw.evaluate(id => (globalThis as unknown as { claxTest: Hook }).claxTest.state(id) ?? null, tabId),
});

/** Polls `fn` until it returns a value other than null or undefined; fails after `ms`. */
async function until<T>(fn: () => Promise<T | null | undefined> | T | null | undefined, ms = 5000): Promise<T> {
  const deadline = Date.now() + ms;
  for (;;) {
    const v = await fn();
    if (v !== null && v !== undefined) return v;
    if (Date.now() > deadline) throw new Error("timed out");
    await new Promise(r => setTimeout(r, 20));
  }
}

async function tabIdOf(live: Live, url: string): Promise<number> {
  return live.sw.evaluate(async u => (await chrome.tabs.query({})).find(t => t.url?.startsWith(u))!.id!, url);
}

async function api(live: Live, path: string, init: RequestInit = {}, session?: string) {
  const headers: Record<string, string> = { authorization: `Bearer ${live.daemon.token}`, "content-type": "application/json" };
  if (session) headers["x-clax-session"] = session;
  const res = await fetch(live.daemon.base + path, { ...init, headers });
  return res.json();
}

test("comment on a dev server page, reach the agent, and follow a hot reload", async ({ live }) => {
  const { ctx, siteUrl } = live;
  const h = hook(live);
  expect(live.extId).toBe(EXTENSION_ID);
  // The agent watches the dev server, as the skill says.
  const session = await api(live, "/api/sessions", { method: "POST", body: JSON.stringify({ harness: "claude", harness_session_id: "e2e-live", cwd: "/tmp", pid: null, parent_pid: null }) });
  const sid: string = session.session?.id ?? session.id;
  const watched = await api(live, `/api/sessions/${sid}/live-watches`, { method: "PUT", body: JSON.stringify({ url: siteUrl }) });
  expect(watched.live_watch.scope).toBe(`${siteUrl}*`);

  const page = await ctx.newPage();
  await page.goto(siteUrl);
  await expect(page.locator("#save")).toHaveText("Save");
  const tabId = await tabIdOf(live, siteUrl);
  const t0 = Date.now();
  await h.comment(tabId, siteUrl);
  await until(async () => ((await h.state(tabId))?.commentMode ? true : null));
  console.log(`icon → comment mode on: ${Date.now() - t0} ms (reported, not judged)`);

  // A page script finds no shadow root to read.
  expect(await page.evaluate(() => (document.querySelector("clax-overlay") as HTMLElement | null)?.shadowRoot ?? null)).toBeNull();

  const t1 = Date.now();
  await page.locator("#save").click();
  const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
  await composer.locator("textarea").waitFor();
  console.log(`pick → composer: ${Date.now() - t1} ms (reported, not judged)`);
  await composer.locator("textarea").fill("The save button needs more room");
  await composer.getByRole("button", { name: "Post" }).click();

  // The thread, its clip and its snapshot are stored.
  const lookup = await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(siteUrl)}`).then(r => r.json());
  const aid: string = lookup.page.artifact_id;
  const thread = await until(async () => (await api(live, `/api/artifacts/${aid}/threads`)).threads[0]);
  const tid: string = thread.id;
  expect(thread.has_clip).toBe(true);
  const snapshot = await fetch(`${live.daemon.base}/api/artifacts/${aid}/versions/1/files/index.html`).then(r => r.text());
  expect(snapshot).toContain(">Save</button>");
  expect(snapshot).not.toMatch(/<script|onload|hunter2|tok123/);

  // The side panel lists it and sends it to the agent.
  const panel = await ctx.newPage();
  await panel.goto(`chrome-extension://${live.extId}/sidepanel.html?tab=${tabId}`);
  await expect(panel.getByText("The save button needs more room")).toBeVisible();
  await panel.getByRole("button", { name: /Send to claude/ }).click();
  const fb = await api(live, `/api/sessions/${sid}/feedback?tier=wait&wait=10`);
  expect(fb.text).toContain(`live page ${siteUrl}`);
  expect(fb.text).toContain("Snapshot: ");

  // A hot update keeps the pin; removing the button detaches it.
  const main = join(live.siteDir, "main.js");
  const found = async () => (await h.state(tabId))?.resolved[tid]?.found;
  writeFileSync(main, readFileSync(main, "utf8").replace('const LABEL = "Save"', 'const LABEL = "Save changes"'));
  await expect(page.locator("#save")).toHaveText("Save changes");
  await expect.poll(found).toBe(true);
  writeFileSync(main, readFileSync(main, "utf8").replace('const LABEL = "Save changes"', 'const LABEL = ""'));
  await expect(page.locator("#save")).toHaveCount(0);
  await expect.poll(found).toBe(false);
  await expect(panel.getByText("Detached")).toBeVisible();

  // The agent says it is fixed; the open page's next snapshot addresses the thread.
  await api(live, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", body: JSON.stringify({ body: "Fixed", author_kind: "agent", addressed: true }) }, sid);
  await expect(panel.getByText("Fixed")).toBeVisible();
  await expect.poll(async () => (await api(live, `/api/artifacts/${aid}/threads/${tid}`)).thread.addressed_in.length, { timeout: 15_000 }).toBe(1);

  // The gallery shows the live page, and its view shows the first snapshot with the thread.
  const shell = await ctx.newPage();
  await shell.goto(`${live.daemon.base}/`);
  await expect(shell.getByText("Live", { exact: true })).toBeVisible();
  // The shell in this browser and the extension are one owner identity (spec L6).
  const paired = await api(live, "/api/extension");
  const shellMe = await shell.evaluate(() => fetch("/api/viewers/me").then(r => r.json()));
  expect(shellMe.viewer.public_id).toBe(paired.viewer.public_id);
  await shell.goto(`${live.daemon.base}/a/${aid}/v/1`);
  await expect(shell.getByRole("button", { name: /^Comment/ })).toBeDisabled();
  await expect(shell.getByText("The save button needs more room")).toBeVisible();
});

test("the panel recovers when the daemon restarts on another port", async ({ live }) => {
  const h = hook(live);
  const page = await live.ctx.newPage();
  await page.goto(live.siteUrl);
  const tabId = await tabIdOf(live, live.siteUrl);
  await h.comment(tabId, live.siteUrl);
  const before = await live.sw.evaluate(() => chrome.storage.session.get("pairing"));
  await live.restartDaemon();
  await h.comment(tabId, live.siteUrl);
  await expect.poll(async () => (await h.state(tabId))?.error ?? null).toBeNull();
  const after = await live.sw.evaluate(() => chrome.storage.session.get("pairing"));
  expect(after.pairing.daemon).not.toBe(before.pairing.daemon);
});
```

In the same spec, load the extension from `<home>/extension` as `clax extension install` writes it (no key) and assert that the worker's `chrome.runtime.id` equals `clax_core::extension::extension_id_in_effect(home)` (exposed to the test through `clax extension status --json`), so the path derivation is checked against Chromium itself.

- [ ] **Step 4: Run it**

Run: `cd web && npm run build && npx playwright test e2e/chrome-overlay.spec.ts`
Expected: PASS. If pairing fails because this Chromium does not read `NativeMessagingHosts` from a profile given with `--user-data-dir`, confirm with `chrome://version` and the worker's error (`host_missing`), then pair the test build through its hook instead: mint a credential with the token (`POST /api/extension/credentials`) and store it with `chrome.storage.session.set({pairing: {...}})` from `sw.evaluate`; keep `crates/clax-cli/tests/native_host.rs` as the native host's proof, and say so in `docs/verification.md`.

- [ ] **Step 5: Run every gate**

Run: `scripts/quality_gates.sh`
Expected: `all gates passed`, with the new spec inside the web e2e lane and the run's total near its former time (report the per-gate table in the hand-off).

- [ ] **Step 6: The verification record and the main spec**

Append to `docs/verification.md` a "Clax in Chrome" section: the exact commands (`clax init`; Load unpacked of `~/.clax/extension`; `scripts/quality_gates.sh`), what `chrome-overlay.spec.ts` exercised on this machine (the real extension, native host, daemon and Vite dev server: pick, screenshot, sanitized snapshot, side panel, send, agent feedback with the live payload, hot update, detach, addressed snapshot, gallery and snapshot view, re-pairing after a restart), and, plainly, what was not exercised and only a person can check: the toolbar icon and its permission prompt, the side panel opened by the icon, Alt+Shift+C and the context menu, `activeTab` lapsing on navigation, Chrome stable, Brave and Edge host registration on macOS and Linux, an agent in a real Claude Code or Codex session watching a dev server through the MCP tool, the self-reload after `clax init` installs a newer extension, and the shell and side panel sharing the owner's marks by hand (read a thread in one; it is not new in the other). Also list, as the owner's steps outside Clax: creating the key in 1Password, running `scripts/extension-pubkey.sh` and committing `web/extension/key/key.pub.b64` (which changes the ID once), and the Web Store upload through `scripts/pack-extension.sh --first-upload` (spec L15, §6.7). CI never signs. In the main spec, add row D19 to §2 ("Chrome overlay: comment on any page; see `2026-10-05-chrome-overlay-design.md`"), the new paths to §4, and the new tests to §16.

- [ ] **Step 7: Commit**

```bash
git add web/e2e web/playwright.config.ts docs/verification.md docs/superpowers/specs/2026-09-28-clax-design.md
git -c commit.gpgsign=false commit -m "Test the Chrome overlay end to end against a Vite dev server, and record what was verified"
```

---

### Task 17: Signing through 1Password (owner-run)

Runs after Task 16; nothing earlier depends on it. Spec L15, §6.7.

**Files:**
- Create: `scripts/extension-pubkey.sh`, `scripts/pack-extension.sh`, `scripts/test-extension-signing.sh`
- Modify: `scripts/quality_gates.sh` (run `test-extension-signing.sh` in the scripts lane), `docs/verification.md`, the Claude Code plugin README's extension section

**Interfaces:**
- Consumes: `CLAX_EXTENSION_KEY_REF` (a full `op://…` reference; required by both scripts, which exit 2 naming it when unset); the release extension build from Task 8 (`web/dist-extension`); `op` (1Password CLI) and `openssl` on `PATH`.
- Produces: `web/extension/key/key.pub.b64` (one line, committed by the owner); `dist/clax-extension-<version>.zip` (Web Store upload) and, with `--crx`, `dist/clax-extension-<version>.crx`.

- [ ] **Step 1: Write the failing tests** — `scripts/test-extension-signing.sh` puts a fake `op` first on `PATH` that prints a throwaway RSA key generated in the test (`openssl genrsa 2048`) only when called as `op read "$CLAX_EXTENSION_KEY_REF"`, and records each call. Cases: `extension-pubkey.sh` writes the one-line public key that `openssl rsa -pubout -outform DER | base64` of the throwaway key gives, and the ID it prints equals `extension_id_from_key` of it (via `clax extension status --json` on a scratch build or a small Rust test helper); with `CLAX_EXTENSION_KEY_REF` unset both scripts exit 2 and call no `op`; `pack-extension.sh` produces a zip without `key.pem` by default, with it under `--first-upload`, and a `.crx` under `--crx` (skip the `.crx` case with a printed reason when no Chromium binary is found); after every run, no file containing the private key remains anywhere under the repository or `$TMPDIR` (the scripts' temp file is gone, including when the script is killed with SIGINT mid-run); the private key never appears in either script's stdout or stderr.
- [ ] **Step 2: Run them; they fail** (scripts missing).
- [ ] **Step 3: Implement** — `extension-pubkey.sh`: `op read "$CLAX_EXTENSION_KEY_REF" | openssl rsa -pubout -outform DER | base64 | tr -d '\n'` into `web/extension/key/key.pub.b64`, then print the resulting ID and remind the owner to commit the file and run `clax init` (the ID changes once). `pack-extension.sh`: build the release extension, `umask 077`, read the key into `mktemp` under a `trap 'rm -f "$key"' EXIT INT TERM`, zip `web/dist-extension` (adding the key as `key.pem` only with `--first-upload`), and with `--crx` run Chromium's `--pack-extension=<dir> --pack-extension-key="$key"`; never echo the key; refuse to run when `CI` is set ("signing is local and owner-run").
- [ ] **Step 4: Run the tests and `scripts/quality_gates.sh`**; expected: pass, with no `op` call outside the fake.
- [ ] **Step 5: Document** in `docs/verification.md` and the plugin README: create the key directly in 1Password (`openssl genrsa 2048 | op document create - --title "Clax extension key"` or an equivalent that leaves nothing on disk), set `CLAX_EXTENSION_KEY_REF`, run `scripts/extension-pubkey.sh`, commit the public key, and use `scripts/pack-extension.sh --first-upload` for the listing's first upload only.
- [ ] **Step 6: Commit**

```bash
git add scripts/extension-pubkey.sh scripts/pack-extension.sh scripts/test-extension-signing.sh scripts/quality_gates.sh docs/verification.md plugins/claude-code/README.md
git -c commit.gpgsign=false commit -m "Sign the extension from 1Password with owner-run scripts"
```
