//! The owner's tailnet devices: which device a request through `tailscale
//! serve` came from, and whether it is one of the owner's.
//!
//! `tailscale serve` names the tailnet user of a request
//! ([`crate::auth::TailscaleUser`]) but not its device. It does put the
//! caller's tailnet address in `X-Forwarded-For` (one address, replacing any
//! the client sent), and `tailscale whois --json <ip>` names the node at that
//! address and its user. With the home's `[serve] owner_tailscale_nodes`
//! ([`clax_core::config::HomeConfig::serve_owner_tailscale_devices`]), a
//! request of the owner's tailnet login is the owner's only when that node
//! is listed and its user is the owner's login; a login on any other device
//! is the tailnet viewer its login names, as any other login is
//! ([`crate::auth::require_api_host`]). Everything fails closed: no
//! `X-Forwarded-For`, a repeated one, a list, an address `whois` does not
//! answer for in time, or an answer that does not parse is not the owner.
//!
//! A local process can forge every header straight to the daemon's loopback
//! port, `X-Forwarded-For` too, but it is this machine's, which is the
//! owner's anyway ([`crate::identity`]); the device check closes the path
//! through `tailscale serve` from another device signed in as the owner.
//!
//! A node's address does not change while it is in the tailnet, so an
//! answer is kept for [`WHOIS_TTL`] by the state's clock; failures are not
//! kept.

use async_trait::async_trait;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// How long a `whois` answer is kept for its address.
pub const WHOIS_TTL: chrono::Duration = chrono::Duration::seconds(60);
/// The most addresses kept at once; past it the cache starts over.
const CACHE_CAP: usize = 256;

/// The node `tailscale whois` names for an address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhoisNode {
    /// `Node.StableID`, such as `nikWRyfT1721CNTRL`.
    pub stable_id: String,
    /// `Node.Name`, the MagicDNS name, such as `iphone171.tail556f5b.ts.net.`.
    pub name: String,
    /// `UserProfile.LoginName`, such as `bdelanghe@github`.
    pub login: String,
}

/// Names the node at a tailnet address.
#[async_trait]
pub trait Whois: Send + Sync {
    async fn whois(&self, ip: IpAddr) -> Result<WhoisNode, String>;
}

/// `tailscale whois --json` output as a [`WhoisNode`]; `Err` without a
/// non-empty `Node.StableID`, `Node.Name` and `UserProfile.LoginName`.
pub fn parse_whois(json: &[u8]) -> Result<WhoisNode, String> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("whois answered no JSON: {e}"))?;
    let field = |path: &[&str]| {
        path.iter()
            .try_fold(&v, |v, k| v.get(k))
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("whois answered no {}", path.join(".")))
    };
    Ok(WhoisNode {
        stable_id: field(&["Node", "StableID"])?,
        name: field(&["Node", "Name"])?,
        login: field(&["UserProfile", "LoginName"])?,
    })
}

/// The one address in the request's one `X-Forwarded-For`; `None` when it is
/// missing, repeated, a list, or not an address.
pub fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let mut all = headers.get_all("x-forwarded-for").iter();
    let first = all.next()?;
    if all.next().is_some() {
        return None;
    }
    first.to_str().ok()?.trim().parse().ok()
}

/// A MagicDNS name compared without case or its trailing dot.
fn node_name(n: &str) -> String {
    n.strip_suffix('.').unwrap_or(n).to_ascii_lowercase()
}

/// The owner's devices (`[serve] owner_tailscale_nodes`) and how a
/// request's device is named.
pub struct OwnerDevices {
    nodes: Vec<String>,
    whois: Arc<dyn Whois>,
    clock: Arc<dyn clax_core::working::Clock>,
    cache: Mutex<HashMap<IpAddr, (DateTime<Utc>, WhoisNode)>>,
}

impl OwnerDevices {
    pub fn new(
        nodes: Vec<String>,
        whois: Arc<dyn Whois>,
        clock: Arc<dyn clax_core::working::Clock>,
    ) -> OwnerDevices {
        OwnerDevices {
            nodes,
            whois,
            clock,
            cache: Mutex::default(),
        }
    }

    /// Whether `node` is one of the owner's devices: its stable ID, or its
    /// MagicDNS name without case or trailing dot.
    pub fn lists(&self, node: &WhoisNode) -> bool {
        self.nodes
            .iter()
            .any(|n| *n == node.stable_id || node_name(n) == node_name(&node.name))
    }

