//! Scrubbing (spec 2026-10-29-feedback-loop-design §5.3): the one rule set
//! that replaces credentials and personal data in what a reporter's browser
//! sends (diagnostics, the page URL, the app's context) by [`REDACTED`].
//! The SDK applies the same rules before sending and Clax applies them
//! again on receipt; this module is their source of truth, and
//! `web/sdk/test/scrub-cases.json` is the table of cases both test suites
//! run.
//!
//! [`scrub`] reads the text left to right. At each position it tries the
//! rules below in order; the first that matches replaces what it names by
//! [`REDACTED`] and the scan resumes after the match. Every pattern is
//! ASCII; letters in key names and schemes match in any case. "Base64url"
//! characters are `A-Z a-z 0-9 - _`; a rule that says "starts a word"
//! applies only where the previous character is not an ASCII letter or
//! digit (or at the start of the text).
//!
//! 1. **Bearer and Basic credentials**: `bearer` or `basic` starting a
//!    word, one or more spaces or tabs, then a token of at least 8
//!    characters of `A-Z a-z 0-9 - . _ ~ + /` followed by any `=`. The
//!    token is replaced; the scheme and the spaces stay.
//! 2. **Clax tokens**: `cx`, one lowercase letter and `_` (`cxq_`,
//!    `cxr_`, …), starting a word that `_` does not precede either,
//!    followed by at least 16 base64url characters. The whole token is
//!    replaced.
//! 3. **JWT-shaped strings**: three maximal runs of at least 8 base64url
//!    characters joined by `.`, the first not preceded by a base64url
//!    character. The whole string is replaced.
//! 4. **Secret pairs**: one of `password`, `passwd`, `token`, `secret`,
//!    `key`, `api_key`, `access_token`, `auth`, preceded by the start of
//!    the text or by a character that is not an ASCII letter or digit (so
//!    `GITHUB_TOKEN=` and `x-api-key:` match, `monkey=` does not); then an
//!    optional `"` or `'`, then `=` or `:`, then optional spaces or tabs,
//!    then an optional `"` or `'`, then a value of at least one character
//!    running up to whitespace, `&`, `"` or `'`. The value is replaced; the
//!    key and the separator stay.
//! 5. **Email addresses**: one or more of `A-Z a-z 0-9 . _ % + -` not
//!    preceded by one of them, `@`, then two or more labels of
//!    `A-Z a-z 0-9 -` joined by `.` whose last is at least two ASCII
//!    letters (a trailing `.` or `-` ends the address). The address is
//!    replaced.
//! 6. **Long runs**: a maximal run of `A-Z a-z 0-9 + - _` (`/` and `.`
//!    end a run, so paths and host names are never one) followed by any
//!    `=`, when it is at least 32 hexadecimal digits, or at least 40
//!    characters with at least one digit. The run is replaced.
//!
//! Text already holding [`REDACTED`] in place of a secret scrubs to
//! itself.

use std::borrow::Cow;

/// What a scrubbed secret becomes.
pub const REDACTED: &str = "‹redacted›";

/// The key names whose values a secret pair (rule 4) replaces, matched in
/// any case.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "token",
    "secret",
    "key",
    "api_key",
    "access_token",
    "auth",
];

/// `s` with every match of the module's rules replaced by [`REDACTED`],
/// borrowed when nothing matched.
pub fn scrub(s: &str) -> Cow<'_, str> {
    let b = s.as_bytes();
    let mut out: Option<String> = None;
    // The end of the text copied to `out` so far.
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        match find_at(b, i) {
            Some((start, end)) => {
                if &s[start..end] != REDACTED {
                    let o = out.get_or_insert_with(|| String::with_capacity(s.len()));
                    o.push_str(&s[copied..start]);
                    o.push_str(REDACTED);
                    copied = end;
                }
                i = end.max(i + 1);
            }
            None => i += 1,
        }
    }
    match out {
        None => Cow::Borrowed(s),
        Some(mut o) => {
            o.push_str(&s[copied..]);
            Cow::Owned(o)
        }
    }
}

