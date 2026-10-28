//! Signing as the owner with a passkey: the `/sign` page and its two routes.
//!
//! The owner composes a chitin message (a reply to a seat, or an approval
//! such as `approve bdelanghe/home#123 at <sha>`) or picks one a seat queued,
//! and approves it with a WebAuthn passkey. Clax holds no key and verifies
//! nothing itself: it queues the message with the home's chitin CLI
//! (`[sign]`, [`clax_core::config::HomeConfig::sign`]) as an unsigned intent
//! (`chitin say --as <title> --queue`, which signs nothing and grants
//! nothing), and the page approves it through chitin's passkey gateway on
//! the same origin (`/approve/v1/challenge/<id>`, `navigator.credentials.get`,
//! `/approve/v1/assertion/<id>`), whose assertion every receiving seat
//! verifies itself (chitin `docs/PROTOCOL.md`, "Passkey approval"). The
//! page is top-level on the daemon's own origin because a framed page
//! cannot call WebAuthn, and the passkey's relying party is that origin's
//! host (`tailscale serve`'s name, such as `phobos.tail556f5b.ts.net`).
//!
//! - `GET /sign` (`?to=<title>&text=<text>` fills the form): the page, to the
//!   owner's browser alone; 403 for anyone else.
//! - `GET /api/sign` → `{as, pending: [...]}`: `chitin pending --as <title>
//!   --json`, the items still waiting.
//! - `POST /api/sign/queue` `{to, text}` → 201 `{id, as, to, text}`: `chitin
//!   say --as <title> --queue <to> <text>`.
//!
//! The routes answer only the owner's browser ([`Identity::owner_browser`],
//! same origin), never an agent, and 404 without `[sign]` or on a hosted
//! daemon.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::config::SignConfig;
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// The most characters a queued message holds (chitin's own limit).
pub const MAX_TEXT: usize = 800;
/// How long one chitin run may take.
const CHITIN_TIMEOUT: Duration = Duration::from_secs(20);

fn config(s: &AppState) -> Result<&SignConfig, ApiError> {
    match (&s.hosted, &s.sign) {
        (None, Some(c)) => Ok(c),
        _ => Err(ApiError::not_found()),
    }
}

/// 403 `browser_required` unless the owner's browser makes the request (the
/// caller has passed `require_owner`).
fn owner_browser(who: &Identity) -> Result<(), ApiError> {
    if who.owner_browser() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "browser_required",
            "messages are signed from the owner's browser, with a passkey",
        ))
    }
}

