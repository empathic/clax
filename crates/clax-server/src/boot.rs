//! The first-load data the daemon puts into `artifact.html` for `/a/…`
//! (spec §8 Time to usable): the bootstrap block, the page title and, when
//! the frame mode is known, the content frame. String injection at comment
//! markers the shell build keeps; no template engine.

use crate::error::ApiError;
use crate::feedback::thread_view;
use crate::routes::artifacts::with_owner;
use crate::shell_route::ShellRoute;
use crate::state::AppState;
use axum::http::{HeaderMap, header};
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

/// Where the frame shows `file` of version `n`, as `pageSrc` in `web/shell/src/origin.ts`.
pub fn page_src(id: &str, n: u64, origin: Option<&str>, file: &str) -> String {
    let root = match origin {
        Some(o) => format!("{o}/v/{n}/"),
        None => format!("/c/{id}/v/{n}/"),
    };
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
    crate::routes::artifacts::strip_sessions(&mut v);
    v
}

/// Everything `artifact.html` gets for `route`, or `None` when it names no
/// artifact the store has. The data is what an unauthenticated browser reads
/// from `GET /api/artifacts/<id>` (less its session IDs),
/// `GET /api/artifacts/<id>/threads?include_resolved=true` without the token
/// (no `clip_path`), and `GET /api/viewers/me` for the cookie's existing
/// viewer: never the token or the viewer cookie, and no viewer is created.
pub async fn assemble(
    s: &AppState,
    route: ShellRoute,
    headers: &HeaderMap,
) -> Result<Option<Injected>, ApiError> {
    let ShellRoute::Artifact { id, version, file } = route else {
        return Ok(None);
    };
    let viewer_id = crate::viewer::read(headers);
    let codex = s.feedback_ctx().codex_push();
    let lookup = id.clone();
    let working = s.working.for_artifact(id.as_str());
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
            let mut threads = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let (page, next) = st.list_threads(&lookup, true, cursor.as_deref(), 200)?;
                for t in &page {
                    threads.push(thread_view(st, t, codex, false)?);
                }
                match next {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            let viewer = match &viewer_id {
                Some(v) => st.get_viewer(v)?,
                None => None,
            };
            let attention = match &viewer {
                Some(v) => Some(st.attention(&v.id, &lookup)?),
                None => None,
            };
            let artifact = with_owner(st, &a, owner.as_ref(), &working)?;
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
    let (frame, frame_json) = match frame_origin(headers, id.as_str()) {
        Some(origin) if holds => {
            let src = page_src(id.as_str(), n, origin.as_deref(), &file);
            (
                Some(frame_html(&src, origin.is_none())),
                json!({"mode": if origin.is_some() { "subdomain" } else { "sandbox" }, "src": src}),
            )
        }
        _ => (None, Value::Null),
    };
    let artifact = without_sessions(
        artifact_json,
        serde_json::to_value(&versions).expect("versions serialise"),
    );
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
        let cases: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/shell/src/frame-src-cases.json"
        )))
        .unwrap();
        for c in cases.as_array().unwrap() {
            let got = page_src(
                c["id"].as_str().unwrap(),
                c["n"].as_u64().unwrap(),
                c["origin"].as_str(),
                c["file"].as_str().unwrap(),
            );
            assert_eq!(got, c["src"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn the_frame_has_the_shells_attributes() {
        let host = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/shell/src/view/frame-host.ts"
        ));
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
