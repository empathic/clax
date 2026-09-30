//! Serve-time wrapping of a published HTML page (the index or any supporting
//! `text/html` file) into the document skeleton, and the recognition rule that
//! keeps a full document from being wrapped twice. Every served page carries
//! exactly one bridge tag, right after the doctype of a full document or first
//! in the skeleton's `<head>`, so `window.claude` exists before any page
//! script runs.

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

/// Where a full document's ([`is_full_document`]) bridge tag goes: just past
/// its leading doctype declaration and any ASCII whitespace after it (never
/// before the doctype, which would put the page in quirks mode). Whitespace
/// left before the tag is dropped by the parser, so a page republished from
/// its served DOM does not gain a text node in `<head>` on every round trip.
///
/// A script right after the doctype runs before anything else in the page:
/// the parser creates `<html>` and `<head>` around it, a later `<html ...>`
/// tag's attributes are merged onto the root, and a later `<head ...>` tag is
/// ignored, attributes included.
fn bridge_insertion_point(page: &str) -> usize {
    let start = doctype_start(page).expect("a full document has a doctype");
    let end = page[start..]
        .find('>')
        .map_or(page.len(), |p| start + p + 1);
    let rest = &page[end..];
    end + rest.len()
        - rest
            .trim_start_matches(['\t', '\n', '\x0C', '\r', ' '])
            .len()
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
/// inserted just after its doctype declaration (see `bridge_insertion_point`).
/// A fragment is placed in the document skeleton with the bridge script
/// first in `<head>`.
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
        let at = bridge_insertion_point(page);
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
    fn full_document_without_head_or_body_tags_gets_bridge_after_doctype() {
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
    fn non_ascii_in_a_tagless_document_does_not_panic() {
        let page = "<!doctype html><title>é — ü</title><p>x</p>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with(&format!(
            "<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    fn unterminated_comment_or_raw_text_leaves_the_bridge_after_the_doctype() {
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

    // Placement no longer looks for `<head>` or `<body>`: a full document gets
    // the bridge right after its doctype, whatever follows.
    #[test]
    fn full_document_is_recognised_case_insensitively_and_bridge_goes_after_the_doctype() {
        let page = "\n  <!DOCTYPE HTML><html><HEAD class=\"h\"><title>x</title></HEAD><body class=\"x\" data-a=\"1\"><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert_eq!(
            out,
            format!(
                "\n  <!DOCTYPE HTML>{}{}",
                bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V),
                &page["\n  <!DOCTYPE HTML>".len()..]
            ),
            "served as-is apart from the tag"
        );
    }

    #[test]
    fn xhtml_doctype_gets_bridge_after_its_closing_bracket_and_is_not_double_wrapped() {
        let page = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\"><html><head><title>x</title></head><body><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        let doctype_end = page.find('>').unwrap() + 1;
        assert!(out[doctype_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)));
        assert_eq!(out.to_lowercase().matches("<!doctype").count(), 1);
    }

    #[test]
    fn the_bridge_runs_before_every_page_script() {
        let tag = bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V);
        for page in [
            // A <head> script.
            "<!doctype html><html><head lang=\"en\"><script>window.early = 1;</script><title>x</title></head><body><p>x</p></body></html>",
            // No <head> tag: the head is implied, with a script before <body>.
            "<!doctype html><html lang=en><meta charset=utf-8><title>T</title><script>window.early = 1;</script><body><p>x</p></body></html>",
            // A script before the <head> tag.
            "<!doctype html><html><script>window.early = 1;</script><head><title>x</title></head><body></body></html>",
            // No <html>, <head> or <body> tags at all.
            "<!doctype html><script>window.early = 1;</script><p>x</p>",
        ] {
            let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
            assert!(out.starts_with(&format!("<!doctype html>{tag}")), "{out}");
            assert_eq!(out.matches("/_artifax/bridge.js").count(), 1, "{out}");
            assert!(out.find("/_artifax/bridge.js").unwrap() < out.find("window.early").unwrap());
        }
        let sub = wrap_page(
            "<!doctype html><head><script>1</script></head>",
            "7q3k9mzx2b4t",
            1,
            "0.2.61",
            "about.html",
            V,
        );
        assert!(sub.starts_with(&format!(
            "<!doctype html>{}<head>",
            bridge_tag_for("7q3k9mzx2b4t", 1, "0.2.61", "about.html", V)
        )));
    }

    #[test]
    fn head_or_body_in_text_does_not_move_the_bridge() {
        let tag = bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V);
        for page in [
            "<!doctype html><title>The <head> element</title><body><p>x</p></body>",
            "<!doctype html><textarea><head></textarea><p>x</p>",
            "<!doctype html><meta content=\"<head>\"><p>x</p>",
            "<!doctype html><!-- <head> --><script>var h=\"<body>\";</script><header>x</header>",
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
    fn non_ascii_after_the_doctype_does_not_panic() {
        let page =
            "<!doctype html><html><head><title>é — ü</title></head><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61", V);
        assert!(out.starts_with(&format!(
            "<!doctype html>{}<html>",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", V)
        )));
    }

    #[test]
    fn the_insertion_point_is_after_the_doctype_and_its_trailing_whitespace() {
        assert_eq!(bridge_insertion_point("<!doctype html><p>"), 15);
        assert_eq!(
            bridge_insertion_point("\u{FEFF} \n<!DOCTYPE html >x"),
            "\u{FEFF} \n<!DOCTYPE html >".len()
        );
        assert_eq!(
            bridge_insertion_point("<!doctype html>\n \t\r\x0C<html>"),
            "<!doctype html>\n \t\r\x0C".len()
        );
        // Only ASCII whitespace: a no-break space is text.
        assert_eq!(bridge_insertion_point("<!doctype html>\u{A0}<p>"), 15);
        assert_eq!(
            bridge_insertion_point("<!doctype html"),
            "<!doctype html".len()
        );
        assert_eq!(
            bridge_insertion_point("<!doctype html>\n"),
            "<!doctype html>\n".len()
        );
    }

    #[test]
    fn repeated_republishes_of_the_served_dom_leave_the_head_unchanged() {
        // What the browser builds from a served page and `"<!doctype html>\n" +
        // outerHTML` sends back: whitespace before the bridge (before <html>)
        // is dropped, the bridge sits first in <head>. Serving the result again
        // must keep the whitespace out of <head>.
        let browser_round_trip = |served: &str| {
            let at = served.find("<script src=\"/_artifax/bridge.js").unwrap();
            let end = at + served[at..].find("</script>").unwrap() + "</script>".len();
            let (tag, after) = (&served[at..end], &served[end..]);
            assert!(served[..at].trim_end() == "<!doctype html>", "{served}");
            let rest = after.strip_prefix("<html lang=\"en\"><head>").expect(after);
            format!("<!doctype html>\n<html lang=\"en\"><head>{tag}{rest}")
        };
        let mut page = "<!doctype html>\n<html lang=\"en\"><head><title>P</title></head><body><p>x</p></body></html>".to_string();
        for v in 1..=4 {
            let served = wrap_document(&page, "7q3k9mzx2b4t", v, "0.2.61", V);
            assert!(served.starts_with("<!doctype html>\n<script"), "{served}");
            page = browser_round_trip(&served);
            assert!(
                page.contains("<head><script src=\"/_artifax/bridge.js")
                    && page.contains("</script><title>P</title></head>"),
                "round trip {v}: {page}"
            );
        }
    }

    #[test]
    fn a_browser_serialized_served_page_republishes_with_one_bridge() {
        // What `"<!doctype html>\n" + document.documentElement.outerHTML` gives for a
        // served page: the parser moved the bridge into the implied `<head>`.
        let served = format!(
            "<!doctype html>\n<html lang=\"en\"><head>{}<title>P</title><script>early()</script></head><body><p>x</p></body></html>",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61", "ffffffffffff")
        );
        let out = wrap_document(&served, "7q3k9mzx2b4t", 2, "0.2.61", V);
        assert_eq!(
            out,
            format!(
                "<!doctype html>\n{}<html lang=\"en\"><head><title>P</title><script>early()</script></head><body><p>x</p></body></html>",
                bridge_tag("7q3k9mzx2b4t", 2, "0.2.61", V)
            )
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