/// Runs chitin with `args` (and `stdin` closed): its stdout on exit 0.
async fn chitin(c: &SignConfig, args: &[&str]) -> Result<String, ApiError> {
    let mut cmd = tokio::process::Command::new(&c.chitin);
    cmd.args(args)
        .env("CHITIN_TITLE", "clax")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = &c.state_dir {
        cmd.env("CHITIN_STATE_DIR", dir);
    }
    let failed = |what: String| {
        tracing::warn!(%what, "chitin");
        ApiError::new(StatusCode::BAD_GATEWAY, "chitin_failed", what)
    };
    let out = tokio::time::timeout(CHITIN_TIMEOUT, cmd.output())
        .await
        .map_err(|_| failed(format!("chitin took longer than {CHITIN_TIMEOUT:?}")))?
        .map_err(|e| failed(format!("could not run {}: {e}", c.chitin)))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let said = err
            .trim()
            .lines()
            .last()
            .unwrap_or("")
            .chars()
            .take(300)
            .collect::<String>();
        return Err(failed(format!(
            "chitin {} exited {}: {said}",
            args[0], out.status
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `GET /api/sign` → `{as, pending}`: the items queued as the title that are
/// neither declined nor expired, as `chitin pending --json` lists them.
pub async fn pending(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let c = config(&s)?;
    who.require_owner("signs as the owner")?;
    owner_browser(&who)?;
    let out = chitin(c, &["pending", "--as", &c.as_title, "--json"]).await?;
    Ok(Json(
        json!({"as": c.as_title, "pending": waiting(&out, &c.as_title)?}),
    ))
}

/// The items of `chitin pending --json` output queued as `as_title` that
/// are neither declined nor expired.
pub fn waiting(out: &str, as_title: &str) -> Result<Vec<Value>, ApiError> {
    let all: Vec<Value> = serde_json::from_str(out).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "chitin_failed",
            format!("chitin pending answered no list: {e}"),
        )
    })?;
    Ok(all
        .into_iter()
        .filter(|i| i["as"] == as_title && i["declined"] != true && i["expired"] != true)
        .collect())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueBody {
    to: String,
    text: String,
}

/// `POST /api/sign/queue` `{to, text}` → 201 `{id, as, to, text}`: the
/// message queued as the title, unsigned, for the page to approve with a
/// passkey. 400 `invalid_recipient` for a `to` that is not a chitin title,
/// 400 `invalid_text` for an empty text, one over [`MAX_TEXT`] characters,
/// or one with a control character other than a newline or tab.
pub async fn queue(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    body: Result<Json<QueueBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    d.refuse_agents()?;
    let c = config(&s)?;
    who.require_owner("signs as the owner")?;
    owner_browser(&who)?;
    let b = super::artifacts::body(body)?;
    if !clax_core::config::valid_chitin_title(&b.to) {
        return Err(ApiError::bad_request(
            "invalid_recipient",
            "to must be a chitin title, such as personal-super-human",
        ));
    }
    let n = b.text.chars().count();
    if b.text.trim().is_empty()
        || n > MAX_TEXT
        || b.text
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(ApiError::bad_request(
            "invalid_text",
            format!("text must be 1 to {MAX_TEXT} characters, without control characters"),
        ));
    }
    let out = chitin(c, &["say", "--as", &c.as_title, "--queue", &b.to, &b.text]).await?;
    let id = queued_id(&out).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "chitin_failed",
            format!(
                "chitin say --queue did not name the queued item: {}",
                out.trim()
            ),
        )
    })?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "as": c.as_title, "to": b.to, "text": b.text})),
    ))
}

/// The ID in `chitin say --queue`'s `queued <id>` line: 12 lowercase hex.
pub fn queued_id(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let id = l.trim().strip_prefix("queued ")?.trim();
        (id.len() == 12
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| id.to_string())
    })
}

/// `GET /sign`: the page, to the owner's browser alone.
pub async fn page(State(s): State<AppState>, d: DeniedToGrants, who: Identity) -> Response {
    if let Err(e) = d.refuse_agents() {
        return e.into_response();
    }
    if config(&s).is_err() {
        return ApiError::not_found().into_response();
    }
    if who.require_owner("signs as the owner").is_err() || !who.owner_browser() {
        return html(
            StatusCode::FORBIDDEN,
            "default-src 'none'",
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Sign</title></head><body><p>Only the owner signs here, from one of the owner's devices.</p></body></html>".to_string(),
        );
    }
    let nonce = {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::rng().fill_bytes(&mut b);
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    };
    html(
        StatusCode::OK,
        &format!(
            "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
        ),
        PAGE.replace("{nonce}", &nonce),
    )
}

fn html(status: StatusCode, policy: &str, body: String) -> Response {
    let mut res = (status, body).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if let Ok(v) = HeaderValue::from_str(policy) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    res
}

