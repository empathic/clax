//! The daemon's HTTP cache for the `net` capability's fetches
//! ([`crate::net::Net`]), on disk under `CLAX_HOME/net-cache`, so a map's
//! tiles (and any other cacheable answer) are not downloaded again on every
//! view, as OpenStreetMap's tile usage policy asks.
//!
//! - **Which requests:** a `GET` without a body whose headers are only
//!   `accept`, `accept-language` and `user-agent` ([`key_of`]). Any other
//!   header (`authorization`, an API key, `cache-control`, a validator of the
//!   page's own) bypasses the cache both ways.
//! - **Which responses** ([`storable`]): a `200` answered by the URL asked
//!   for (no redirect), without `Set-Cookie`, `Vary: *`, or `Cache-Control`
//!   `no-store` or `private`, with a lifetime (`s-maxage`, `max-age`,
//!   `Expires`) or a validator (`ETag`, `Last-Modified`).
//! - **Shared across apps:** the key is the URL and those headers, not the
//!   app; only answers marked `public` or carrying no privacy directive are
//!   kept, and a cached answer is served only after the asking app's own
//!   host check passes.
//! - **Use:** a fresh entry answers without a request; a stale one is
//!   revalidated with `If-None-Match`/`If-Modified-Since`, and a `304`
//!   refreshes its headers and serves its body.
//! - **Size:** at most `max_bytes` on disk, least recently used out first.
//!   Recency survives a restart as each file's modification time.

use crate::net::{FetchRequest, FetchResponse};
use chrono::{DateTime, Utc};
use clax_core::working::Clock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The daemon's budget for cached responses on disk: 100 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 100 << 20;

/// The request headers a cacheable request may carry; each is part of the key.
const KEYED: &[&str] = &["accept", "accept-language", "user-agent"];

