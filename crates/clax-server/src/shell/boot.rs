//! The first-load data the daemon puts into `artifact.html` for `/a/…`
//! (spec §8 Time to usable): the bootstrap block, the page title and, when
//! the frame mode is known, the content frame. String injection at comment
//! markers the shell build keeps; no template engine.

use super::route::ShellRoute;
use axum::http::{HeaderMap, header};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_http::views::artifact::with_owner;
use serde_json::{Value, json};

/// The frame's sandbox, as `FRAME_SANDBOX` in `web/shell/src/view/frame-host.ts`.
pub const FRAME_SANDBOX: &str =
    "allow-scripts allow-forms allow-modals allow-popups allow-downloads";
/// Set by the shell once it has decided the frame mode: `subdomain` or `sandbox`.
/// It names a mode only, never anything secret.
pub const FRAME_COOKIE: &str = "clax_frame";
const BOOT_MARK: &str = "<!--clax:boot-->";
const FRAME_MARK: &str = "<!--clax:frame-->";
const TITLE_MARK: &str = "<h1>Clax</h1>";

/// What goes into `artifact.html`: the bootstrap JSON (already escaped by
/// [`script_json`]), the frame's markup when the mode is known, the title.
pub struct Injected {
    pub boot: String,
    pub frame: Option<String>,
    pub title: String,
}

/// `v` as JSON safe inside a `<script>` element: `<`, `>`, `&`, U+2028 and
/// U+2029 (which occur only inside JSON strings) become `\u` escapes.
pub fn script_json(v: &Value) -> String {
    let s = serde_json::to_string(v).expect("a JSON value serialises");
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes text for an HTML element or a double- or single-quoted attribute.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// JavaScript's `encodeURIComponent`.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Where the frame shows `file` of version `n`, as `pageSrc` in `web/shell/src/origin.ts`:
/// under `k/<key>/` when the version has a content key (a hosted daemon's,
/// [`clax_http::hosted::content_key`]).
pub fn page_src(id: &str, n: u64, origin: Option<&str>, key: Option<&str>, file: &str) -> String {
    let mut root = match origin {
        Some(o) => format!("{o}/v/{n}/"),
        None => format!("/c/{id}/v/{n}/"),
    };
    if let Some(k) = key {
        root = format!("{root}k/{k}/");
    }
    if file == "index.html" {
        return root;
    }
    root + &file
        .split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// The content frame's markup, with the attributes `FrameHost.show` in
/// `web/shell/src/view/frame-host.ts` gives it: sandboxed exactly when there
/// is no artifact origin.
pub fn frame_html(src: &str, sandboxed: bool) -> String {
    let sandbox = if sandboxed {
        format!(" sandbox=\"{FRAME_SANDBOX}\"")
    } else {
        String::new()
    };
    format!(
        "<iframe class=\"frame\" title=\"artifact content\" src=\"{}\" allow=\"clipboard-write; fullscreen\"{sandbox}></iframe>",
        html_escape(src)
    )
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// The frame mode the shell will choose, when the daemon can know it: `Some(None)`
/// for sandbox (always on a non-loopback host, where the shell has no artifact
/// origins), `Some(Some(origin))` for the subdomain the cookie names; `None`
/// when a loopback browser has not decided yet (no cookie, or a value that is
/// neither mode).
fn frame_origin(headers: &HeaderMap, id: &str) -> Option<Option<String>> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let (name, port) = match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, Some(p)),
        _ => (host, None),
    };
    if !name.eq_ignore_ascii_case("localhost") && name != "127.0.0.1" {
        return Some(None);
    }
    match cookie(headers, FRAME_COOKIE).as_deref() {
        Some("subdomain") => Some(Some(format!(
            "http://{id}.localhost{}",
            port.map(|p| format!(":{p}")).unwrap_or_default()
        ))),
        Some("sandbox") => Some(None),
        _ => None,
    }
}

/// Removes the session IDs the artifact API carries (the owner session, and
/// the session that published each version): the shell never reads them, and
/// the bootstrap holds nothing that names a session.
fn without_sessions(artifact: Value, versions: Value) -> Value {
    let mut v = json!({"artifact": artifact, "versions": versions});
    clax_http::views::artifact::strip_sessions(&mut v);
    v
}

