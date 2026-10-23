//! `/_diag/…`: what a deployed Worker's platform answers, for
//! `smoke-remote.mjs`. Served only while `CLAX_DIAG_ROUTES` is `1` and to a
//! request bearing the owner's token, which must be set as the
//! `CLAX_TOKEN` secret (or var); 404 otherwise, as for any other path.
//!
//! - `GET /_diag/env`: the store, the hosted vars, and which bindings the
//!   Worker has, with the functions the `ARTIFACTS` binding exposes;
//! - `POST /_diag/artifacts/probe?repo=<name>`: the `ARTIFACTS` binding's
//!   calls on a scratch repository (`clax-diag-…`, created and deleted
//!   again), each with its time and its raw answer or thrown error
//!   (`name`, `message`, `code`, `numericCode` and its other properties);
//!   tokens are shown by their shape, never whole. It answers whether
//!   `get` of a missing repository throws or answers `null`, what the
//!   reads of a missing object answer, a token's format, the remote's
//!   smart HTTP advertisements as the Worker's own `fetch` sees them
//!   (under three ways of presenting the token), and what a fork answers
//!   while it is copied;
//! - `GET /_diag/artifacts/tree?repo=<name>&ref=<ref>`: `log`, then
//!   every tree under the ref's commit, recursively, as `readTree`
//!   answers them, and `readFile` of each blob's path's size: of a probe's
//!   scratch repository, or of an artifact's (`a-<ID>`) whose artifact
//!   lies within the owner's reach (the owner made it), else 404;
//! - `POST /_diag/repos/token?artifact=<ID>&scope=read|write`: a
//!   repository token handed to the owner, as the server's git-token route
//!   will mint one (`repos::worker::mint_git_token`), for an artifact
//!   within the owner's reach alone (404 otherwise);
//! - `POST /_diag/reconcile?artifact=<ID>`: asks the instance to record
//!   the artifact's pushed commits now, as an Artifacts `pushed` event
//!   does, and answers its status.

use serde_json::{Value, json};

/// The routes' prefix.
pub const PREFIX: &str = "/_diag/";

/// The shape of a token, never the token: its length, its prefix up to
/// the first `_`-separated parts, whether it carries an `?expires=` suffix
/// and when that is.
pub fn token_shape(token: &str) -> Value {
    let (secret, expires) = match token.split_once("?expires=") {
        Some((s, e)) => (s, Some(e)),
        None => (token, None),
    };
    let prefix: String = match secret.rfind('_') {
        Some(i) => secret[..=i].to_string(),
        None => String::new(),
    };
    let tail = &secret[prefix.len()..];
    json!({
        "length": token.len(),
        "prefix": prefix,
        "secret_length": tail.len(),
        "secret_is_hex": !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_hexdigit()),
        "expires_suffix": expires,
    })
}

/// Whether a key names a credential whose value is shown by its shape.
pub fn secret_key(key: &str) -> bool {
    matches!(key, "token" | "plaintext" | "secret")
}

/// A smart HTTP advertisement as `{service, refs, capabilities}`: the
/// `# service=` line, each ref line, and the first ref line's capabilities.
pub fn advertisement(body: &[u8]) -> Value {
    let mut lines = Vec::new();
    let mut at = 0;
    while at + 4 <= body.len() {
        let Some(n) = std::str::from_utf8(&body[at..at + 4])
            .ok()
            .and_then(|h| usize::from_str_radix(h, 16).ok())
        else {
            break;
        };
        if n == 0 {
            lines.push(None);
            at += 4;
            continue;
        }
        if n < 4 || at + n > body.len() {
            break;
        }
        lines.push(Some(
            String::from_utf8_lossy(&body[at + 4..at + n])
                .trim_end_matches('\n')
                .to_string(),
        ));
        at += n;
    }
    let mut service = None;
    let mut refs = Vec::new();
    let mut caps: Vec<String> = Vec::new();
    for l in lines.into_iter().flatten() {
        if let Some(s) = l.strip_prefix("# service=") {
            service = Some(s.to_string());
            continue;
        }
        let (r, c) = match l.split_once('\0') {
            Some((r, c)) => (r.to_string(), Some(c.to_string())),
            None => (l.clone(), None),
        };
        if let Some(c) = c {
            caps = c
                .split(' ')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
        }
        refs.push(r);
    }
    json!({ "service": service, "refs": refs, "capabilities": caps })
}

