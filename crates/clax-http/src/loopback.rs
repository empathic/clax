//! Which addresses name this machine's own daemon: the pages a device
//! sign-in may return its person to.

use url::{Host, Url};

/// The longest address [`loopback_return`] accepts, in bytes.
pub const MAX_RETURN_LEN: usize = 2048;

/// `url` normalised when it names a page on this machine: scheme `http`,
/// host `localhost`, `127.0.0.1` or `[::1]`, no userinfo and no fragment,
/// at most [`MAX_RETURN_LEN`] bytes; else `None`.
#[must_use]
pub fn loopback_return(url: &str) -> Option<String> {
    if url.len() > MAX_RETURN_LEN {
        return None;
    }
    let u = Url::parse(url).ok()?;
    let local = match u.host()? {
        Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(a) => a.octets() == [127, 0, 0, 1],
        Host::Ipv6(a) => a == std::net::Ipv6Addr::LOCALHOST,
    };
    if u.scheme() != "http"
        || !local
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    let out = u.to_string();
    (out.len() <= MAX_RETURN_LEN).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::loopback_return;

    #[test]
    fn loopback_return_accepts_only_this_machine() {
        for ok in [
            "http://localhost:7480/a/x?publish=1",
            "http://127.0.0.1:7481/",
            "http://[::1]:7480/a/x",
        ] {
            assert_eq!(loopback_return(ok).as_deref(), Some(ok), "{ok}");
        }
        let long = format!("http://localhost/{}", "a".repeat(2049 - 17));
        assert_eq!(long.len(), 2049);
        for bad in [
            "https://localhost/",
            "http://localhost.evil.example/",
            "http://user@localhost/",
            "http://localhost/#f",
            "javascript:alert(1)",
            "http://localhost@evil.example/",
            "",
            &long,
        ] {
            assert_eq!(loopback_return(bad), None, "{bad}");
        }
    }
}