/// Everything `artifact.html` gets for `route`, or `None` when it names no
/// artifact the store has. The data is what an unauthenticated browser reads
/// from `GET /api/artifacts/<id>` (less its session IDs),
/// `GET /api/artifacts/<id>/threads?include_resolved=true` without the token
/// (no `clip_path`), and `GET /api/viewers/me` for the request's existing
/// viewer (the owner's for the owner cookie or a hosted session of the
/// owner's, see [`clax_http::identity`]); no threads for a hosted daemon's
/// person whose rights do not read comments there: never
/// the token or a cookie, and no viewer is created (but the owner's row,
/// made on first use).
pub async fn assemble(
    s: &AppState,
    route: ShellRoute,
    headers: &HeaderMap,
    extensions: &axum::http::Extensions,
) -> Result<Option<Injected>, ApiError> {
    let ShellRoute::Artifact { id, version, file } = route else {
        return Ok(None);
    };
    let who =
        clax_http::identity::Identity::of(headers, &s.token, clax_http::identity::peer_of(extensions));
    // A hosted daemon's browser is the viewer its session signs in as.
    let who = match extensions.get::<clax_http::hosted::HostedRequest>() {
        Some(h) => h.identity(who),
        None => who,
    };
    let owner_browser = who.is_owner();
    let codex = s.feedback_ctx().codex_push();
    let wakes = crate::routes::sessions::Wakes::of(s);
    let lookup = id.clone();
    let working = s.working.for_artifact(id.as_str());
    // The source of a fork is named for a reader who may read it, and
    // Fork offered as `GET /api/artifacts/<id>` offers it.
    let member = extensions
        .get::<clax_http::hosted::HostedRequest>()
        .and_then(|h| h.member.clone());
    let member_rights = member.as_ref().map(|m| m.rights.clone());
    let hosted = s.hosted.is_some();
    let found = s
        .store_call(move |st| {
            let Some(a) = st.get_artifact(&lookup)? else {
                return Ok(None);
            };
            let versions = st.list_versions(&lookup)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            // Each thread's delivery state as this browser reads it
            // (spec 2026-10-08 §10, §18).
            let viewer = who.browser_viewer(st)?;
            let reads = match &viewer {
                _ if owner_browser => clax_store::DeliveryViewer::Owner,
                Some(v) if v.person && v.person_revoked_at.is_none() => {
                    clax_store::DeliveryViewer::Person(v.public_id.clone())
                }
                _ => clax_store::DeliveryViewer::Visitor,
            };
            // A person whose rights do not read comments here gets none.
            let comments = clax_http::hosted::reads_comments(st, member.as_ref(), lookup.as_str())?;
            let mut threads = Vec::new();
            let mut cursor: Option<String> = None;
            if comments {
                loop {
                    let (page, next) = st.list_threads(&lookup, true, cursor.as_deref(), 200)?;
                    threads.extend(clax_http::views::thread::thread_views_as(
                        st, &page, codex, false, &reads,
                    )?);
                    match next {
                        Some(c) => cursor = Some(c),
                        None => break,
                    }
                }
            }
            let attention = match &viewer {
                Some(v) => Some(st.attention(&v.id, &lookup)?),
                None => None,
            };
            let mut participants = st.participants(&lookup)?;
            crate::routes::sessions::Wakes::mark_with(
                wakes.for_viewer(&reads).as_ref(),
                &mut participants,
            );
            let mut artifact = with_owner(
                &a,
                owner.as_ref(),
                &working,
                &participants,
                clax_http::views::artifact::live_part(st, &a)?.as_ref(),
            );
            let caller = who.caller(st, None);
            // A caller who cannot be told is read as no one.
            let reader = crate::routes::artifacts::SourceReader::new(
                caller
                    .as_ref()
                    .map_or(clax_http::auth::Caller::Anonymous, Clone::clone),
                member.clone(),
                hosted,
            );
            if let Some(lineage) =
                crate::routes::artifacts::forked_from_view(st, &a, &reader, None)?
            {
                artifact["forked_from"] = lineage;
            }
            let may_fork = match caller {
                Ok(c) if a.kind != clax_core::live::KIND_LIVE => {
                    crate::routes::artifacts::fork_refusal(st, &c, member_rights.as_deref(), &a.id)?
                        .is_none()
                }
                _ => false,
            };
            artifact["may_fork"] = json!(may_fork);
            Ok(Some((a, artifact, versions, threads, viewer, attention)))
        })
        .await?;
    let Some((a, artifact_json, versions, threads, viewer, attention)) = found else {
        return Ok(None);
    };
    let n = version.unwrap_or(u64::from(a.current_version));
    let holds = versions
        .iter()
        .find(|v| u64::from(v.n) == n)
        .is_some_and(|v| file == "index.html" || v.files.contains_key(&file));
    // A hosted daemon's content origin frames every page sandboxed, there.
    let content_origin = s.hosted.as_ref().and_then(|h| h.content_origin());
    let mode = match content_origin {
        Some(_) => Some(None),
        None => frame_origin(headers, id.as_str()),
    };
    let (frame, frame_json) = match mode {
        Some(origin) if holds => {
            let key = s
                .hosted
                .as_ref()
                .map(|_| clax_http::hosted::content_key(&s.token, id.as_str(), n));
            let src = page_src(id.as_str(), n, origin.as_deref(), key.as_deref(), &file);
            let src = format!("{}{src}", content_origin.unwrap_or(""));
            (
                Some(frame_html(&src, origin.is_none())),
                json!({"mode": if origin.is_some() { "subdomain" } else { "sandbox" }, "src": src}),
            )
        }
        _ => (None, Value::Null),
    };
    let mut artifact = without_sessions(
        artifact_json,
        serde_json::to_value(&versions).expect("versions serialise"),
    );
    clax_http::hosted::stamp_content_keys(s, &mut artifact);
    let mut boot = json!({
        "v": 1,
        "artifact": artifact,
        "threads": threads,
        "viewer": viewer,
        "frame": frame_json,
    });
    if let Some(att) = attention {
        boot["attention"] = json!(att);
    }
    // As `GET /api/viewers/me` marks it.
    if owner_browser && boot["viewer"].is_object() {
        boot["viewer"]["owner"] = json!(true);
    }
    Ok(Some(Injected {
        boot: script_json(&boot),
        frame,
        title: a.title.clone(),
    }))
}

