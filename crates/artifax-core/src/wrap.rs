//! Serve-time wrapping of a published HTML page (the index or any supporting
//! `text/html` file) into the document skeleton, and the recognition rule that
//! keeps a full document from being wrapped twice. Every served page carries
//! exactly one bridge tag, first in `<head>` where there is one, so
//! `window.claude` exists before any page script runs.

pub const RESET_CSS: &str = "*,*::before,*::after{box-sizing:border-box}html,body{margin:0;padding:0;min-height:100%}img,video,svg{max-width:100%;display:block}";

/// Where the daemon serves the bridge script. A tag names it with `?v=<bridge
/// version>` (a short hash of the bundle), so a changed bridge has a new URL.
pub const BRIDGE_PATH: &str = "/_artifax/bridge.js";

/// The bridge tag for the version's index page.
pub fn bridge_tag(artifact_id: &str, version: u32, contract: &str, bridge: &str) -> String {
    bridge_tag_for(
        artifact_id,
        version,
        contract,
        crate::publish::INDEX,
        bridge,
    )
}

/// The bridge tag for the page published at `file`; `data-file` carries the
/// path, attribute-escaped. `bridge` is the bridge version its URL carries
/// (`/_artifax/bridge.js?v=<bridge>`); empty names the bare URL.
pub fn bridge_tag_for(
    artifact_id: &str,
    version: u32,
    contract: &str,
    file: &str,
    bridge: &str,
) -> String {
    let attr = |s: &str| {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let file = attr(file);
    let src = if bridge.is_empty() {
        BRIDGE_PATH.to_string()
    } else {
        format!("{BRIDGE_PATH}?v={}", attr(bridge))
    };
    format!(
        "<script src=\"{src}\" data-artifact=\"{artifact_id}\" data-version=\"{version}\" data-contract=\"{contract}\" data-file=\"{file}\"></script>"
    )
}

/// Every bridge tag begins with this.
const BRIDGE_START: &str = "<script src=\"/_artifax/bridge.js";
/// What follows the URL in every bridge tag the daemon has written.
const BRIDGE_ATTRS: &str = "\" data-artifact=\"";

/// Whether `at` begins with a bridge tag exactly as the daemon writes it: the
/// bare URL (as tags carried before the URL named a version) or the URL with
/// `?v=<lowercase hex>`, then ` data-artifact="`. Anything else that merely
/// contains the URL (a page's own string or comment) is not a bridge tag.
fn is_bridge_tag(at: &str) -> bool {
    let Some(rest) = at.strip_prefix(BRIDGE_START) else {
        return false;
    };
    let rest = match rest.strip_prefix("?v=") {
        Some(v) => v.trim_start_matches(|c: char| matches!(c, '0'..='9' | 'a'..='f')),
        None => rest,
    };
    rest.starts_with(BRIDGE_ATTRS)
}

/// `page` without any bridge tag an earlier serve inserted (from a tag
/// [`is_bridge_tag`] recognises to the next `</script>`, or to the end when
/// unterminated), so a page
/// republished from its served DOM runs exactly one bridge: the one for the
/// version and file being served, at the current bridge URL.
fn strip_bridge_tags(page: &str) -> std::borrow::Cow<'_, str> {
    if !page
        .match_indices(BRIDGE_START)
        .any(|(i, _)| is_bridge_tag(&page[i..]))
    {
        return std::borrow::Cow::Borrowed(page);
    }
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    while let Some(i) = rest.find(BRIDGE_START) {
        if !is_bridge_tag(&rest[i..]) {
            let past = i + BRIDGE_START.len();
            out.push_str(&rest[..past]);
            rest = &rest[past..];
            continue;
        }
        out.push_str(&rest[..i]);
        rest = match rest[i..].find("</script>") {
            Some(j) => &rest[i + j + "</script>".len()..],
            None => "",
        };
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// Byte offset of a leading `<!doctype` declaration (any case), after whitespace
/// and an optional UTF-8 BOM, or None when the page does not begin with one.
fn doctype_start(page: &str) -> Option<usize> {
    let start = page.len()
        - page
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
            .len();
    let rest = &page.as_bytes()[start..];
    (rest.len() >= 9 && rest[..9].eq_ignore_ascii_case(b"<!doctype")).then_some(start)
}

/// A page is a full document when, after leading whitespace and an optional
/// UTF-8 BOM, it begins with a `<!doctype` declaration of any kind
/// (`<!doctype html>`, `<!DOCTYPE html PUBLIC "...">`, `<!doctype html >`).
pub fn is_full_document(page: &str) -> bool {
    doctype_start(page).is_some()
}

/// Byte offset just past the first real open tag named `name` (lowercase
/// ASCII, matched case-insensitively), skipping HTML comments and raw-text
/// elements (`<script>` and `<style>`). The name must be followed by `>`,
/// space, tab, CR, or LF, so `<header>` is not `<head>` and `<bodyx>` is not
/// `<body>`.
///
/// A comment runs from `<!--` to the next `-->`; a `<script>` or `<style>`
/// element runs to its literal `</script>` or `</style>` end tag, and comment
/// markers inside it have no effect. An unterminated comment or raw-text element
/// extends to the end of the page, so no tag after its start is found.
///
/// Limitations: a `>` inside a quoted attribute value truncates tag detection,
/// `<title>`/`<textarea>` contents are not skipped, and end tags with
/// whitespace before the `>` (`</script >`) are not recognized.
fn open_tag_end(doc: &str, name: &[u8]) -> Option<usize> {
    let bytes = doc.as_bytes();
    let mut i = 0;

    // Whether `bytes[i..]` begins with `<` + `tag` followed by a tag-name end.
    let opens = |i: usize, tag: &[u8]| {
        let end = i + 1 + tag.len();
        end < bytes.len()
            && bytes[i] == b'<'
            && bytes[i + 1..end].eq_ignore_ascii_case(tag)
            && matches!(bytes[end], b'>' | b' ' | b'\t' | b'\n' | b'\r')
    };

    while i < bytes.len() {
        if bytes[i..].starts_with(b"<!--") {
            i = find_from(bytes, i + 4, b"-->", false).map_or(bytes.len(), |p| p + 3);
            continue;
        }
        if opens(i, b"script") {
            i = find_from(bytes, i + 7, b"</script>", true).map_or(bytes.len(), |p| p + 9);
            continue;
        }
        if opens(i, b"style") {
            i = find_from(bytes, i + 6, b"</style>", true).map_or(bytes.len(), |p| p + 8);
            continue;
        }
        if opens(i, name) {
            let after = i + 1 + name.len();
            return bytes[after..]
                .iter()
                .position(|&b| b == b'>')
                .map(|pos| after + pos + 1);
        }
        i += 1;
    }
    None
}

/// Offset of the first occurrence of `needle` in `bytes` at or after `from`,
/// optionally ignoring ASCII case.
fn find_from(bytes: &[u8], from: usize, needle: &[u8], ignore_case: bool) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| {
            if ignore_case {
                w.eq_ignore_ascii_case(needle)
            } else {
                w == needle
            }
        })
        .map(|p| from + p)
}