/// `url` with its scheme, host, path and query keys kept and every query
/// value replaced by [`REDACTED`] (an empty value stays empty). The path,
/// a query entry with no `=`, and the fragment are [`scrub`]bed; a user
/// name or password before the host is replaced. A string with no
/// `scheme://` is treated as a path with an optional query and fragment.
pub fn scrub_url(url: &str) -> String {
    let (before_frag, frag) = match url.split_once('#') {
        Some((a, f)) => (a, Some(f)),
        None => (url, None),
    };
    let (base, query) = match before_frag.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (before_frag, None),
    };
    let mut out = String::with_capacity(url.len());
    match base.find("://") {
        Some(at) => {
            let (scheme, rest) = base.split_at(at + 3);
            out.push_str(scheme);
            let (authority, path) = match rest.find('/') {
                Some(p) => rest.split_at(p),
                None => (rest, ""),
            };
            match authority.rsplit_once('@') {
                Some((_, host)) => {
                    out.push_str(REDACTED);
                    out.push('@');
                    out.push_str(host);
                }
                None => out.push_str(authority),
            }
            out.push_str(&scrub(path));
        }
        None => out.push_str(&scrub(base)),
    }
    if let Some(q) = query {
        out.push('?');
        for (n, entry) in q.split('&').enumerate() {
            if n > 0 {
                out.push('&');
            }
            match entry.split_once('=') {
                Some((k, v)) => {
                    out.push_str(&scrub(k));
                    out.push('=');
                    if !v.is_empty() {
                        out.push_str(REDACTED);
                    }
                }
                None => out.push_str(&scrub(entry)),
            }
        }
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(&scrub(f));
    }
    out
}

/// The byte range at `i` that the first matching rule replaces.
fn find_at(b: &[u8], i: usize) -> Option<(usize, usize)> {
    bearer(b, i)
        .or_else(|| clax_token(b, i))
        .or_else(|| jwt(b, i))
        .or_else(|| secret_pair(b, i))
        .or_else(|| email(b, i))
        .or_else(|| long_run(b, i))
}

fn is_b64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

fn is_b64token(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
}

fn is_email_local(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_run(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'_')
}

/// Whether a word starts at `i`: the previous byte is not an ASCII letter
/// or digit.
fn word_start(b: &[u8], i: usize) -> bool {
    i == 0 || !b[i - 1].is_ascii_alphanumeric()
}

/// The end of the run of bytes from `i` that `f` accepts.
fn run_end(b: &[u8], i: usize, f: impl Fn(u8) -> bool) -> usize {
    let mut j = i;
    while j < b.len() && f(b[j]) {
        j += 1;
    }
    j
}

/// Whether `b[i..]` starts with `word`, in any case.
fn starts_with_ci(b: &[u8], i: usize, word: &str) -> bool {
    b.len() >= i + word.len() && b[i..i + word.len()].eq_ignore_ascii_case(word.as_bytes())
}

/// Rule 1: the token after `bearer` or `basic`.
fn bearer(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !word_start(b, i) {
        return None;
    }
    let scheme = ["bearer", "basic"]
        .into_iter()
        .find(|w| starts_with_ci(b, i, w))?;
    let after = i + scheme.len();
    let start = run_end(b, after, |c| c == b' ' || c == b'\t');
    if start == after {
        return None;
    }
    let body = run_end(b, start, is_b64token);
    if body - start < 8 {
        return None;
    }
    Some((start, run_end(b, body, |c| c == b'=')))
}

/// Rule 2: a `cx?_` token.
fn clax_token(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !word_start(b, i) || (i > 0 && b[i - 1] == b'_') || b.len() < i + 4 {
        return None;
    }
    if !(b[i] == b'c' && b[i + 1] == b'x' && b[i + 2].is_ascii_lowercase() && b[i + 3] == b'_') {
        return None;
    }
    let end = run_end(b, i + 4, is_b64url);
    (end - (i + 4) >= 16).then_some((i, end))
}