#[cfg(target_arch = "wasm32")]
pub use wasm::route;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use clax_core::secret::constant_time_eq;
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use serde_json::{Map, Value, json};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use worker::{Env, Fetch, Headers, Method, Request, RequestInit, Response, Result};

    use super::{advertisement, secret_key, token_shape};
    use crate::repos::tokens::{Holder, PERSON_TOKEN_TTL_S, Scope};
    use crate::repos::worker::{ARTIFACTS_BINDING, mint_git_token, repo_store};

    fn var(env: &Env, name: &str) -> String {
        env.var(name).map(|v| v.to_string()).unwrap_or_default()
    }

    fn secret(env: &Env, name: &str) -> String {
        env.secret(name)
            .map(|v| v.to_string())
            .unwrap_or_else(|_| var(env, name))
    }

    fn now() -> f64 {
        js_sys::Date::now()
    }

    fn param(url: &worker::Url, key: &str) -> Option<String> {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }

    /// `/_diag/…` (see the module docs); `None` when the routes are off,
    /// so the path goes on as any other.
    pub async fn route(env: &Env, req: &Request) -> Option<Result<Response>> {
        if var(env, "CLAX_DIAG_ROUTES") != "1" {
            return None;
        }
        Some(serve(env, req).await)
    }

    async fn serve(env: &Env, req: &Request) -> Result<Response> {
        let token = secret(env, "CLAX_TOKEN");
        let presented = req
            .headers()
            .get("authorization")?
            .and_then(|h| h.strip_prefix("Bearer ").map(str::to_string))
            .unwrap_or_default();
        if token.is_empty() {
            return Response::error("the diagnostics need the CLAX_TOKEN secret set\n", 404);
        }
        if !constant_time_eq(presented.as_bytes(), token.as_bytes()) {
            return Response::error("the owner token is required\n", 401);
        }
        let url = req.url()?;
        let path = url.path().trim_start_matches(super::PREFIX).to_string();
        match (req.method(), path.as_str()) {
            (Method::Get, "env") => Response::from_json(&environment(env)),
            (Method::Post, "artifacts/probe") => {
                let stamp = now() as u64;
                let repo = param(&url, "repo").unwrap_or_else(|| format!("clax-diag-{stamp}"));
                if !repo.starts_with("clax-diag-") {
                    return Response::error("a probe's repo is named clax-diag-…\n", 400);
                }
                Response::from_json(&probe(env, &repo).await)
            }
            (Method::Get, "artifacts/tree") => {
                let Some(repo) = param(&url, "repo") else {
                    return Response::error("repo is required\n", 400);
                };
                if !repo_in_reach(env, &repo).await? {
                    return Response::error("Not found\n", 404);
                }
                let rev = param(&url, "ref").unwrap_or_else(|| "main".into());
                Response::from_json(&tree(env, &repo, &rev).await)
            }
            (Method::Post, "repos/token") => {
                let Some(id) = param(&url, "artifact") else {
                    return Response::error("artifact is required\n", 400);
                };
                let artifact = match clax_core::ArtifactId::parse(&id) {
                    Ok(a) => a,
                    Err(e) => return Response::error(format!("{e}\n"), 400),
                };
                if !crate::owner_reaches(env, artifact.as_str()).await? {
                    return Response::error("Not found\n", 404);
                }
                let scope = param(&url, "scope")
                    .as_deref()
                    .and_then(Scope::parse)
                    .unwrap_or(Scope::Write);
                let origin = url.origin().ascii_serialization();
                let started = now();
                match mint_git_token(
                    env,
                    &artifact,
                    scope,
                    PERSON_TOKEN_TTL_S,
                    Some(&origin),
                    &Holder::default(),
                )
                .await
                {
                    Ok(t) => Response::from_json(&json!({
                        "ms": now() - started,
                        "id": t.id,
                        "remote": t.remote,
                        "token": t.token,
                        "token_shape": token_shape(&t.token),
                        "expires_at": t.expires_at,
                    })),
                    Err(e) => Response::from_json(&json!({
                        "ms": now() - started,
                        "error": e.to_string(),
                        "kind": format!("{e:?}").split('(').next().unwrap_or_default(),
                    }))
                    .map(|r| r.with_status(502)),
                }
            }
            (Method::Post, "reconcile") => {
                let Some(id) = param(&url, "artifact") else {
                    return Response::error("artifact is required\n", 400);
                };
                let started = now();
                let status = crate::reconcile_pushes(env, &id, &Holder::default()).await;
                Response::from_json(&json!({
                    "ms": now() - started,
                    "status": status.as_ref().ok(),
                    "error": status.err().map(|e| e.to_string()),
                }))
            }
            _ => Response::error("Not found\n", 404),
        }
    }

    /// Whether repository `repo` is one the diagnostics may read: a probe's
    /// scratch one (`clax-diag-…`), or an artifact's whose artifact lies
    /// within the owner's reach (`crate::owner_reaches`).
    async fn repo_in_reach(env: &Env, repo: &str) -> Result<bool> {
        if repo.starts_with("clax-diag-") {
            return Ok(true);
        }
        let Some(aid) = repo
            .strip_prefix("a-")
            .and_then(|a| clax_core::ArtifactId::parse(a).ok())
        else {
            return Ok(false);
        };
        crate::owner_reaches(env, aid.as_str()).await
    }

    fn binding(env: &Env, name: &str) -> JsValue {
        Reflect::get(env.as_ref(), &JsValue::from(name)).unwrap_or(JsValue::UNDEFINED)
    }

    /// The function names on `o` and its prototype chain, up to `Object`'s.
    fn functions(o: &JsValue) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = o.clone();
        for _ in 0..4 {
            if at.is_null() || at.is_undefined() {
                break;
            }
            let Ok(obj) = at.clone().dyn_into::<Object>() else {
                break;
            };
            for k in Object::get_own_property_names(&obj).iter() {
                let Some(k) = k.as_string() else { continue };
                if k == "constructor" || out.contains(&k) {
                    continue;
                }
                if Reflect::get(&at, &k.clone().into())
                    .is_ok_and(|v| v.is_instance_of::<Function>())
                {
                    out.push(k);
                }
            }
            at = Object::get_prototype_of(&at).into();
            if at.loose_eq(&Object::get_prototype_of(&Object::new()).into()) {
                break;
            }
        }
        out.sort();
        out
    }

    fn environment(env: &Env) -> Value {
        let present = |n: &str| !binding(env, n).is_undefined();
        let artifacts = binding(env, ARTIFACTS_BINDING);
        json!({
            "repo_store": var(env, "REPO_STORE"),
            "repo_store_parsed": repo_store(env).is_ok(),
            "hosted": var(env, "HOSTED"),
            "public_origin": var(env, "PUBLIC_ORIGIN"),
            "retired_content_origin": var(env, "RETIRED_CONTENT_ORIGIN"),
            "git_smart_http": var(env, "GIT_SMART_HTTP"),
            "setup_secret_set": !secret(env, "SETUP_SECRET").is_empty(),
            "bindings": {
                "ARTIFACTS": present(ARTIFACTS_BINDING),
                "ASSETS": present("ASSETS"),
                "REPO": present("REPO"),
                "CLAX_INSTANCE_DO": present("CLAX_INSTANCE_DO"),
                "WEB": present("WEB"),
            },
            "artifacts_functions": functions(&artifacts),
        })
    }

    /// A JavaScript value as JSON, credentials by their shape; at most
    /// `depth` levels down.
    fn describe(v: &JsValue, depth: u32) -> Value {
        if v.is_undefined() {
            return json!({ "undefined": true });
        }
        if v.is_null() {
            return Value::Null;
        }
        if let Some(b) = v.as_bool() {
            return b.into();
        }
        if let Some(n) = v.as_f64() {
            return json!(n);
        }
        if let Some(s) = v.as_string() {
            return s.into();
        }
        if v.is_instance_of::<Function>() {
            return json!({ "function": true });
        }
        if let Some(a) = v.dyn_ref::<Array>() {
            if depth == 0 {
                return json!({ "array_length": a.length() });
            }
            return a.iter().take(50).map(|x| describe(&x, depth - 1)).collect();
        }
        if v.is_instance_of::<js_sys::Error>() {
            return error(v);
        }
        let Ok(obj) = v.clone().dyn_into::<Object>() else {
            return json!({ "debug": format!("{v:?}") });
        };
        let mut out = Map::new();
        let class = Reflect::get(v, &"constructor".into())
            .ok()
            .and_then(|c| Reflect::get(&c, &"name".into()).ok())
            .and_then(|n| n.as_string());
        if let Some(c) = class.filter(|c| c != "Object") {
            out.insert("$class".into(), c.into());
        }
        for k in Object::keys(&obj).iter() {
            let Some(k) = k.as_string() else { continue };
            let val = Reflect::get(v, &k.clone().into()).unwrap_or(JsValue::UNDEFINED);
            let d = match val.as_string() {
                Some(s) if secret_key(&k) => token_shape(&s),
                _ if depth == 0 => json!({ "type": val.js_typeof().as_string() }),
                _ => describe(&val, depth - 1),
            };
            out.insert(k, d);
        }
        let fns = functions(v);
        if !fns.is_empty() {
            out.insert("$functions".into(), json!(fns));
        }
        Value::Object(out)
    }

    /// A thrown value: its name, message, code, numeric code and every
    /// other own property.
    fn error(e: &JsValue) -> Value {
        let field = |k: &str| Reflect::get(e, &k.into()).unwrap_or(JsValue::UNDEFINED);
        let mut out = Map::new();
        for k in ["name", "message", "code", "numericCode"] {
            let v = field(k);
            if !v.is_undefined() {
                out.insert(k.into(), describe(&v, 0));
            }
        }
        if let Ok(obj) = e.clone().dyn_into::<Object>() {
            for k in Object::get_own_property_names(&obj).iter() {
                let Some(k) = k.as_string() else { continue };
                if out.contains_key(&k) || k == "stack" {
                    continue;
                }
                out.insert(k.clone(), describe(&field(&k), 1));
            }
        }
        if out.is_empty() {
            out.insert("debug".into(), format!("{e:?}").into());
        }
        Value::Object(out)
    }

    /// One call's outcome: its time, and its value or what it threw.
    struct Called {
        ms: f64,
        value: std::result::Result<JsValue, JsValue>,
    }

    impl Called {
        fn report(&self, call: &str) -> Value {
            match &self.value {
                Ok(v) => {
                    json!({ "call": call, "ms": self.ms, "ok": true, "value": describe(v, 3) })
                }
                Err(e) => json!({ "call": call, "ms": self.ms, "ok": false, "threw": error(e) }),
            }
        }
    }

    /// `o[method](...args)` through `Reflect.apply` (a stub method's own
    /// `apply` is a remote call), awaited through `Promise.resolve` (an RPC
    /// call answers an `RpcPromise`, a thenable that is no `Promise`), as
    /// `repos::js`'s calls are.
    async fn raw(o: &JsValue, method: &str, args: &[JsValue]) -> Called {
        let started = now();
        let value = async {
            let f: Function = Reflect::get(o, &method.into())?
                .dyn_into()
                .map_err(|_| JsValue::from_str(&format!("no function {method}")))?;
            let argv: Array = args.iter().collect();
            let v = Reflect::apply(&f, o, &argv)?;
            JsFuture::from(Promise::resolve(&v)).await
        }
        .await;
        Called {
            ms: now() - started,
            value,
        }
    }

    fn opts(pairs: &[(&str, JsValue)]) -> JsValue {
        let o = Object::new();
        for (k, v) in pairs {
            let _ = Reflect::set(&o, &(*k).into(), v);
        }
        o.into()
    }

    fn string_at(v: &Called, key: &str) -> Option<String> {
        v.value
            .as_ref()
            .ok()
            .and_then(|o| Reflect::get(o, &key.into()).ok())
            .and_then(|s| s.as_string())
    }

    const ZERO: &str = "0000000000000000000000000000000000000000";

    async fn probe(env: &Env, repo: &str) -> Value {
        let api = binding(env, ARTIFACTS_BINDING);
        let mut steps: Vec<Value> = Vec::new();
        if api.is_undefined() {
            return json!({ "repo": repo, "binding": false, "steps": steps });
        }
        let missing = format!("{repo}-missing");
        steps.push(
            raw(&api, "get", &[missing.as_str().into()])
                .await
                .report("get(missing)"),
        );
        let created = raw(
            &api,
            "create",
            &[repo.into(), opts(&[("setDefaultBranch", "main".into())])],
        )
        .await;
        steps.push(created.report("create(repo, {setDefaultBranch: main})"));
        let create_token = string_at(&created, "token");
        let create_remote = string_at(&created, "remote");
        let got = raw(&api, "get", &[repo.into()]).await;
        steps.push(got.report("get(repo)"));
        let mut forked = None;
        if let Ok(h) = &got.value {
            steps.push(raw(h, "info", &[]).await.report("info()"));
            let tok = raw(h, "createToken", &["write".into(), 600.into()]).await;
            steps.push(tok.report("createToken(write, 600)"));
            steps.push(raw(h, "listTokens", &[]).await.report("listTokens()"));
            for m in ["readCommit", "readTree", "readBlob"] {
                steps.push(
                    raw(h, m, &[ZERO.into()])
                        .await
                        .report(&format!("{m}(zero hash)")),
                );
            }
            steps.push(
                raw(h, "readCommit", &["not-a-hash".into()])
                    .await
                    .report("readCommit(malformed)"),
            );
            steps.push(
                raw(
                    h,
                    "readFile",
                    &[opts(&[
                        ("ref", "main".into()),
                        ("path", "index.html".into()),
                    ])],
                )
                .await
                .report("readFile({ref: main, path: index.html}) on an empty repo"),
            );
            steps.push(
                raw(h, "log", &[opts(&[("ref", "main".into())])])
                    .await
                    .report("log({ref: main}) on an empty repo"),
            );
            // The remote as the Worker's own fetch reaches it.
            if let (Some(remote), Some(t)) = (create_remote.clone(), string_at(&tok, "plaintext")) {
                steps.extend(adverts(&remote, &t).await);
            }
            let to = format!("{repo}-fork");
            let f = raw(h, "fork", &[to.as_str().into()]).await;
            steps.push(f.report("fork(repo-fork) of an empty repo"));
            if f.value.is_ok() {
                forked = Some(to.clone());
                // What `get` answers while the copy is made, for at most ~10 s.
                let started = now();
                let mut seen = Vec::new();
                loop {
                    let g = raw(&api, "get", &[to.as_str().into()]).await;
                    let code = match &g.value {
                        Ok(_) => "ready".to_string(),
                        Err(e) => Reflect::get(e, &"code".into())
                            .ok()
                            .and_then(|c| c.as_string())
                            .unwrap_or_else(|| "threw".into()),
                    };
                    let done = code == "ready";
                    seen.push(json!({ "at_ms": now() - started, "answer": code }));
                    if done || now() - started > 10_000.0 {
                        break;
                    }
                    worker::Delay::from(std::time::Duration::from_millis(500)).await;
                }
                steps.push(
                    json!({ "call": "get(repo-fork) until ready", "ok": true, "value": seen }),
                );
            }
            if let Some(id) = string_at(&tok, "id") {
                steps.push(
                    raw(h, "revokeToken", &[id.as_str().into()])
                        .await
                        .report("revokeToken(id)"),
                );
                steps.push(
                    raw(h, "revokeToken", &[id.as_str().into()])
                        .await
                        .report("revokeToken(id) again"),
                );
            }
            if let Some(t) = &create_token {
                steps.push(
                    raw(h, "revokeToken", &[t.as_str().into()])
                        .await
                        .report("revokeToken(create's plaintext token)"),
                );
            }
            dispose(h);
        }
        if let Some(f) = forked {
            steps.push(
                raw(&api, "delete", &[f.as_str().into()])
                    .await
                    .report("delete(repo-fork)"),
            );
        }
        steps.push(
            raw(&api, "delete", &[repo.into()])
                .await
                .report("delete(repo)"),
        );
        steps.push(
            raw(&api, "delete", &[repo.into()])
                .await
                .report("delete(repo) again"),
        );
        steps.push(
            raw(&api, "get", &[repo.into()])
                .await
                .report("get(repo) after delete"),
        );
        json!({ "repo": repo, "binding": true, "steps": steps })
    }

    fn dispose(h: &JsValue) {
        let symbol =
            Reflect::get(&js_sys::global(), &"Symbol".into()).unwrap_or(JsValue::UNDEFINED);
        let Ok(key) = Reflect::get(&symbol, &"dispose".into()) else {
            return;
        };
        if key.is_undefined() {
            return;
        }
        if let Some(f) = Reflect::get(h, &key)
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
        {
            let _ = Reflect::apply(&f, h, &Array::new());
        }
    }

    /// `GET <remote>/info/refs?service=…` for both services, the token as a
    /// Bearer header whole, as a Bearer header without its `?expires=`
    /// suffix, and as Basic auth's password.
    async fn adverts(remote: &str, token: &str) -> Vec<Value> {
        let bare = token.split("?expires=").next().unwrap_or(token).to_string();
        let basic = format!("Basic {}", STANDARD.encode(format!("x:{bare}")));
        let ways = [
            ("bearer, whole token", format!("Bearer {token}")),
            ("bearer, without ?expires=", format!("Bearer {bare}")),
            ("basic, x:token", basic),
        ];
        let mut out = Vec::new();
        for service in ["git-receive-pack", "git-upload-pack"] {
            for (how, auth) in &ways {
                let url = format!(
                    "{}/info/refs?service={service}",
                    remote.trim_end_matches('/')
                );
                out.push(advert(&url, auth, &format!("GET info/refs {service} ({how})")).await);
            }
        }
        out
    }

    async fn advert(url: &str, authorization: &str, call: &str) -> Value {
        let started = now();
        let r = async {
            let headers = Headers::new();
            headers.set("authorization", authorization)?;
            headers.set("user-agent", "git/2.45.0 (clax-diag)")?;
            let mut init = RequestInit::new();
            init.with_method(Method::Get).with_headers(headers);
            let req = Request::new_with_init(url, &init)?;
            let mut res = Fetch::Request(req).send().await?;
            let status = res.status_code();
            let ct = res.headers().get("content-type")?.unwrap_or_default();
            let www = res.headers().get("www-authenticate")?;
            let body = res.bytes().await?;
            Ok::<_, worker::Error>(json!({
                "status": status,
                "content_type": ct,
                "www_authenticate": www,
                "advertisement": if status == 200 { advertisement(&body) } else {
                    Value::String(String::from_utf8_lossy(&body[..body.len().min(300)]).into_owned())
                },
            }))
        }
        .await;
        match r {
            Ok(v) => json!({ "call": call, "ms": now() - started, "ok": true, "value": v }),
            Err(e) => {
                json!({ "call": call, "ms": now() - started, "ok": false, "threw": e.to_string() })
            }
        }
    }

    async fn tree(env: &Env, repo: &str, rev: &str) -> Value {
        let api = binding(env, ARTIFACTS_BINDING);
        if api.is_undefined() {
            return json!({ "repo": repo, "binding": false });
        }
        let got = raw(&api, "get", &[repo.into()]).await;
        let Ok(h) = &got.value else {
            return json!({ "repo": repo, "get": got.report("get(repo)") });
        };
        let mut steps = Vec::new();
        let log = raw(
            h,
            "log",
            &[opts(&[("ref", rev.into()), ("limit", 5.into())])],
        )
        .await;
        steps.push(log.report("log({ref, limit: 5})"));
        let head = log
            .value
            .as_ref()
            .ok()
            .and_then(|l| l.dyn_ref::<Array>().map(|a| a.get(0)))
            .and_then(|c| Reflect::get(&c, &"hash".into()).ok())
            .and_then(|h| h.as_string());
        let mut entries = Vec::new();
        if let Some(head) = head {
            let c = raw(h, "readCommit", &[head.as_str().into()]).await;
            steps.push(c.report("readCommit(head)"));
            let root = string_at(&c, "treeHash");
            // Breadth first: (path, tree hash).
            let mut queue = root.map(|r| vec![(String::new(), r)]).unwrap_or_default();
            let mut reads = 0;
            while let Some((dir, hash)) = queue.pop() {
                reads += 1;
                if reads > 64 {
                    break;
                }
                let t = raw(h, "readTree", &[hash.as_str().into()]).await;
                let Ok(list) = &t.value else {
                    steps.push(t.report(&format!("readTree({dir}/)")));
                    continue;
                };
                let Some(list) = list.dyn_ref::<Array>() else {
                    steps.push(t.report(&format!("readTree({dir}/)")));
                    continue;
                };
                entries.push(json!({ "path": format!("{dir}/"), "hash": hash, "ms": t.ms }));
                for e in list.iter() {
                    let s = |k: &str| {
                        Reflect::get(&e, &k.into())
                            .ok()
                            .and_then(|v| v.as_string())
                            .unwrap_or_default()
                    };
                    let path = if dir.is_empty() {
                        s("name")
                    } else {
                        format!("{dir}/{}", s("name"))
                    };
                    if s("mode") == "40000" || s("mode") == "040000" || s("type") == "tree" {
                        queue.push((path, s("hash")));
                    } else {
                        entries.push(json!({
                            "path": path, "mode": s("mode"), "type": s("type"), "hash": s("hash"),
                        }));
                    }
                }
            }
            if let Some(first) = entries
                .iter()
                .find(|e| e.get("mode").is_some())
                .and_then(|e| e["path"].as_str())
                .map(String::from)
            {
                let f = raw(
                    h,
                    "readFile",
                    &[opts(&[
                        ("ref", rev.into()),
                        ("path", first.as_str().into()),
                    ])],
                )
                .await;
                let mut r = f.report(&format!("readFile({{ref, path: {first}}})"));
                if let Ok(b) = &f.value {
                    r["type"] = Reflect::get(b, &"type".into())
                        .ok()
                        .and_then(|t| t.as_string())
                        .into();
                    let size = Reflect::get(b, &"size".into())
                        .ok()
                        .and_then(|s| s.as_f64());
                    r["size"] = size.into();
                }
                steps.push(r);
            }
            if let Some(blob) = entries
                .iter()
                .find(|e| e.get("mode").is_some())
                .and_then(|e| e["hash"].as_str())
                .map(String::from)
            {
                let b = raw(h, "readBlob", &[blob.as_str().into()]).await;
                let mut r = b.report("readBlob(first blob)");
                if let Ok(v) = &b.value
                    && let Ok(buf) = raw(v, "arrayBuffer", &[]).await.value
                {
                    r["bytes"] = Uint8Array::new(&buf).length().into();
                }
                steps.push(r);
            }
        }
        dispose(h);
        json!({ "repo": repo, "ref": rev, "steps": steps, "entries": entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_shown_by_its_shape_only() {
        let t = format!("art_v1_{}?expires=1760000000", "ab".repeat(20));
        let s = token_shape(&t);
        assert_eq!(s["prefix"], "art_v1_");
        assert_eq!(s["secret_length"], 40);
        assert_eq!(s["secret_is_hex"], true);
        assert_eq!(s["expires_suffix"], "1760000000");
        assert!(!s.to_string().contains("abab"));
        let plain = token_shape("cxg_0123");
        assert_eq!(plain["prefix"], "cxg_");
        assert_eq!(plain["expires_suffix"], Value::Null);
    }

    #[test]
    fn an_advertisement_names_its_service_refs_and_capabilities() {
        fn pkt(s: &str) -> String {
            format!("{:04x}{s}", s.len() + 4)
        }
        let body = format!(
            "{}0000{}{}0000",
            pkt("# service=git-receive-pack\n"),
            pkt(&format!(
                "{} refs/heads/main\0report-status delete-refs ofs-delta agent=x\n",
                "1".repeat(40)
            )),
            pkt(&format!("{} refs/heads/other\n", "2".repeat(40))),
        );
        let a = advertisement(body.as_bytes());
        assert_eq!(a["service"], "git-receive-pack");
        assert_eq!(a["refs"].as_array().unwrap().len(), 2);
        assert_eq!(
            a["capabilities"],
            json!(["report-status", "delete-refs", "ofs-delta", "agent=x"])
        );
    }
}