/// [`wrap_page`] for the version's index page.
pub fn wrap_document(
    page: &str,
    artifact_id: &str,
    version: u32,
    contract: &str,
    bridge: &str,
) -> String {
    wrap_page(
        page,
        artifact_id,
        version,
        contract,
        crate::publish::INDEX,
        bridge,
    )
}

/// Returns the page published at `file` as served, with exactly one bridge
/// tag (see [`bridge_tag_for`]), placed so it runs before any page script;
/// bridge tags already in the page are removed first. A full document
/// ([`is_full_document`]) is served unchanged except for the bridge script,
/// inserted just after the first real `<head ...>` tag (see `open_tag_end`)
/// when it comes before any `<body ...>` tag; without one, just after the first real `<body ...>` tag; without either,
/// just after the doctype declaration. A fragment is placed in the document
/// skeleton with the bridge script first in `<head>`.
pub fn wrap_page(
    page: &str,
    artifact_id: &str,
    version: u32,
    contract: &str,
    file: &str,
    bridge: &str,
) -> String {
    let page = strip_bridge_tags(page);
    let page = page.as_ref();
    let tag = bridge_tag_for(artifact_id, version, contract, file, bridge);

    if is_full_document(page) {
        let body = open_tag_end(page, b"body");
        // A stray `<head>` after the body tag is not the document's head.
        let head = open_tag_end(page, b"head").filter(|&h| body.is_none_or(|b| h < b));
        let at = head.or(body).unwrap_or_else(|| {
            let start = doctype_start(page).expect("full document has a doctype");
            match page.as_bytes()[start..].iter().position(|&b| b == b'>') {
                Some(off) => start + off + 1,
                None => page.len(),
            }
        });
        return format!("{}{}{}", &page[..at], tag, &page[at..]);
    }
    format!(
        "<!doctype html><html><head>{tag}<meta charset=utf8><meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><style>{RESET_CSS}</style></head><body>{page}</body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bridge version the tests serve.
    const V: &str = "0123456789ab";

    #[test]
    // The bridge moved from first in `<body>` to first in `<head>`, so it runs
    // before any `<head>` script.
    fn fragment_is_wrapped_with_skeleton_and_bridge_first_in_head() {
        let out = wrap_document(
            "<title>T</title><style>p{}</style><p>hi</p>",
            "7q3k9mzx2b4t",
            2,
            "0.2.61",
            V,
        );
        assert!(out.starts_with("<!doctype html><html><head><script src=\"/_artifax/bridge.js"));
        let tag = out.find("<script src=\"/_artifax/bridge.js").unwrap();
        assert!(tag < out.find("<meta charset=utf8>").unwrap());
        assert!(tag < out.find("<body>").unwrap() && tag < out.find("<title>").unwrap());
        assert!(out.contains(
            "data-artifact=\"7q3k9mzx2b4t\" data-version=\"2\" data-contract=\"0.2.61\""
        ));
        assert!(out.ends_with("</body></html>"));
    }

    #[test]
    // The page has a `<head>`, so the bridge now goes right after it.
    fn full_document_is_recognised_case_insensitively_and_bridge_goes_after_head_tag() {
        let page = "\n  <!DOCTYPE HTML><html><HEAD class=\"h\"><title>x</title></HEAD><body class=\"x\" data-a=\"1\"><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with("\n  <!DOCTYPE HTML>"), "served as-is");
        let head_end = out.find("<HEAD class=\"h\">").unwrap() + "<HEAD class=\"h\">".len();
        assert!(out[head_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)));
        assert_eq!(
            out.matches("<!DOCTYPE").count() + out.matches("<!doctype").count(),
            1,
            "not double wrapped"
        );
    }

    #[test]
    fn full_document_without_body_tag_gets_bridge_after_doctype() {
        let out = wrap_document(
            "<!doctype html><p>no body tag</p>",
            "7q3k9mzx2b4t",
            1,
            "0.2.61",
            V,
        );
        assert!(out.starts_with(&format!(
            "<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    // The page has a `<head>`, so the bridge now goes right after it.
    fn xhtml_doctype_with_head_gets_bridge_after_head_and_is_not_double_wrapped() {
        let page = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\"><html><head><title>x</title></head><body><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        let head_end = out.find("<head>").unwrap() + "<head>".len();
        assert!(out[head_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)));
        assert!(out.starts_with("<!DOCTYPE html PUBLIC"));
        assert_eq!(out.to_lowercase().matches("<!doctype").count(), 1);
    }

    #[test]
    fn doctype_with_space_and_no_body_gets_bridge_after_its_closing_bracket() {
        let out = wrap_document("<!doctype html ><p>x</p>", "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with(&format!(
            "<!doctype html >{}<p>x</p>",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    fn bom_prefixed_doctype_is_recognised_and_bom_preserved() {
        let page = "\u{FEFF}<!doctype html><p>x</p>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with(&format!(
            "\u{FEFF}<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    // No `<head>` (the comment sits directly under `<html>`), so this still
    // tests body detection.
    fn body_inside_a_comment_is_not_the_body_tag() {
        let page = "<!doctype html><html><!-- <body> --><body><p></p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        let real = out
            .find("<body><script")
            .expect("bridge after real body tag");
        assert!(real > out.find("-->").unwrap());
    }

    #[test]
    fn non_ascii_before_body_tag_does_not_panic() {
        // No `<head>`, so this still tests body detection.
        let page = "<!doctype html><html><title>é — ü</title><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        let body_end = out.find("<body>").unwrap() + "<body>".len();
        assert!(out[body_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)));
    }

    #[test]
    fn non_ascii_before_missing_body_tag_does_not_panic() {
        let page = "<!doctype html><title>é — ü</title><p>x</p>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with(&format!(
            "<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    // No `<head>` (the script sits directly under `<html>`), so this still
    // tests body detection.
    fn body_inside_script_is_not_the_body_tag() {
        let page =
            "<!doctype html><html><script>var a=\"<body>\";</script><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        // The real <body> tag should have the bridge after it
        let body_end =
            out.find("<body><script src=\"/_artifax/bridge.js").unwrap() + "<body>".len();
        assert!(
            out[body_end..].starts_with("<script src=\"/_artifax/bridge.js"),
            "bridge should be directly after real body tag"
        );
        assert!(out.contains("<script>var a=\"<body>\";"));
    }

    #[test]
    fn unterminated_comment_or_raw_text_hides_the_rest_of_the_page() {
        let tag = bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V);
        for page in [
            "<!doctype html><!-- x<body>",
            "<!doctype html><script>x<body>",
            "<!doctype html><style>x<body>",
            "<!doctype html><script>let s = 1;</scrip<body>",
        ] {
            let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
            assert_eq!(
                out,
                format!("<!doctype html>{tag}{}", &page["<!doctype html>".len()..]),
                "{page}"
            );
        }
    }

    #[test]
    fn the_bridge_tag_names_its_file_escaped() {
        assert!(
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
                .ends_with("data-contract=\"0.2.61\" data-file=\"index.html\"></script>")
        );
        let out = wrap_page("<p>x</p>", "7q3k9mzx2b4t", 1, "0.2.61", "a\"&<b>.html", V);
        assert!(
            out.contains("data-file=\"a&quot;&amp;&lt;b&gt;.html\"></script>"),
            "{out}"
        );
        assert_eq!(
            wrap_document("<p>x</p>", "7q3k9mzx2b4t", 1, "0.2.61", V),
            wrap_page("<p>x</p>", "7q3k9mzx2b4t", 1, "0.2.61", "index.html", V)
        );
    }

    #[test]
    fn republished_outer_html_keeps_one_bridge() {
        let served_v1 = wrap_page(
            "<!doctype html><html><head><title>P</title></head><body><p>x</p></body></html>",
            "7q3k9mzx2b4t",
            1,
            "0.2.61",
            "about.html",
            V,
        );
        // A page that republishes its served DOM sends the version 1 bridge tag back.
        let served_v2 = wrap_page(&served_v1, "7q3k9mzx2b4t", 2, "0.2.61", "about.html", V);
        assert_eq!(
            served_v2.matches("/_artifax/bridge.js").count(),
            1,
            "{served_v2}"
        );
        assert!(
            served_v2.contains("data-version=\"2\"") && !served_v2.contains("data-version=\"1\"")
        );
        assert_eq!(served_v2.matches("<!doctype").count(), 1);
        let fragment = format!(
            "<p>a</p>{}<p>b</p>",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        );
        let out = wrap_document(&fragment, "7q3k9mzx2b4t", 2, "0.2.61", V);
        assert_eq!(out.matches("/_artifax/bridge.js").count(), 1);
        assert!(out.contains("<p>a</p><p>b</p>"));
    }

    #[test]
    fn an_unterminated_bridge_tag_is_dropped_with_the_rest_of_the_page() {
        let out = wrap_document(
            "<!doctype html><body><p>keep</p><script src=\"/_artifax/bridge.js\" data-artifact=\"7q3k9mzx2b4t\" data-version=\"1\">",
            "7q3k9mzx2b4t",
            2,
            "0.2.61",
            V,
        );
        assert_eq!(out.matches("/_artifax/bridge.js").count(), 1);
        assert!(out.contains("<p>keep</p>"));
    }

    #[test]
    fn bodyx_is_not_body_and_body_with_newline_is() {
        let page1 = "<!doctype html><bodyx><p>x</p>";
        let out1 = wrap_document(page1, "id1", 1, "0.2.61", V);
        assert!(out1.contains("<bodyx><p>x</p>"));

        let page2 = "<!doctype html><body\n class=\"a\"><p>x</p></body></html>";
        let out2 = wrap_document(page2, "id2", 1, "0.2.61", V);
        let body_end = out2.find("<body\n class=\"a\">").unwrap() + "<body\n class=\"a\">".len();
        assert!(out2[body_end..].starts_with(&bridge_tag("id2", 1, "0.2.61", V)));
    }
    #[test]
    fn the_bridge_goes_right_after_the_head_tag_before_any_head_script() {
        let page = "<!doctype html><html><head lang=\"en\"><script>window.early = typeof window.claude;</script><title>x</title></head><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        let head_end = out.find("<head lang=\"en\">").unwrap() + "<head lang=\"en\">".len();
        assert!(out[head_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)));
        assert!(out.find("/_artifax/bridge.js").unwrap() < out.find("window.early").unwrap());
        let sub = wrap_page(page, "7q3k9mzx2b4t", 1, "0.2.61", "about.html", V);
        assert!(sub[head_end..].starts_with(&bridge_tag_for(
            "7q3k9mzx2b4t",
            1,
            "0.2.61",
            "about.html",
            V
        )));
    }

    #[test]
    fn head_in_comments_scripts_or_header_is_not_the_head_tag() {
        let tag = bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V);
        let page = "<!doctype html><html><!-- <head> --><script>var h=\"<head>\";</script><header>x</header><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(
            out.contains(&format!("<body>{tag}<p>x</p>")),
            "no real head: after the body tag"
        );
        assert_eq!(out.matches("/_artifax/bridge.js").count(), 1);
    }

    #[test]
    fn a_head_tag_after_the_body_tag_is_not_the_head() {
        let tag = bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V);
        let page = "<!doctype html><body><script>a()</script><head></head><p>x</p></body>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(
            out.starts_with(&format!("<!doctype html><body>{tag}<script>a()")),
            "{out}"
        );
    }

    #[test]
    fn a_fragment_carries_the_bridge_first_in_its_head() {
        let out = wrap_document("<p>hi</p>", "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with(&format!(
            "<!doctype html><html><head>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    fn the_bridge_url_carries_the_bridge_version() {
        assert!(
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", "0123456789ab").starts_with(
                "<script src=\"/_artifax/bridge.js?v=0123456789ab\" data-artifact=\"7q3k9mzx2b4t\""
            )
        );
        assert!(
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", "")
                .starts_with("<script src=\"/_artifax/bridge.js\" data-artifact="),
            "no version known: the bare URL"
        );
    }

    #[test]
    fn republished_bare_and_versioned_bridge_tags_leave_one_current_tag() {
        let bare = "<script src=\"/_artifax/bridge.js\" data-artifact=\"7q3k9mzx2b4t\" data-version=\"1\" data-contract=\"0.2.61\" data-file=\"index.html\"></script>";
        let old = bridge_tag("7q3k9mzx2b4t", 2, "0.2.61", "ffffffffffff");
        for page in [
            format!("<!doctype html><html><body>{bare}<p>x</p></body></html>"),
            format!("<!doctype html><html><body>{old}<p>x</p></body></html>"),
            format!("<!doctype html><html><body>{old}<p>x</p>{bare}</body></html>"),
        ] {
            let out = wrap_document(&page, "7q3k9mzx2b4t", 3, "0.2.61", V);
            assert_eq!(out.matches("/_artifax/bridge.js").count(), 1, "{out}");
            assert!(
                out.contains(&bridge_tag("7q3k9mzx2b4t", 3, "0.2.61", V)),
                "{out}"
            );
            assert!(out.contains("<p>x</p></body>"), "{out}");
        }
        let other = "<script src=\"/_artifax/bridge.jsx\"></script>";
        let own = "<script>const s = '<script src=\"/_artifax/bridge.js';</script><!-- <script src=\"/_artifax/bridge.js?v=1\"> --><script>const t = '<script src=\"/_artifax/bridge.js?v=x\" data-artifact=';</script>";
        let out = wrap_document(&format!("<p>{own}</p>"), "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.contains(own), "the page's own text is kept: {out}");
        assert_eq!(
            out.matches(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V))
                .count(),
            1
        );
        let out = wrap_document(&format!("<p>{other}</p>"), "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.contains(other), "another script is kept: {out}");
    }
}