/// Rule 3: a JWT-shaped string.
fn jwt(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if i > 0 && is_b64url(b[i - 1]) {
        return None;
    }
    let mut at = i;
    for part in 0..3 {
        if part > 0 {
            if b.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
        let end = run_end(b, at, is_b64url);
        if end - at < 8 {
            return None;
        }
        at = end;
    }
    Some((i, at))
}

/// Rule 4: the value of a secret pair.
fn secret_pair(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !word_start(b, i) {
        return None;
    }
    let key = SECRET_KEYS.iter().find(|k| starts_with_ci(b, i, k))?;
    let mut at = i + key.len();
    if matches!(b.get(at), Some(b'"' | b'\'')) {
        at += 1;
    }
    if !matches!(b.get(at), Some(b'=' | b':')) {
        return None;
    }
    at = run_end(b, at + 1, |c| c == b' ' || c == b'\t');
    if matches!(b.get(at), Some(b'"' | b'\'')) {
        at += 1;
    }
    let end = run_end(b, at, |c| {
        !(c.is_ascii_whitespace() || matches!(c, b'&' | b'"' | b'\''))
    });
    (end > at).then_some((at, end))
}

/// Rule 5: an email address.
fn email(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if i > 0 && is_email_local(b[i - 1]) {
        return None;
    }
    let local = run_end(b, i, is_email_local);
    if local == i || b.get(local) != Some(&b'@') {
        return None;
    }
    let start = local + 1;
    let mut end = run_end(b, start, |c| {
        c.is_ascii_alphanumeric() || c == b'.' || c == b'-'
    });
    while end > start && matches!(b[end - 1], b'.' | b'-') {
        end -= 1;
    }
    let domain = &b[start..end];
    let mut labels = domain.split(|&c| c == b'.');
    let count = labels.clone().count();
    if count < 2 || labels.any(<[u8]>::is_empty) {
        return None;
    }
    let tld = domain.rsplit(|&c| c == b'.').next()?;
    (tld.len() >= 2 && tld.iter().all(u8::is_ascii_alphabetic)).then_some((i, end))
}

/// Rule 6: a long hex or base64 run.
fn long_run(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if i > 0 && is_run(b[i - 1]) {
        return None;
    }
    let end = run_end(b, i, is_run);
    let run = &b[i..end];
    let hex = run.len() >= 32 && run.iter().all(u8::is_ascii_hexdigit);
    let b64 = run.len() >= 40 && run.iter().any(u8::is_ascii_digit);
    (hex || b64).then(|| (i, run_end(b, end, |c| c == b'=')))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of `web/sdk/test/scrub-cases.json`: `in` is [`scrub`]bed,
    /// `url` is [`scrub_url`]ed.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Case {
        #[serde(rename = "in")]
        input: Option<String>,
        url: Option<String>,
        out: String,
    }

    #[test]
    fn every_shared_case_scrubs_as_listed() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../../web/sdk/test/scrub-cases.json")).unwrap();
        assert!(cases.len() >= 9);
        for c in cases {
            match (c.input, c.url) {
                (Some(s), None) => {
                    assert_eq!(scrub(&s), c.out, "scrub({s:?})");
                    assert_eq!(scrub(&c.out), c.out, "scrub is idempotent on {s:?}");
                }
                (None, Some(u)) => {
                    assert_eq!(scrub_url(&u), c.out, "scrub_url({u:?})");
                    assert_eq!(scrub_url(&c.out), c.out, "scrub_url is idempotent on {u:?}");
                }
                _ => panic!("a case names exactly one of in and url"),
            }
        }
    }

    #[test]
    fn scrub_borrows_when_nothing_matches() {
        assert!(matches!(scrub("ok"), Cow::Borrowed(_)));
        assert!(matches!(scrub("token=‹redacted›"), Cow::Borrowed(_)));
        assert!(matches!(scrub("token=x"), Cow::Owned(_)));
    }

    #[test]
    fn patterns_cut_short_or_beside_multibyte_text_scrub_safely() {
        for s in [
            "cx", "cxq_", "Bearer", "Bearer ", "a@", "@x.io", "token", "token=", "‹", "a.b.",
        ] {
            assert_eq!(scrub(s), s);
        }
        assert_eq!(scrub("token=é"), format!("token={REDACTED}"));
        assert_eq!(scrub("é token=x é"), format!("é token={REDACTED} é"));
        assert_eq!(scrub("ünï@exämple.com"), "ünï@exämple.com");
        assert_eq!(scrub_url(""), "");
        assert_eq!(scrub_url("?"), "?");
        assert_eq!(scrub_url("https://"), "https://");
    }
}