/// The cache key of `req`, or `None` when the cache must not answer it.
#[must_use]
pub fn key_of(req: &FetchRequest) -> Option<String> {
    if !req.method.eq_ignore_ascii_case("GET") || req.body.is_some() {
        return None;
    }
    let mut hs: Vec<(String, &str)> = Vec::new();
    for (k, v) in &req.headers {
        let k = k.to_ascii_lowercase();
        if !KEYED.contains(&k.as_str()) {
            return None;
        }
        hs.push((k, v.as_str()));
    }
    hs.sort();
    let mut h = Sha256::new();
    h.update(req.url.as_bytes());
    for (k, v) in hs {
        h.update(b"\n");
        h.update(k.as_bytes());
        h.update(b": ");
        h.update(v.as_bytes());
    }
    Some(
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

/// `Cache-Control`'s directives, lowercased, with their values unquoted.
fn directives(headers: &BTreeMap<String, String>) -> HashMap<String, Option<String>> {
    headers
        .get("cache-control")
        .map(|cc| {
            cc.split(',')
                .filter_map(|d| {
                    let mut kv = d.splitn(2, '=');
                    let k = kv.next()?.trim().to_ascii_lowercase();
                    if k.is_empty() {
                        return None;
                    }
                    let v = kv.next().map(|v| v.trim().trim_matches('"').to_string());
                    Some((k, v))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn http_date(v: Option<&String>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(v?.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// How long, in seconds, a response stays fresh from its `Date`; `None`
/// when it states no lifetime. `no-cache` is a lifetime of 0.
fn lifetime(headers: &BTreeMap<String, String>) -> Option<u64> {
    let cc = directives(headers);
    if cc.contains_key("no-cache") {
        return Some(0);
    }
    let secs = |k: &str| cc.get(k)?.as_deref()?.parse::<u64>().ok();
    if let Some(s) = secs("s-maxage").or_else(|| secs("max-age")) {
        return Some(s);
    }
    let expires = headers.get("expires")?;
    let Some(exp) = http_date(Some(expires)) else {
        // An invalid Expires means already expired.
        return Some(0);
    };
    let date = http_date(headers.get("date"))?;
    Some(u64::try_from((exp - date).num_seconds()).unwrap_or(0))
}

/// Whether the shared cache may keep a `status` response with `headers`
/// (already without cookies), `set_cookie` saying whether it had any.
#[must_use]
pub fn storable(status: u16, set_cookie: bool, headers: &BTreeMap<String, String>) -> bool {
    let cc = directives(headers);
    status == 200
        && !set_cookie
        && !cc.contains_key("no-store")
        && !cc.contains_key("private")
        && !headers
            .get("vary")
            .is_some_and(|v| v.split(',').any(|f| f.trim() == "*"))
        && (lifetime(headers).is_some()
            || headers.contains_key("etag")
            || headers.contains_key("last-modified"))
}

/// One cached response's metadata; its body follows it in the entry's file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    /// When the response (or its last revalidation) arrived, Unix seconds.
    pub received: i64,
}

impl Entry {
    fn age(&self, now: DateTime<Utc>) -> u64 {
        let at_receipt = self
            .headers
            .get("age")
            .and_then(|a| a.trim().parse::<u64>().ok())
            .unwrap_or(0);
        at_receipt + u64::try_from(now.timestamp() - self.received).unwrap_or(0)
    }

    /// Whether the entry may answer without asking the server.
    #[must_use]
    pub fn fresh(&self, now: DateTime<Utc>) -> bool {
        lifetime(&self.headers).is_some_and(|l| l > self.age(now))
    }

    /// The validators to send when revalidating.
    #[must_use]
    pub fn validators(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if let Some(e) = self.headers.get("etag") {
            v.push(("if-none-match".into(), e.clone()));
        }
        if let Some(m) = self.headers.get("last-modified") {
            v.push(("if-modified-since".into(), m.clone()));
        }
        v
    }

    /// The entry with a `304`'s headers merged in, received `now`.
    #[must_use]
    pub fn refreshed(&self, headers: &BTreeMap<String, String>, now: DateTime<Utc>) -> Entry {
        let mut e = self.clone();
        e.headers.remove("age");
        for (k, v) in headers {
            if k != "content-length" && k != "content-type" && k != "content-encoding" {
                e.headers.insert(k.clone(), v.clone());
            }
        }
        e.received = now.timestamp();
        e
    }

    /// The entry as the page's answer, its `age` as of `now`.
    #[must_use]
    pub fn response(&self, body: Vec<u8>, now: DateTime<Utc>) -> FetchResponse {
        let mut headers = self.headers.clone();
        headers.insert("age".into(), self.age(now).to_string());
        FetchResponse {
            url: self.url.clone(),
            status: self.status,
            headers,
            body,
        }
    }
}

#[derive(Default)]
struct Index {
    /// key -> (bytes on disk, last use).
    files: HashMap<String, (u64, u64)>,
    /// last use -> key, least recent first.
    order: BTreeMap<u64, String>,
    bytes: u64,
    tick: u64,
}

impl Index {
    fn touch(&mut self, key: &str, size: Option<u64>) {
        self.tick += 1;
        let tick = self.tick;
        if let Some((old_size, old_tick)) = self.files.remove(key) {
            self.order.remove(&old_tick);
            self.bytes -= old_size;
            let size = size.unwrap_or(old_size);
            self.files.insert(key.to_string(), (size, tick));
            self.bytes += size;
        } else if let Some(size) = size {
            self.files.insert(key.to_string(), (size, tick));
            self.bytes += size;
        } else {
            return;
        }
        self.order.insert(tick, key.to_string());
    }

    fn remove(&mut self, key: &str) {
        if let Some((size, tick)) = self.files.remove(key) {
            self.order.remove(&tick);
            self.bytes -= size;
        }
    }

    /// Keys to delete until at most `max` bytes are kept.
    fn evict(&mut self, max: u64) -> Vec<String> {
        let mut out = Vec::new();
        while self.bytes > max {
            let Some((_, key)) = self.order.pop_first() else {
                break;
            };
            if let Some((size, _)) = self.files.remove(&key) {
                self.bytes -= size;
            }
            out.push(key);
        }
        out
    }
}

/// The cache: entry files in one directory, indexed in memory.
pub struct NetCache {
    dir: PathBuf,
    max_bytes: u64,
    clock: Arc<dyn Clock>,
    index: Mutex<Index>,
}

impl NetCache {
    /// The cache in `dir` (made when missing), holding at most `max_bytes`;
    /// the entries already there are indexed by their modification times.
    ///
    /// # Errors
    /// When `dir` cannot be made or read.
    pub fn open(dir: &Path, max_bytes: u64, clock: Arc<dyn Clock>) -> std::io::Result<NetCache> {
        std::fs::create_dir_all(dir)?;
        let mut found = Vec::new();
        for f in std::fs::read_dir(dir)? {
            let f = f?;
            let name = f.file_name().to_string_lossy().into_owned();
            let meta = f.metadata()?;
            if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) && meta.is_file() {
                found.push((meta.modified().ok(), name, meta.len()));
            } else if name.ends_with(".tmp") {
                let _ = std::fs::remove_file(f.path());
            }
        }
        found.sort();
        let mut index = Index::default();
        for (_, key, size) in found {
            index.touch(&key, Some(size));
        }
        let cache = NetCache {
            dir: dir.to_path_buf(),
            max_bytes,
            clock,
            index: Mutex::new(index),
        };
        let gone = cache.index.lock().unwrap().evict(max_bytes);
        for k in gone {
            let _ = std::fs::remove_file(cache.dir.join(k));
        }
        Ok(cache)
    }

    /// The clock freshness is judged by.
    #[must_use]
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Bytes kept on disk.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.index.lock().unwrap().bytes
    }

    /// The entry under `key` and its body, marked as just used.
    pub async fn get(&self, key: &str) -> Option<(Entry, Vec<u8>)> {
        if !self.index.lock().unwrap().files.contains_key(key) {
            return None;
        }
        let path = self.dir.join(key);
        let read = tokio::task::spawn_blocking(move || {
            let raw = std::fs::read(&path)?;
            // Recency for the next start; best effort.
            if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            Ok::<_, std::io::Error>(raw)
        })
        .await
        .ok()
        .and_then(Result::ok);
        let parsed = read.and_then(|raw| {
            let nl = raw.iter().position(|&b| b == b'\n')?;
            let entry: Entry = serde_json::from_slice(&raw[..nl]).ok()?;
            Some((entry, raw[nl + 1..].to_vec()))
        });
        let mut index = self.index.lock().unwrap();
        if parsed.is_some() {
            index.touch(key, None);
        } else {
            index.remove(key);
            let _ = std::fs::remove_file(self.dir.join(key));
        }
        parsed
    }

    /// Keeps `entry` and `body` under `key`, then drops the least recently
    /// used entries past the budget. An entry over the whole budget is not
    /// kept. Failures leave the cache without the entry.
    pub async fn put(&self, key: &str, entry: &Entry, body: &[u8]) {
        let Ok(mut raw) = serde_json::to_vec(entry) else {
            return;
        };
        raw.push(b'\n');
        raw.extend_from_slice(body);
        let size = raw.len() as u64;
        if size > self.max_bytes {
            self.remove(key).await;
            return;
        }
        let dir = self.dir.clone();
        let k = key.to_string();
        let tmp_name = format!("{k}.{:016x}.tmp", rand::random::<u64>());
        let written = tokio::task::spawn_blocking(move || {
            let tmp = dir.join(tmp_name);
            std::fs::write(&tmp, &raw)?;
            std::fs::rename(&tmp, dir.join(&k)).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        })
        .await
        .is_ok_and(|r| r.is_ok());
        if !written {
            return;
        }
        let gone = {
            let mut index = self.index.lock().unwrap();
            index.remove(key);
            index.touch(key, Some(size));
            index.evict(self.max_bytes)
        };
        for k in gone {
            let _ = tokio::fs::remove_file(self.dir.join(k)).await;
        }
    }

    /// Drops the entry under `key`.
    pub async fn remove(&self, key: &str) {
        self.index.lock().unwrap().remove(key);
        let _ = tokio::fs::remove_file(self.dir.join(key)).await;
    }
}

/// The daemon's fetcher: [`crate::net::Net::default`] with a cache of
/// [`DEFAULT_MAX_BYTES`] in `home/net-cache`, or without one (logged) when
/// that directory cannot be used.
#[must_use]
pub fn daemon_net(home: &Path) -> crate::net::Net {
    let dir = home.join("net-cache");
    match NetCache::open(
        &dir,
        DEFAULT_MAX_BYTES,
        Arc::new(clax_core::working::SystemClock),
    ) {
        Ok(c) => crate::net::Net::default().with_cache(Arc::new(c)),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "net cache unavailable");
            crate::net::Net::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{Hop, HopResponse, Net, NetError, Resolver, Transport};
    use clax_core::working::ManualClock;
    use futures::future::BoxFuture;
    use std::net::SocketAddr;

    struct One;
    impl Resolver for One {
        fn resolve<'a>(
            &'a self,
            _host: &'a str,
            port: u16,
        ) -> BoxFuture<'a, std::io::Result<Vec<SocketAddr>>> {
            Box::pin(async move {
                Ok(vec![SocketAddr::new(
                    "93.184.216.34".parse().unwrap(),
                    port,
                )])
            })
        }
    }

    /// Answers from a script, keeping the hops it got.
    #[derive(Default)]
    struct Script {
        answers: Mutex<Vec<HopResponse>>,
        hops: Mutex<Vec<Hop>>,
    }
    impl Transport for Script {
        fn send(&self, hop: Hop) -> BoxFuture<'_, Result<HopResponse, NetError>> {
            self.hops.lock().unwrap().push(hop);
            let next = self.answers.lock().unwrap().remove(0);
            Box::pin(async move { Ok(next) })
        }
    }

    fn answer(status: u16, headers: &[(&str, &str)], body: &str) -> HopResponse {
        HopResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    struct Rig {
        net: Net,
        script: Arc<Script>,
        clock: Arc<ManualClock>,
        cache: Arc<NetCache>,
        _dir: tempfile::TempDir,
    }

    fn rig(max: u64) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-14T12:00:00Z"));
        let cache = Arc::new(NetCache::open(dir.path(), max, clock.clone()).unwrap());
        let script = Arc::new(Script::default());
        let net = Net::new(Arc::new(One), script.clone()).with_cache(cache.clone());
        Rig {
            net,
            script,
            clock,
            cache,
            _dir: dir,
        }
    }

    impl Rig {
        fn answer(&self, a: HopResponse) {
            self.script.answers.lock().unwrap().push(a);
        }
        fn sent(&self) -> usize {
            self.script.hops.lock().unwrap().len()
        }
        async fn get(&self, url: &str, headers: &[(&str, &str)]) -> FetchResponse {
            self.fetch(url, headers, &|_| true).await.unwrap()
        }
        async fn fetch(
            &self,
            url: &str,
            headers: &[(&str, &str)],
            allows: &(dyn Fn(&str) -> bool + Send + Sync),
        ) -> Result<FetchResponse, NetError> {
            let req = FetchRequest {
                url: url.into(),
                method: "GET".into(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body: None,
            };
            self.net.fetch(allows, req).await
        }
    }

    const TILE: &str = "https://tile.example.org/1/0/0.png";

    #[tokio::test]
    async fn a_fresh_answer_is_served_from_disk_until_it_expires() {
        let r = rig(DEFAULT_MAX_BYTES);
        r.answer(answer(
            200,
            &[
                ("cache-control", "max-age=600"),
                ("content-type", "image/png"),
            ],
            "tile",
        ));
        let first = r.get(TILE, &[("accept", "image/*")]).await;
        assert_eq!((first.status, first.body.as_slice()), (200, &b"tile"[..]));
        r.clock.advance(300);
        let hit = r.get(TILE, &[("accept", "image/*")]).await;
        assert_eq!(r.sent(), 1, "the second fetch is answered by the cache");
        assert_eq!(hit.body, b"tile");
        assert_eq!(hit.headers["age"], "300");
        assert_eq!(hit.headers["content-type"], "image/png");
        // Another app that may not reach the host gets no answer from the cache.
        let refused = r.fetch(TILE, &[("accept", "image/*")], &|_| false).await;
        assert_eq!(refused.unwrap_err().code(), "not_granted");
        // Other request headers are another entry.
        r.answer(answer(200, &[("cache-control", "max-age=600")], "json"));
        r.get(TILE, &[("accept", "application/json")]).await;
        assert_eq!(r.sent(), 2);
        // Expired, without a validator: fetched again.
        r.clock.advance(301);
        r.answer(answer(200, &[("cache-control", "max-age=600")], "tile 2"));
        assert_eq!(r.get(TILE, &[("accept", "image/*")]).await.body, b"tile 2");
        assert_eq!(r.sent(), 3);
        // The index survives a restart.
        let again = NetCache::open(&r.cache.dir, DEFAULT_MAX_BYTES, r.clock.clone()).unwrap();
        assert_eq!(again.bytes(), r.cache.bytes());
    }

    #[tokio::test]
    async fn a_stale_answer_is_revalidated_and_a_304_serves_the_kept_body() {
        let r = rig(DEFAULT_MAX_BYTES);
        let modified = "Tue, 13 Oct 2026 12:00:00 GMT";
        r.answer(answer(
            200,
            &[
                ("cache-control", "max-age=60"),
                ("etag", "\"v1\""),
                ("last-modified", modified),
            ],
            "tile",
        ));
        r.get(TILE, &[]).await;
        r.clock.advance(120);
        r.answer(answer(
            304,
            &[("cache-control", "max-age=600"), ("etag", "\"v1\"")],
            "",
        ));
        let res = r.get(TILE, &[]).await;
        assert_eq!((res.status, res.body.as_slice()), (200, &b"tile"[..]));
        let hop = r.script.hops.lock().unwrap()[1].clone();
        assert!(
            hop.headers
                .contains(&("if-none-match".into(), "\"v1\"".into()))
        );
        assert!(
            hop.headers
                .contains(&("if-modified-since".into(), modified.into()))
        );
        // The 304's lifetime now applies.
        r.clock.advance(500);
        assert_eq!(r.get(TILE, &[]).await.body, b"tile");
        assert_eq!(r.sent(), 2);
        // A changed resource replaces the entry.
        r.clock.advance(200);
        r.answer(answer(
            200,
            &[("cache-control", "max-age=600"), ("etag", "\"v2\"")],
            "tile v2",
        ));
        assert_eq!(r.get(TILE, &[]).await.body, b"tile v2");
        assert_eq!(r.get(TILE, &[]).await.body, b"tile v2");
        assert_eq!(r.sent(), 3);
    }

    #[tokio::test]
    async fn private_or_unshareable_answers_are_never_kept() {
        for headers in [
            vec![("cache-control", "no-store, max-age=600")],
            vec![("cache-control", "private, max-age=600")],
            vec![("cache-control", "max-age=600"), ("set-cookie", "s=1")],
            vec![("cache-control", "max-age=600"), ("vary", "*")],
            vec![("content-type", "image/png")],
        ] {
            let r = rig(DEFAULT_MAX_BYTES);
            r.answer(answer(200, &headers, "a"));
            r.answer(answer(200, &headers, "b"));
            r.get(TILE, &[]).await;
            assert_eq!(r.get(TILE, &[]).await.body, b"b", "{headers:?}");
            assert_eq!((r.sent(), r.cache.bytes()), (2, 0), "{headers:?}");
        }
        // Nor a redirected answer, or one that is not a 200.
        let r = rig(DEFAULT_MAX_BYTES);
        r.answer(answer(302, &[("location", "/other.png")], ""));
        r.answer(answer(200, &[("cache-control", "max-age=600")], "a"));
        r.answer(answer(404, &[("cache-control", "max-age=600")], "gone"));
        r.get(TILE, &[]).await;
        r.get("https://tile.example.org/missing.png", &[]).await;
        assert_eq!(r.cache.bytes(), 0);
    }

    #[tokio::test]
    async fn a_request_with_credentials_or_its_own_headers_bypasses_the_cache() {
        let r = rig(DEFAULT_MAX_BYTES);
        r.answer(answer(
            200,
            &[("cache-control", "public, max-age=600")],
            "open",
        ));
        r.get(TILE, &[]).await;
        for h in [
            ("authorization", "Bearer k"),
            ("x-api-key", "k"),
            ("if-none-match", "\"x\""),
            ("cache-control", "no-cache"),
        ] {
            r.answer(answer(
                200,
                &[("cache-control", "public, max-age=600")],
                "theirs",
            ));
            assert_eq!(r.get(TILE, &[h]).await.body, b"theirs", "{h:?}");
        }
        assert_eq!(r.sent(), 5);
        // The kept answer is the one without credentials, and none was added.
        assert_eq!(r.get(TILE, &[]).await.body, b"open");
        assert_eq!(r.sent(), 5);
        // A POST is never cached.
        let post = FetchRequest {
            url: TILE.into(),
            method: "POST".into(),
            headers: vec![],
            body: Some(b"x".to_vec()),
        };
        assert!(key_of(&post).is_none());
    }

    #[tokio::test]
    async fn the_cache_keeps_to_its_size_dropping_the_least_recently_used() {
        let r = rig(3 * 400);
        let body = "x".repeat(250);
        for i in 0..3 {
            r.answer(answer(200, &[("cache-control", "max-age=600")], &body));
            r.get(&format!("https://tile.example.org/{i}.png"), &[])
                .await;
        }
        let full = r.cache.bytes();
        assert!(full <= 1200, "{full}");
        // 0 is used again, so 1 is the least recently used.
        r.get("https://tile.example.org/0.png", &[]).await;
        r.answer(answer(200, &[("cache-control", "max-age=600")], &body));
        r.get("https://tile.example.org/3.png", &[]).await;
        assert!(r.cache.bytes() <= 1200);
        let before = r.sent();
        r.get("https://tile.example.org/0.png", &[]).await;
        r.get("https://tile.example.org/3.png", &[]).await;
        assert_eq!(r.sent(), before, "0 and 3 are kept");
        r.answer(answer(200, &[("cache-control", "max-age=600")], &body));
        r.get("https://tile.example.org/1.png", &[]).await;
        assert_eq!(r.sent(), before + 1, "1 was dropped");
        // An answer over the whole budget is not kept.
        let big = "y".repeat(2000);
        r.answer(answer(200, &[("cache-control", "max-age=600")], &big));
        r.answer(answer(200, &[("cache-control", "max-age=600")], &big));
        r.get("https://tile.example.org/big.png", &[]).await;
        r.get("https://tile.example.org/big.png", &[]).await;
        assert_eq!(r.sent(), before + 3);
        assert!(r.cache.bytes() <= 1200);
        // Files on disk match the index.
        let on_disk: u64 = std::fs::read_dir(&r.cache.dir)
            .unwrap()
            .map(|f| f.unwrap().metadata().unwrap().len())
            .sum();
        assert_eq!(on_disk, r.cache.bytes());
    }

    #[test]
    fn lifetimes_follow_cache_control_then_expires() {
        let h = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            lifetime(&h(&[("cache-control", "public, s-maxage=10, max-age=5")])),
            Some(10)
        );
        assert_eq!(lifetime(&h(&[("cache-control", "max-age=\"7\"")])), Some(7));
        assert_eq!(
            lifetime(&h(&[("cache-control", "no-cache, max-age=7")])),
            Some(0)
        );
        assert_eq!(
            lifetime(&h(&[
                ("date", "Wed, 14 Oct 2026 12:00:00 GMT"),
                ("expires", "Wed, 14 Oct 2026 13:00:00 GMT"),
            ])),
            Some(3600)
        );
        assert_eq!(lifetime(&h(&[("expires", "0")])), Some(0));
        assert_eq!(lifetime(&h(&[("etag", "\"x\"")])), None);
    }
}
