//! How an opener hands a browser a one-use login link without putting it
//! on a command line, where any user of the machine can read it (`ps`,
//! `/proc/<pid>/cmdline`) and race the browser to redeem it.
//!
//! The link goes into a small page under the Clax home, [`Home::login_dir`]
//! (mode 0700), in a file of its own (mode 0600) named for the code's
//! SHA-256 ([`page_name`]), and the opener is given the file's path. The
//! page sends the browser on to the link (`<meta http-equiv="refresh">` and
//! `location.replace`) and loads nothing else. The daemon removes the page
//! when its code is redeemed ([`remove`]), and every write first removes
//! the pages older than a code lives ([`LOGIN_CODE_TTL_SECS`]).

use crate::home::Home;
use crate::secret::sha256_hex;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// How long a login code may be redeemed, from when it was made.
pub const LOGIN_CODE_TTL_SECS: i64 = 120;

impl Home {
    /// `login/`: the pages that carry a login link to the browser an
    /// opener starts ([`crate::login_page`]).
    pub fn login_dir(&self) -> PathBuf {
        self.root().join("login")
    }
}

/// The login code in `url`'s query (`code=…`), if any.
pub fn code_of(url: &str) -> Option<&str> {
    let query = url.split_once('?')?.1.split('#').next()?;
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("code="))
        .filter(|c| !c.is_empty())
}

/// The file name of the page that carries login code `code`: its SHA-256,
/// which only a holder of the code can name.
pub fn page_name(code: &str) -> String {
    format!("{}.html", sha256_hex(code))
}

/// The page that sends its browser on to `url`.
pub fn page(url: &str) -> String {
    let attr = url
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let script = serde_json::to_string(url)
        .unwrap_or_else(|_| "\"/\"".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<meta name=\"referrer\" content=\"no-referrer\">\n\
         <meta http-equiv=\"refresh\" content=\"0;url={attr}\">\n<title>Opening Clax</title>\n\
         <script>location.replace({script})</script>\n<p><a href=\"{attr}\">Open Clax</a></p>\n"
    )
}

/// Writes the page for login link `url` into `dir` (made mode 0700 if
/// missing, and set to 0700), as a new file of mode 0600 named
/// [`page_name`] of the link's code, after removing the pages older than
/// [`LOGIN_CODE_TTL_SECS`]. Returns the page's path.
///
/// # Errors
/// `url` carries no code, or the directory or file cannot be written.
pub fn write(dir: &Path, url: &str) -> std::io::Result<PathBuf> {
    let code = code_of(url).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "the link carries no code")
    })?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    sweep(dir, std::time::SystemTime::now());
    let path = dir.join(page_name(code));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let mut f = open.open(&path)?;
    f.write_all(page(url).as_bytes())?;
    Ok(path)
}

/// Removes the page of login code `code` from `dir`, if there is one.
pub fn remove(dir: &Path, code: &str) {
    let _ = std::fs::remove_file(dir.join(page_name(code)));
}

/// Removes the pages in `dir` last written more than
/// [`LOGIN_CODE_TTL_SECS`] before `now`: their codes can no longer be
/// redeemed.
pub fn sweep(dir: &Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ttl = std::time::Duration::from_secs(LOGIN_CODE_TTL_SECS.unsigned_abs());
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "html") {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > ttl));
        if stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "http://localhost:7/login?code=cxi_abc&next=%2Fa%2Fx";

    #[test]
    fn the_code_is_read_from_the_query() {
        assert_eq!(code_of(LINK), Some("cxi_abc"));
        assert_eq!(
            code_of("http://localhost:7/login?next=%2F&code=cxi_z#f"),
            Some("cxi_z")
        );
        assert_eq!(code_of("http://localhost:7/a/x"), None);
        assert_eq!(code_of("http://localhost:7/login?code="), None);
    }

    #[test]
    fn the_page_sends_the_browser_on_and_loads_nothing_else() {
        let p = page("http://localhost:7/login?code=c&next=\"</script>");
        assert!(
            p.contains(
                "content=\"0;url=http://localhost:7/login?code=c&amp;next=&quot;&lt;/script&gt;\""
            ),
            "{p}"
        );
        assert!(p.contains("location.replace(\"http://localhost:7/login?code=c&next=\\\"\\u003c/script\\u003e\")"), "{p}");
        assert!(!p.contains("src="), "{p}");
    }

    #[test]
    fn a_page_is_its_owners_alone_and_removed_once_spent() {
        let dir = tempfile::tempdir().unwrap();
        let login = dir.path().join("login");
        let path = write(&login, LINK).unwrap();
        assert_eq!(path, login.join(page_name("cxi_abc")));
        assert!(!path.to_string_lossy().contains("cxi_"));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("code=cxi_abc")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&login), 0o700);
            assert_eq!(mode(&path), 0o600);
        }
        assert!(write(&login, "http://localhost:7/a/x").is_err());
        remove(&login, "cxi_abc");
        assert!(!path.exists());
    }

    #[test]
    fn pages_older_than_a_code_are_swept() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), LINK).unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        sweep(dir.path(), written + std::time::Duration::from_secs(120));
        assert!(path.exists(), "a page within its code's life stays");
        sweep(dir.path(), written + std::time::Duration::from_secs(121));
        assert!(!path.exists());
    }
}