    /// The owner's device a request of the owner's tailnet `login` came
    /// from, or why it is not one.
    pub async fn device_of(&self, headers: &HeaderMap, login: &str) -> Result<WhoisNode, String> {
        let ip = forwarded_for(headers)
            .ok_or("X-Forwarded-For is missing, repeated or not one address")?;
        let node = self.node_at(ip).await?;
        if node.login != login {
            return Err(format!("{ip} is a node of {}, not of {login}", node.login));
        }
        if !self.lists(&node) {
            return Err(format!(
                "{} ({}) is not one of the owner's devices",
                node.name, node.stable_id
            ));
        }
        Ok(node)
    }

    async fn node_at(&self, ip: IpAddr) -> Result<WhoisNode, String> {
        let now = self.clock.now();
        if let Some((at, node)) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            && now - *at < WHOIS_TTL
            && *at <= now
        {
            return Ok(node.clone());
        }
        let node = self.whois.whois(ip).await?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(ip, (now, node.clone()));
        Ok(node)
    }
}

/// `whois` by running `<tailscale> whois --json <ip>`, for at most
/// `timeout`; a timeout, a failed run or a non-zero exit is an `Err`.
#[cfg(not(target_arch = "wasm32"))]
pub struct CliWhois {
    pub tailscale: std::path::PathBuf,
    pub timeout: std::time::Duration,
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl Whois for CliWhois {
    async fn whois(&self, ip: IpAddr) -> Result<WhoisNode, String> {
        let run = tokio::process::Command::new(&self.tailscale)
            .args(["whois", "--json", &ip.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(self.timeout, run)
            .await
            .map_err(|_| format!("tailscale whois {ip} took longer than {:?}", self.timeout))?
            .map_err(|e| format!("could not run {}: {e}", self.tailscale.display()))?;
        if !out.status.success() {
            return Err(format!("tailscale whois {ip} exited {}", out.status));
        }
        parse_whois(&out.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `tailscale whois --json 100.98.28.5` on phobos, 2026-10-28 (tailscale
    /// 1.102.2): Bobby's phone, with `jq 'del(.Node.Endpoints, .Node.Key,
    /// .Node.DiscoKey, .Node.Hostinfo, .Node.KeyExpiry, .Node.HomeDERP,
    /// .Node.AllowedIPs, .UserProfile.ProfilePicURL)'`; the rest as recorded.
    pub(crate) const PHONE: &str = r#"{"Node":{"ID":8197052596022907,"StableID":"nikWRyfT1721CNTRL","Name":"iphone171.tail556f5b.ts.net.","User":7959848025173258,"Addresses":["100.98.28.5/32","fd7a:115c:a1e0::430:1c06/128"],"Created":"2026-09-17T19:20:26.96117238Z","Cap":148,"LastSeen":"2026-10-28T17:10:00.1Z","Online":true,"ComputedName":"iphone171","ComputedNameWithHost":"iphone171 (localhost)"},"UserProfile":{"ID":7959848025173258,"LoginName":"bdelanghe@github","DisplayName":"Robert DeLanghe"},"CapMap":null}"#;
    /// The same for 100.114.177.2: orthrus, another of Bobby's nodes.
    pub(crate) const ORTHRUS: &str = r#"{"Node":{"ID":672457026483707,"StableID":"nCd9ijJZF611CNTRL","Name":"orthrus.tail556f5b.ts.net.","User":7959848025173258,"Addresses":["100.114.177.2/32","fd7a:115c:a1e0::cd30:b103/128"],"Created":"2026-10-13T15:44:18.754969961Z","Cap":115,"Online":true,"ComputedName":"orthrus","ComputedNameWithHost":"orthrus"},"UserProfile":{"ID":7959848025173258,"LoginName":"bdelanghe@github","DisplayName":"Robert DeLanghe"},"CapMap":null}"#;

    struct Fixed(HashMap<IpAddr, &'static str>, AtomicUsize);

    #[async_trait]
    impl Whois for Fixed {
        async fn whois(&self, ip: IpAddr) -> Result<WhoisNode, String> {
            self.1.fetch_add(1, Ordering::SeqCst);
            parse_whois(self.0.get(&ip).ok_or("no such node")?.as_bytes())
        }
    }

    struct At(Mutex<DateTime<Utc>>);
    impl clax_core::working::Clock for At {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    fn headers(xff: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in xff {
            h.append("x-forwarded-for", v.parse().unwrap());
        }
        h
    }

    #[test]
    fn the_recorded_answers_parse() {
        assert_eq!(
            parse_whois(PHONE.as_bytes()).unwrap(),
            WhoisNode {
                stable_id: "nikWRyfT1721CNTRL".into(),
                name: "iphone171.tail556f5b.ts.net.".into(),
                login: "bdelanghe@github".into(),
            }
        );
        assert_eq!(
            parse_whois(ORTHRUS.as_bytes()).unwrap().stable_id,
            "nCd9ijJZF611CNTRL"
        );
        for bad in ["", "{}", "[]", r#"{"Node":{"StableID":"x","Name":"y"}}"#] {
            assert!(parse_whois(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_one_forwarded_address_counts() {
        assert_eq!(
            forwarded_for(&headers(&["100.98.28.5"])),
            "100.98.28.5".parse().ok()
        );
        for h in [
            headers(&[]),
            headers(&["100.98.28.5", "100.98.28.5"]),
            headers(&["1.2.3.4, 100.98.28.5"]),
            headers(&["phone"]),
        ] {
            assert_eq!(forwarded_for(&h), None, "{h:?}");
        }
    }

    #[tokio::test]
    async fn a_listed_node_of_the_login_is_the_owners_device_and_is_kept_a_minute() {
        let whois = Arc::new(Fixed(
            HashMap::from([
                ("100.98.28.5".parse().unwrap(), PHONE),
                ("100.114.177.2".parse().unwrap(), ORTHRUS),
            ]),
            AtomicUsize::new(0),
        ));
        let start = Utc::now();
        let clock = Arc::new(At(Mutex::new(start)));
        let d = OwnerDevices::new(
            vec![
                "nikWRyfT1721CNTRL".into(),
                "Phobos.tail556f5b.ts.net".into(),
            ],
            whois.clone(),
            clock.clone(),
        );
        let phone = headers(&["100.98.28.5"]);
        assert_eq!(
            d.device_of(&phone, "bdelanghe@github")
                .await
                .unwrap()
                .stable_id,
            "nikWRyfT1721CNTRL"
        );
        // Another login on the listed node is not the owner.
        assert!(d.device_of(&phone, "alex@github").await.is_err());
        // orthrus is the owner's login, but not listed here.
        let e = d
            .device_of(&headers(&["100.114.177.2"]), "bdelanghe@github")
            .await
            .unwrap_err();
        assert!(e.contains("not one of the owner's devices"), "{e}");
        // An address whois has no node for, and no address at all.
        assert!(
            d.device_of(&headers(&["100.64.0.9"]), "bdelanghe@github")
                .await
                .is_err()
        );
        assert!(
            d.device_of(&headers(&[]), "bdelanghe@github")
                .await
                .is_err()
        );
        // The phone was asked once, then kept; a minute on, asked again.
        let asked = whois.1.load(Ordering::SeqCst);
        d.device_of(&phone, "bdelanghe@github").await.unwrap();
        assert_eq!(whois.1.load(Ordering::SeqCst), asked);
        *clock.0.lock().unwrap() = start + WHOIS_TTL;
        d.device_of(&phone, "bdelanghe@github").await.unwrap();
        assert_eq!(whois.1.load(Ordering::SeqCst), asked + 1);
    }

    #[test]
    fn a_node_is_listed_by_stable_id_or_magicdns_name() {
        let d = OwnerDevices::new(
            vec![
                "nCd9ijJZF611CNTRL".into(),
                "IPHONE171.tail556f5b.ts.net.".into(),
            ],
            Arc::new(Fixed(HashMap::new(), AtomicUsize::new(0))),
            Arc::new(clax_core::working::SystemClock),
        );
        assert!(d.lists(&parse_whois(PHONE.as_bytes()).unwrap()));
        assert!(d.lists(&parse_whois(ORTHRUS.as_bytes()).unwrap()));
        let mut other = parse_whois(PHONE.as_bytes()).unwrap();
        other.stable_id = "nOther".into();
        other.name = "iphone171.tail556f5b.ts.net.evil.".into();
        assert!(!d.lists(&other));
    }
}