/// `template` (the built `artifact.html`) with `i` injected at its markers,
/// or with the markers removed when there is nothing to inject. Every
/// injected string is escaped (the title as HTML text, the bootstrap by
/// [`script_json`], the frame's `src` as an attribute), so none can form a
/// marker or close its element.
pub fn inject(template: &str, i: Option<&Injected>) -> String {
    let Some(i) = i else {
        return template
            .replacen(BOOT_MARK, "", 1)
            .replacen(FRAME_MARK, "", 1);
    };
    template
        .replacen(
            TITLE_MARK,
            &format!("<h1>{}</h1>", html_escape(&i.title)),
            1,
        )
        .replacen(FRAME_MARK, i.frame.as_deref().unwrap_or(""), 1)
        .replacen(
            BOOT_MARK,
            &format!(
                "<script type=\"application/json\" id=\"clax-boot\">{}</script>",
                i.boot
            ),
            1,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_src_agrees_with_the_shell() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../../../web/shell/src/frame-src-cases.json"
        ))
        .unwrap();
        for c in cases.as_array().unwrap() {
            let got = page_src(
                c["id"].as_str().unwrap(),
                c["n"].as_u64().unwrap(),
                c["origin"].as_str(),
                c["key"].as_str(),
                c["file"].as_str().unwrap(),
            );
            assert_eq!(got, c["src"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn the_frame_has_the_shells_attributes() {
        let host = include_str!("../../../../web/shell/src/view/frame-host.ts");
        assert!(
            host.contains(&format!(
                "export const FRAME_SANDBOX = \"{FRAME_SANDBOX}\";"
            )),
            "FRAME_SANDBOX differs from frame-host.ts"
        );
        for attr in [
            "const FRAME_ALLOW = \"clipboard-write; fullscreen\";",
            "const FRAME_TITLE = \"artifact content\";",
            "el.className = \"frame\";",
            "el.title = FRAME_TITLE;",
            "el.setAttribute(\"allow\", FRAME_ALLOW);",
            "if (sandboxed) el.setAttribute(\"sandbox\", FRAME_SANDBOX);",
        ] {
            assert!(host.contains(attr), "frame-host.ts no longer sets: {attr}");
        }
        assert_eq!(
            frame_html("/c/x/v/1/\"><b>", true),
            format!(
                "<iframe class=\"frame\" title=\"artifact content\" src=\"/c/x/v/1/&quot;&gt;&lt;b&gt;\" allow=\"clipboard-write; fullscreen\" sandbox=\"{FRAME_SANDBOX}\"></iframe>"
            )
        );
        assert!(!frame_html("http://x.localhost:1/v/1/", false).contains("sandbox"));
    }

    #[test]
    fn script_json_cannot_close_its_element_or_break_a_line() {
        let v = json!({"t": "</script><!--<script>\u{2028}\u{2029}&>"});
        let s = script_json(&v);
        for bad in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
            assert!(!s.contains(bad), "{s}");
        }
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap(), v);
    }

    #[test]
    fn html_escape_covers_text_and_attributes() {
        assert_eq!(
            html_escape(r#"<a href="x" title='y'>&</a>"#),
            "&lt;a href=&quot;x&quot; title=&#39;y&#39;&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn inject_without_data_only_removes_the_markers() {
        let t = "<head><!--clax:boot--></head><div class=\"stage\"><!--clax:frame--></div><h1>Clax</h1>";
        assert_eq!(
            inject(t, None),
            "<head></head><div class=\"stage\"></div><h1>Clax</h1>"
        );
        let i = Injected {
            boot: "{\"v\":1}".into(),
            frame: Some("<iframe></iframe>".into()),
            title: "A <b>".into(),
        };
        assert_eq!(
            inject(t, Some(&i)),
            "<head><script type=\"application/json\" id=\"clax-boot\">{\"v\":1}</script></head><div class=\"stage\"><iframe></iframe></div><h1>A &lt;b&gt;</h1>"
        );
    }

    #[test]
    fn a_title_cannot_forge_a_marker() {
        let t = "<head><!--clax:boot--></head><div class=\"stage\"><!--clax:frame--></div><h1>Clax</h1>";
        let i = Injected {
            boot: "{}".into(),
            frame: None,
            title: "<!--clax:frame--><!--clax:boot-->".into(),
        };
        let out = inject(t, Some(&i));
        assert_eq!(out.matches("<script").count(), 1);
        assert!(out.contains("<h1>&lt;!--clax:frame--&gt;&lt;!--clax:boot--&gt;</h1>"));
    }

    fn headers(host: &str, cookie: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(c) = cookie {
            h.insert(header::COOKIE, c.parse().unwrap());
        }
        h
    }

    #[test]
    fn the_frame_mode_comes_from_the_host_and_the_cookie() {
        let id = "7q3k9mzx2b4t";
        assert_eq!(frame_origin(&headers("localhost:7", None), id), None);
        assert_eq!(
            frame_origin(&headers("localhost:7", Some("clax_frame=bogus")), id),
            None
        );
        assert_eq!(
            frame_origin(&headers("localhost:7", Some("a=1; clax_frame=sandbox")), id),
            Some(None)
        );
        assert_eq!(
            frame_origin(&headers("127.0.0.1:7", Some("clax_frame=subdomain")), id),
            Some(Some(format!("http://{id}.localhost:7")))
        );
        assert_eq!(
            frame_origin(&headers("localhost", Some("clax_frame=subdomain")), id),
            Some(Some(format!("http://{id}.localhost")))
        );
        // A LAN or other host is always sandboxed, whatever the cookie says.
        for host in ["192.168.1.5:7", "[::1]:7", "example.com", "localhost.:7"] {
            assert_eq!(
                frame_origin(&headers(host, Some("clax_frame=subdomain")), id),
                Some(None),
                "{host}"
            );
        }
    }
}
