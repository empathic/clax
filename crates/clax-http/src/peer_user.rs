//! Which user of the machine a connection comes from, on Linux, read from
//! `/proc` with no system call of our own: the daemon's real UID from
//! `/proc/self/status`, and the owner of the peer's socket from
//! `/proc/net/tcp` and `/proc/net/tcp6`.
//!
//! `GET /login` uses it: on Linux the opener (`xdg-open`) takes a login
//! link on its command line, which every user of the machine can read, so
//! the route redeems a code only for a connection whose socket belongs to
//! the user the daemon runs as ([`redeemer_is_this_user`]). Elsewhere the
//! link reaches no command line (macOS hands it to `osascript` on standard
//! input) and [`default_proc_root`] is None.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

/// `/proc` on Linux, where `GET /login` checks the redeeming user; None
/// elsewhere.
pub fn default_proc_root() -> Option<PathBuf> {
    cfg!(target_os = "linux").then(|| PathBuf::from("/proc"))
}

/// The real UID in a `/proc/<pid>/status` text: the first number of its
/// `Uid:` line.
pub fn real_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// An address of a `/proc/net/tcp{,6}` row (`<hex address>:<hex port>`):
/// the address is the kernel's in-memory words, each printed as a native
/// `u32` (`%08X`), the port a host-order number.
fn parse_addr(field: &str) -> Option<SocketAddr> {
    let (addr, port) = field.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |i: usize| {
        addr.get(i * 8..i * 8 + 8)
            .and_then(|w| u32::from_str_radix(w, 16).ok())
            .map(u32::to_ne_bytes)
    };
    let ip = match addr.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word(0)?)),
        32 => {
            let mut b = [0u8; 16];
            for i in 0..4 {
                b[i * 4..i * 4 + 4].copy_from_slice(&word(i)?);
            }
            IpAddr::V6(Ipv6Addr::from(b))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// Whether two socket addresses name the same endpoint, an IPv4-mapped
/// IPv6 address counting as its IPv4 address.
fn same(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() == b.port() && a.ip().to_canonical() == b.ip().to_canonical()
}

/// The UID owning the socket whose local address is `local` and whose
/// remote address is `remote`, from the text of `/proc/net/tcp` or
/// `/proc/net/tcp6` (a header line, then rows of `sl local_address
/// rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid …`).
pub fn socket_uid(table: &str, local: SocketAddr, remote: SocketAddr) -> Option<u32> {
    table.lines().skip(1).find_map(|row| {
        let f: Vec<&str> = row.split_whitespace().collect();
        let (l, r) = (parse_addr(f.get(1)?)?, parse_addr(f.get(2)?)?);
        (same(l, local) && same(r, remote))
            .then(|| f.get(7)?.parse().ok())
            .flatten()
    })
}

/// Whether the connection from `peer` to the daemon at `local` belongs to
/// the user the daemon runs as, read under `proc` (`/proc`): the peer's
/// socket is the row whose local address is `peer` and whose remote
/// address is `local`. An error, saying what could not be read or found,
/// when it cannot be told; the caller refuses then.
pub fn redeemer_is_this_user(
    proc: &Path,
    peer: SocketAddr,
    local: SocketAddr,
) -> Result<bool, String> {
    let read = |p: &str| std::fs::read_to_string(proc.join(p));
    let status = read("self/status").map_err(|e| format!("self/status: {e}"))?;
    let me = real_uid(&status).ok_or("self/status names no Uid")?;
    let tables: Vec<String> = ["net/tcp", "net/tcp6"]
        .iter()
        .filter_map(|p| read(p).ok())
        .collect();
    if tables.is_empty() {
        return Err("net/tcp and net/tcp6 cannot be read".into());
    }
    tables
        .iter()
        .find_map(|t| socket_uid(t, peer, local))
        .map(|uid| uid == me)
        .ok_or_else(|| "no socket of the connection is listed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_real_uid_is_the_first_of_the_uid_line() {
        let status = "Name:\tclax\nUmask:\t0022\nUid:\t1000\t1001\t1002\t1003\nGid:\t100\n";
        assert_eq!(real_uid(status), Some(1000));
        assert_eq!(real_uid("Name:\tclax\n"), None);
    }

    // The rows below are as a little-endian machine prints them.
    #[cfg(target_endian = "little")]
    #[test]
    fn an_ipv4_peer_socket_is_found_by_both_ends() {
        // The daemon on 127.0.0.1:7480 (0x1D38) and a browser's socket from
        // 127.0.0.1:51000 (0xC738), with its listening and accepted rows.
        let table = format!(
            "{HEADER}   0: 0100007F:1D38 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1\n\
             1: 0100007F:1D38 0100007F:C738 01 00000000:00000000 00:00000000 00000000  1000        0 2 1\n\
             2: 0100007F:C738 0100007F:1D38 01 00000000:00000000 00:00000000 00000000  1001        0 3 1\n"
        );
        let (daemon, browser) = (a("127.0.0.1:7480"), a("127.0.0.1:51000"));
        assert_eq!(socket_uid(&table, browser, daemon), Some(1001));
        assert_eq!(socket_uid(&table, daemon, browser), Some(1000));
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn an_ipv6_peer_socket_is_found() {
        // ::1 is the words 0, 0, 0, 0x01000000 as printed natively.
        let table = format!(
            "{HEADER}   0: 00000000000000000000000001000000:C738 00000000000000000000000001000000:1D38 01 00000000:00000000 00:00000000 00000000  1002        0 3 1\n"
        );
        assert_eq!(socket_uid(&table, a("[::1]:51000"), a("[::1]:7480")), Some(1002));
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn an_ipv4_mapped_row_matches_an_ipv4_connection() {
        // ::ffff:127.0.0.1 in tcp6, seen by a daemon as 127.0.0.1, and the
        // other way round.
        let table = format!(
            "{HEADER}   0: 0000000000000000FFFF00000100007F:C738 0000000000000000FFFF00000100007F:1D38 01 00000000:00000000 00:00000000 00000000  1003        0 3 1\n"
        );
        assert_eq!(socket_uid(&table, a("127.0.0.1:51000"), a("127.0.0.1:7480")), Some(1003));
        assert_eq!(
            socket_uid(&table, a("[::ffff:127.0.0.1]:51000"), a("[::ffff:127.0.0.1]:7480")),
            Some(1003)
        );
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn no_row_names_no_user() {
        let table = format!(
            "{HEADER}   0: 0100007F:C738 0100007F:1D38 01 00000000:00000000 00:00000000 00000000  1001        0 3 1\n"
        );
        assert_eq!(socket_uid(&table, a("127.0.0.1:51001"), a("127.0.0.1:7480")), None);
        assert_eq!(socket_uid(HEADER, a("127.0.0.1:51000"), a("127.0.0.1:7480")), None);
        assert_eq!(socket_uid("", a("127.0.0.1:51000"), a("127.0.0.1:7480")), None);
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn the_redeemer_is_compared_with_the_daemons_user() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("self")).unwrap();
        std::fs::create_dir_all(dir.path().join("net")).unwrap();
        let (daemon, browser) = (a("127.0.0.1:7480"), a("127.0.0.1:51000"));
        // Nothing to read: refused, saying why.
        assert!(redeemer_is_this_user(dir.path(), browser, daemon).is_err());
        std::fs::write(dir.path().join("self/status"), "Uid:\t1000\t1000\t1000\t1000\n").unwrap();
        assert!(redeemer_is_this_user(dir.path(), browser, daemon).is_err());
        let row = |uid: u32| {
            format!("{HEADER}   0: 0100007F:C738 0100007F:1D38 01 00000000:00000000 00:00000000 00000000  {uid}        0 3 1\n")
        };
        std::fs::write(dir.path().join("net/tcp"), row(1001)).unwrap();
        assert_eq!(redeemer_is_this_user(dir.path(), browser, daemon), Ok(false));
        std::fs::write(dir.path().join("net/tcp"), row(1000)).unwrap();
        assert_eq!(redeemer_is_this_user(dir.path(), browser, daemon), Ok(true));
        // A connection no row lists.
        assert!(redeemer_is_this_user(dir.path(), a("127.0.0.1:51001"), daemon).is_err());
    }
}
