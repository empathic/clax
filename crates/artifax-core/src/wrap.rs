//! Serve-time wrapping of a published page into the document skeleton, and the
//! recognition rule that keeps a full document from being wrapped twice.

pub const RESET_CSS: &str = "*,*::before,*::after{box-sizing:border-box}html,body{margin:0;padding:0;min-height:100%}img,video,svg{max-width:100%;display:block}";

pub fn bridge_tag(artifact_id: &str, version: u32, contract: &str) -> String {
    format!(
        "<script src=\"/_artifax/bridge.js\" data-artifact=\"{artifact_id}\" data-version=\"{version}\" data-contract=\"{contract}\"></script>"
    )
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

/// Byte offset just past the first real `<body ...>` tag, skipping HTML comments
/// and raw-text elements (`<script>` and `<style>`).
///
/// A comment runs from `<!--` to the next `-->`; a `<script>` or `<style>`
/// element runs to its literal `</script>` or `</style>` end tag, and comment
/// markers inside it have no effect. An unterminated comment or raw-text element
/// extends to the end of the page, so no `<body>` after its start is found.
///
/// Limitations: a `>` inside a quoted attribute value truncates tag detection,
/// `<title>`/`<textarea>` contents are not skipped, and end tags with
/// whitespace before the `>` (`</script >`) are not recognized.
fn body_tag_end(doc: &str) -> Option<usize> {
    let bytes = doc.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Skip HTML comments: <!--
        if i + 4 <= bytes.len() && &bytes[i..i + 4] == b"<!--" {
            i += 4;
            i = find_from(bytes, i, b"-->", false).map_or(bytes.len(), |p| p + 3);
            continue;
        }

        // Skip <script> elements
        if i + 7 <= bytes.len()
            && bytes[i..i + 7].eq_ignore_ascii_case(b"<script")
            && i + 7 < bytes.len()
        {
            match bytes[i + 7] {
                b'>' | b' ' | b'\t' | b'\n' | b'\r' => {
                    i = find_from(bytes, i + 7, b"</script>", true).map_or(bytes.len(), |p| p + 9);
                    continue;
                }
                _ => {}
            }
        }

        // Skip <style> elements
        if i + 6 <= bytes.len()
            && bytes[i..i + 6].eq_ignore_ascii_case(b"<style")
            && i + 6 < bytes.len()
        {
            match bytes[i + 6] {
                b'>' | b' ' | b'\t' | b'\n' | b'\r' => {
                    i = find_from(bytes, i + 6, b"</style>", true).map_or(bytes.len(), |p| p + 8);
                    continue;
                }
                _ => {}
            }
        }

        // Check for <body tag
        if i + 5 <= bytes.len() && bytes[i..i + 5].eq_ignore_ascii_case(b"<body") {
            let after = i + 5;
            if after < bytes.len() {
                match bytes[after] {
                    b'>' | b' ' | b'\t' | b'\n' | b'\r' => {
                        // Find the closing >
                        if let Some(pos) = bytes[after..].iter().position(|&b| b == b'>') {
                            return Some(after + pos + 1);
                        }
                        return None;
                    }
                    _ => {}
                }
            }
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

/// Returns the page as served. A fragment (not [`is_full_document`]) is placed in
/// the document skeleton with the bridge script first in `<body>`. A full
/// document is served unchanged except for the bridge script, inserted just after
/// the first real `<body ...>` tag (see `body_tag_end`); as a fallback, when
/// there is no such tag, it is inserted just after the doctype declaration.
pub fn wrap_document(page: &str, artifact_id: &str, version: u32, contract: &str) -> String {
    let tag = bridge_tag(artifact_id, version, contract);
    if is_full_document(page) {
        if let Some(pos) = body_tag_end(page) {
            return format!("{}{}{}", &page[..pos], tag, &page[pos..]);
        }
        let start = doctype_start(page).expect("full document has a doctype");
        let doctype_end = match page.as_bytes()[start..].iter().position(|&b| b == b'>') {
            Some(off) => start + off + 1,
            None => page.len(),
        };
        return format!("{}{}{}", &page[..doctype_end], tag, &page[doctype_end..]);
    }
    format!(
        "<!doctype html><html><head><meta charset=utf8><meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><style>{RESET_CSS}</style></head><body>{tag}{page}</body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_is_wrapped_with_skeleton_and_bridge_first_in_body() {
        let out = wrap_document(
            "<title>T</title><style>p{}</style><p>hi</p>",
            "7q3k9mzx2b4t",
            2,
            "0.2.61",
        );
        assert!(
            out.starts_with("<!doctype html><html><head><meta charset=utf8><meta name=viewport")
        );
        let body = out.find("<body>").unwrap();
        let tag = out.find("<script src=\"/_artifax/bridge.js\"").unwrap();
        assert!(tag > body && tag < out.find("<title>").unwrap());
        assert!(out.contains(
            "data-artifact=\"7q3k9mzx2b4t\" data-version=\"2\" data-contract=\"0.2.61\""
        ));
        assert!(out.ends_with("</body></html>"));
    }

    #[test]
    fn full_document_is_recognised_case_insensitively_and_bridge_goes_after_body_tag() {
        let page = "\n  <!DOCTYPE HTML><html><head><title>x</title></head><body class=\"x\" data-a=\"1\"><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        assert!(out.starts_with("\n  <!DOCTYPE HTML>"), "served as-is");
        let body_end = out.find("<body class=\"x\" data-a=\"1\">").unwrap()
            + "<body class=\"x\" data-a=\"1\">".len();
        assert!(out[body_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")));
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
        );
        assert!(out.starts_with(&format!(
            "<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")
        )));
    }

    #[test]
    fn xhtml_doctype_with_body_gets_bridge_after_body_and_is_not_double_wrapped() {
        let page = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\"><html><head><title>x</title></head><body><p>hi</p></body></html>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        let body_end = out.find("<body>").unwrap() + "<body>".len();
        assert!(out[body_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")));
        assert!(out.starts_with("<!DOCTYPE html PUBLIC"));
        assert_eq!(out.to_lowercase().matches("<!doctype").count(), 1);
    }

    #[test]
    fn doctype_with_space_and_no_body_gets_bridge_after_its_closing_bracket() {
        let out = wrap_document("<!doctype html ><p>x</p>", "7q3k9mzx2b4t", 1, "0.2.61");
        assert!(out.starts_with(&format!(
            "<!doctype html >{}<p>x</p>",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")
        )));
    }

    #[test]
    fn bom_prefixed_doctype_is_recognised_and_bom_preserved() {
        let page = "\u{FEFF}<!doctype html><p>x</p>";
        assert!(is_full_document(page));
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        assert!(out.starts_with(&format!(
            "\u{FEFF}<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")
        )));
    }

    #[test]
    fn body_inside_a_comment_is_not_the_body_tag() {
        let page = "<!doctype html><html><head><!-- <body> --></head><body><p></p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        let real = out
            .find("<body><script")
            .expect("bridge after real body tag");
        assert!(real > out.find("-->").unwrap());
    }

    #[test]
    fn non_ascii_before_body_tag_does_not_panic() {
        let page =
            "<!doctype html><html><head><title>é — ü</title></head><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        let body_end = out.find("<body>").unwrap() + "<body>".len();
        assert!(out[body_end..].starts_with(&bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")));
    }

    #[test]
    fn non_ascii_before_missing_body_tag_does_not_panic() {
        let page = "<!doctype html><title>é — ü</title><p>x</p>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        assert!(out.starts_with(&format!(
            "<!doctype html>{}",
            bridge_tag("7q3k9mzx2b4t", 1, "0.2.61")
        )));
    }

    #[test]
    fn body_inside_script_is_not_the_body_tag() {
        let page = "<!doctype html><html><head><script>var a=\"<body>\";</script></head><body><p>x</p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        // The real <body> tag should have the bridge after it
        let body_end = out
            .find("<body><script src=\"/_artifax/bridge.js\"")
            .unwrap()
            + "<body>".len();
        assert!(
            out[body_end..].starts_with("<script src=\"/_artifax/bridge.js\""),
            "bridge should be directly after real body tag"
        );
        assert!(out.contains("<script>var a=\"<body>\";"));
    }

    #[test]
    fn unterminated_comment_or_raw_text_hides_the_rest_of_the_page() {
        let tag = bridge_tag("7q3k9mzx2b4t", 1, "0.2.61");
        for page in [
            "<!doctype html><!-- x<body>",
            "<!doctype html><script>x<body>",
            "<!doctype html><style>x<body>",
            "<!doctype html><script>let s = 1;</scrip<body>",
        ] {
            let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
            assert_eq!(
                out,
                format!("<!doctype html>{tag}{}", &page["<!doctype html>".len()..]),
                "{page}"
            );
        }
    }

    #[test]
    fn bodyx_is_not_body_and_body_with_newline_is() {
        let page1 = "<!doctype html><bodyx><p>x</p>";
        let out1 = wrap_document(page1, "id1", 1, "0.2.61");
        assert!(out1.contains("<bodyx><p>x</p>"));

        let page2 = "<!doctype html><body\n class=\"a\"><p>x</p></body></html>";
        let out2 = wrap_document(page2, "id2", 1, "0.2.61");
        let body_end = out2.find("<body\n class=\"a\">").unwrap() + "<body\n class=\"a\">".len();
        assert!(out2[body_end..].starts_with(&bridge_tag("id2", 1, "0.2.61")));
    }
}
