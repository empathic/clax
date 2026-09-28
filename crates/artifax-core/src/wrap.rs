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
            match lower[i..].find("-->") {
                Some(e) => {
                    i += e + 3;
                    continue;
                }
                None => return None,
            }
        }
        if lower[i..].starts_with("<body") {
            let after = i + 5;
            let next = bytes.get(after).copied();
            if matches!(
                next,
                Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
            ) {
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
    fn body_inside_a_comment_or_attribute_is_not_the_body_tag() {
        let page = "<!doctype html><html><head><!-- <body> --></head><body><p></p></body></html>";
        let out = wrap_document(page, "7q3k9mzx2b4t", 1, "0.2.61");
        let real = out
            .find("<body><script")
            .expect("bridge after real body tag");
        assert!(real > out.find("-->").unwrap());
    }
}