/// The page: a form to compose (or the query's message), the items waiting,
/// and the passkey approval of either through chitin's gateway. Every value
/// shown is set as text, never markup.
const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="referrer" content="no-referrer">
<title>Sign as you</title>
<style nonce="{nonce}">
:root { --bg:#fff; --fg:#1d1d1f; --muted:#6e6e73; --line:#d2d2d7; --accent:#0a66d8; --ok:#1b7f3b; --bad:#b3261e; --card:#f5f5f7; color-scheme: light dark; }
@media (prefers-color-scheme: dark) { :root { --bg:#000; --fg:#f5f5f7; --muted:#a1a1a6; --line:#3a3a3c; --accent:#4a9dff; --ok:#4cc26a; --bad:#ff6b5e; --card:#1c1c1e; } }
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--fg); font:16px/1.45 -apple-system, system-ui, sans-serif;
  padding: max(16px, env(safe-area-inset-top)) max(16px, env(safe-area-inset-right)) max(16px, env(safe-area-inset-bottom)) max(16px, env(safe-area-inset-left)); }
main { max-width: 40rem; margin: 0 auto; }
h1 { font-size: 1.5rem; margin: 0 0 .25rem; } h2 { font-size: 1.15rem; margin: 1.5rem 0 .5rem; }
p.lead { color: var(--muted); margin: 0 0 1rem; }
label { display:block; font-weight:600; margin: .75rem 0 .25rem; }
input, textarea, select { width:100%; font: inherit; color: inherit; background: var(--card); border:1px solid var(--line); border-radius: 10px; padding: .6rem .7rem; }
textarea { min-height: 7rem; resize: vertical; }
button { min-height: 48px; padding: 0 1.1rem; border-radius: 12px; border: 0; font: inherit; font-weight: 600; background: var(--accent); color: #fff; margin-top: .75rem; }
button:disabled { opacity: .5; }
.row { display:flex; gap:.5rem; flex-wrap: wrap; } .row > * { flex: 1 1 8rem; }
.item { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: .75rem; margin: .5rem 0; }
.meta { color: var(--muted); font-size: .9rem; overflow-wrap: anywhere; }
.text { white-space: pre-wrap; overflow-wrap: anywhere; margin: .4rem 0; }
#status { min-height: 1.5rem; margin-top: 1rem; font-weight: 600; }
#status.ok { color: var(--ok); } #status.bad { color: var(--bad); }
</style>
</head>
<body>
<main>
<h1>Sign as you</h1>
<p class="lead">Each message is approved with your passkey, and every seat checks the signature itself.</p>
<div id="status" role="status" aria-live="polite"></div>
<h2>Write a message</h2>
<form id="compose">
<label for="kind">Kind</label>
<select id="kind"><option value="message">Message</option><option value="pr">Approve a pull request</option><option value="contract">Approve a contract change</option></select>
<label for="to">To (a chitin seat)</label>
<input id="to" autocomplete="off" autocapitalize="off" spellcheck="false" required>
<div id="pr" hidden>
<div class="row"><div><label for="repo">Repository</label><input id="repo" placeholder="bdelanghe/home" autocapitalize="off" spellcheck="false"></div>
<div><label for="num">Number</label><input id="num" inputmode="numeric" placeholder="123"></div></div>
<label for="sha">Head commit</label><input id="sha" placeholder="the commit you reviewed" autocapitalize="off" spellcheck="false">
</div>
<div id="contract" hidden>
<label for="what">The change</label><input id="what" placeholder="what you approve, by name or link" autocapitalize="off">
</div>
<label for="text">Message</label>
<textarea id="text" maxlength="800" required></textarea>
<button id="send" type="submit">Sign and send with passkey</button>
</form>
<h2>Waiting for you</h2>
<div id="pending"><p class="meta">Loading…</p></div>
</main>
<script nonce="{nonce}">
"use strict";
const $ = (id) => document.getElementById(id);
const status = (text, kind) => { const s = $("status"); s.textContent = text; s.className = kind || ""; };
const dec = (s) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "=")), (c) => c.charCodeAt(0));
const enc = (b) => btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
async function call(path, init) {
  const r = await fetch(path, Object.assign({ credentials: "same-origin", cache: "no-store" }, init || {}));
  let body = {};
  try { body = await r.json(); } catch (_) {}
  return { ok: r.ok, status: r.status, body };
}
const reason = (r) => (r.body && (r.body.reason || (r.body.error && r.body.error.message) || r.body.message)) || ("HTTP " + r.status);
async function approve(id, to, text) {
  const c = await call("/approve/v1/challenge/" + encodeURIComponent(id));
  if (!c.body.ok) throw new Error(reason(c));
  if (c.body.intent.to !== to || c.body.intent.text !== text) throw new Error("the queued message is not the one shown; not signed");
  const pk = c.body.publicKey;
  const publicKey = Object.assign({}, pk, {
    challenge: dec(pk.challenge),
    allowCredentials: (pk.allowCredentials || []).map((x) => Object.assign({}, x, { id: dec(x.id) })),
  });
  status("Waiting for your passkey…");
  const cred = await navigator.credentials.get({ publicKey });
  const r = cred.response;
  const body = typeof cred.toJSON === "function" ? cred.toJSON() : {
    id: cred.id, type: cred.type,
    response: { clientDataJSON: enc(r.clientDataJSON), authenticatorData: enc(r.authenticatorData), signature: enc(r.signature), userHandle: r.userHandle ? enc(r.userHandle) : undefined },
  };
  const a = await call("/approve/v1/assertion/" + encodeURIComponent(id), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
  if (!a.body.ok) throw new Error(reason(a));
  return a.body;
}
async function run(button, work) {
  button.disabled = true;
  try { const done = await work(); status("Signed and sent to " + done.to + (done.frame_hash ? " (#" + done.frame_hash + ")" : ""), "ok"); }
  catch (e) { status("Not sent: " + (e && e.message ? e.message : e), "bad"); }
  finally { button.disabled = false; load(); }
}
function compose() {
  const k = $("kind").value;
  $("pr").hidden = k !== "pr"; $("contract").hidden = k !== "contract";
  if (k === "pr" && $("repo").value && $("num").value && $("sha").value)
    $("text").value = "approve " + $("repo").value.trim() + "#" + $("num").value.trim() + " at " + $("sha").value.trim();
  if (k === "contract" && $("what").value) $("text").value = "approve contract change: " + $("what").value.trim();
}
["kind", "repo", "num", "sha", "what"].forEach((id) => $(id).addEventListener("input", compose));
$("compose").addEventListener("submit", (ev) => {
  ev.preventDefault();
  const to = $("to").value.trim(), text = $("text").value;
  run($("send"), async () => {
    const q = await call("/api/sign/queue", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ to, text }) });
    if (!q.ok) throw new Error(reason(q));
    const done = await approve(q.body.id, to, text);
    return Object.assign({ to }, done);
  });
});
async function load() {
  const list = $("pending");
  const r = await call("/api/sign");
  list.textContent = "";
  if (!r.ok) { const p = document.createElement("p"); p.className = "meta"; p.textContent = reason(r); list.append(p); return; }
  if (!r.body.pending.length) { const p = document.createElement("p"); p.className = "meta"; p.textContent = "Nothing is waiting."; list.append(p); return; }
  for (const i of r.body.pending) {
    const d = document.createElement("div"); d.className = "item";
    const m = document.createElement("div"); m.className = "meta";
    m.textContent = "To " + i.to + " · queued by " + i.queued_by + " (unverified) · " + Math.floor(i.age_s / 60) + " min ago · #" + i.text_hash;
    const t = document.createElement("div"); t.className = "text"; t.textContent = i.text;
    const b = document.createElement("button"); b.type = "button"; b.textContent = "Approve with passkey";
    b.setAttribute("aria-label", "Approve with passkey: to " + i.to);
    b.addEventListener("click", () => run(b, async () => Object.assign({ to: i.to }, await approve(i.id, i.to, i.text))));
    d.append(m, t, b); list.append(d);
  }
}
const q = new URLSearchParams(location.search);
if (q.get("to")) $("to").value = q.get("to");
if (q.get("text")) $("text").value = q.get("text");
if (q.get("kind")) { $("kind").value = q.get("kind"); compose(); }
if (!window.PublicKeyCredential) status("This browser has no passkeys; open this page in Safari or Chrome.", "bad");
load();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queued_line_names_the_id() {
        assert_eq!(
            queued_id("queued 0a1b2c3d4e5f\n").as_deref(),
            Some("0a1b2c3d4e5f")
        );
        for out in [
            "",
            "queued\n",
            "queued 0A1B2C3D4E5F\n",
            "queued 0a1b\n",
            "written abc\n",
        ] {
            assert_eq!(queued_id(out), None, "{out:?}");
        }
    }

    #[test]
    fn only_waiting_items_of_the_title_are_listed() {
        let out = r#"[{"id":"0a1b2c3d4e5f","as":"bobby","to":"lander","text":"approve x","declined":false,"expired":false},
            {"id":"111111111111","as":"bobby","to":"lander","text":"old","declined":false,"expired":true},
            {"id":"222222222222","as":"bobby","to":"lander","text":"no","declined":true,"expired":false},
            {"id":"333333333333","as":"alex","to":"lander","text":"other","declined":false,"expired":false}]"#;
        let w = waiting(out, "bobby").unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0]["id"], "0a1b2c3d4e5f");
        assert!(waiting("[]", "bobby").unwrap().is_empty());
        assert!(waiting("nope", "bobby").is_err());
    }
}
