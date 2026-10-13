//! Remote mode (spec §16, M26): this machine connects to another machine's
//! Clax daemon over the LAN as a person's machine, holding a GNAP grant.
//!
//! `clax connect` writes `<home>/remote.json` ([`RemoteConfig`]) and
//! `<home>/remote.key` (the Ed25519 key, PKCS#8), both 0600. While
//! `remote.json` exists and `CLAX_REMOTE` is not `0` ([`Mode::of`]), the
//! CLI, the hooks and the MCP shim talk to that daemon and never start one
//! here; every request is signed with `clax-ed25519 v2` ([`Signer`],
//! spec §6.6). [`Retry`] is the clients' shared retry rule: once on
//! `stale_request` with the clock offset the daemon's `Date` gives, and on
//! `limit_reached` and `busy` with backoff, re-signing every attempt.
//!
//! Each agent session proves itself with a secret of its own (spec
//! 2026-10-08 §6.8): the shim keeps it in memory, and [`SessionFiles`]
//! keeps it on disk for the hooks and the CLI, which run as new processes.
//!
//! Tokens are [`Secret`]s and the key is a [`KeyPair`]: neither prints in
//! `Debug`. Nothing here logs.

use crate::Home;
use crate::secret::Secret;
use crate::sig::{self, AgentHeaderValues, KeyPair, SigError, SignInput};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

/// The environment variable that turns remote mode off: `0` makes this
/// home's clients use (and start) a local daemon though `remote.json`
/// exists.
pub const REMOTE_ENV: &str = "CLAX_REMOTE";

/// The version of `remote.json`'s format this build writes and reads.
pub const FILE_VERSION: u32 = 1;

/// The backoff before each retry of `limit_reached` and `busy`, in
/// seconds, before jitter: three retries.
pub const BACKOFF_S: [u64; 3] = [1, 2, 4];

/// The largest clock offset a [`Signer`] learns from a daemon's `Date`, in
/// seconds. A `Date` further off is refused, and nothing is signed for that
/// time: an on-path peer that forged it could keep the request it made the
/// client sign and replay it then (§21 items 4 and 6).
pub const MAX_OFFSET_S: i64 = 600;

/// The most bytes a client reads of one answer of a remote daemon: above
/// the largest publish (96 MiB), far below what would fill memory.
pub const MAX_RESPONSE_BYTES: u64 = 128 << 20;

/// Why remote mode cannot be used though `remote.json` exists. Every
/// command fails with it; nothing falls back to a local daemon (spec §22).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// `remote.key` is missing, unreadable or not a key.
    #[error(
        "remote_key_unreadable: {0}: reconnect with `clax connect --off`, then `clax connect` with a new invite"
    )]
    KeyUnreadable(String),
    /// `remote.json` cannot be read or parsed.
    #[error(
        "remote_json_unreadable: {0}: reconnect with `clax connect --off`, then `clax connect` with a new invite"
    )]
    JsonUnreadable(String),
}

impl RemoteError {
    /// The error's code.
    pub fn code(&self) -> &'static str {
        match self {
            RemoteError::KeyUnreadable(_) => "remote_key_unreadable",
            RemoteError::JsonUnreadable(_) => "remote_json_unreadable",
        }
    }
}

/// A daemon's address as `clax connect` takes it: `http://<IP>:<port>`,
/// optionally with an invite's `/i/<code>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonUrl {
    /// `http://<IP literal>[:<port>]`, no trailing slash.
    pub base: String,
    /// The `Host` requests name: the URL's authority.
    pub host: String,
    /// The invite code of an invite URL.
    pub invite: Option<String>,
}

impl DaemonUrl {
    /// Parses `raw`: an `http` URL whose host is an IP literal (the
    /// daemon's host rule accepts only those from the LAN), with no path
    /// but `/` or `/i/<code>`, no query, fragment or credentials.
    ///
    /// # Errors
    /// A sentence saying what is wrong.
    pub fn parse(raw: &str) -> Result<DaemonUrl, String> {
        let u = url::Url::parse(raw.trim())
            .map_err(|e| format!("'{raw}' is not a URL ({e}): give http://<IP>:<port>"))?;
        if u.scheme() != "http" {
            return Err(format!(
                "'{raw}' is not an http:// URL: Clax daemons serve http://<IP>:<port>"
            ));
        }
        if !u.username().is_empty() || u.password().is_some() {
            return Err(format!(
                "'{raw}' carries credentials: give http://<IP>:<port>"
            ));
        }
        if u.query().is_some() || u.fragment().is_some() {
            return Err(format!(
                "'{raw}' has a query or fragment: give http://<IP>:<port>"
            ));
        }
        let host = match u.host() {
            Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip).to_string(),
            Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
            Some(url::Host::Domain(d)) => {
                return Err(format!(
                    "'{d}' is a host name: name the daemon by its IP address, as the invite shows (http://<IP>:<port>)"
                ));
            }
            None => return Err(format!("'{raw}' names no host")),
        };
        let authority = match u.port() {
            Some(p) => format!("{host}:{p}"),
            None => host,
        };
        let invite = match u.path() {
            "" | "/" => None,
            p => match p.strip_prefix("/i/") {
                Some(code) if !code.is_empty() && !code.contains('/') => Some(code.to_string()),
                _ => {
                    return Err(format!(
                        "'{raw}' is neither a daemon's address (http://<IP>:<port>) nor an invite (…/i/<code>)"
                    ));
                }
            },
        };
        Ok(DaemonUrl {
            base: format!("http://{authority}"),
            host: authority,
            invite,
        })
    }

    /// Whether `base`'s host is an IP literal (what `clax doctor` checks of
    /// `remote.json`).
    pub fn is_ip_literal(base: &str) -> bool {
        DaemonUrl::parse(base).is_ok()
    }
}

/// `remote.json`: the daemon this machine is connected to and its grant.
#[derive(Clone)]
pub struct RemoteConfig {
    /// `http://<IP>:<port>`.
    pub url: String,
    pub grant_id: String,
    /// `cxg_…`: `Authorization: Clax-Grant`.
    pub access_token: Secret,
    /// `cxm_…`: `DELETE /api/grants/<gid>/token`.
    pub manage_token: Secret,
    /// `cxc_…`: `DELETE /api/grants/<gid>/continue` (`clax connect --off`).
    pub continue_token: Secret,
    /// The person this machine acts for.
    pub person_name: Option<String>,
    pub person_id: Option<String>,
    /// The owner of the daemon, as it named itself.
    pub owner_name: Option<String>,
    /// A machine of the owner's own (an invite for the owner row).
    pub own_machine: bool,
    /// The name the grant request gave this machine.
    pub client_name: String,
    /// RFC 3339.
    pub connected_at: String,
}

impl std::fmt::Debug for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConfig")
            .field("url", &self.url)
            .field("grant_id", &self.grant_id)
            .field("person_name", &self.person_name)
            .field("client_name", &self.client_name)
            .finish_non_exhaustive()
    }
}

/// `remote.json` as stored.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    url: String,
    grant_id: String,
    access_token: String,
    manage_token: String,
    continue_token: String,
    #[serde(default)]
    person: Option<StoredPerson>,
    #[serde(default)]
    owner_name: Option<String>,
    #[serde(default)]
    own_machine: bool,
    #[serde(default)]
    client_name: String,
    #[serde(default)]
    connected_at: String,
}

#[derive(Serialize, Deserialize)]
struct StoredPerson {
    #[serde(default)]
    public_id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

impl RemoteConfig {
    /// Reads `<home>/remote.json`; `None` when it does not exist.
    ///
    /// # Errors
    /// [`RemoteError::JsonUnreadable`] when it exists but cannot be read or
    /// parsed, or names a URL that is not `http://<IP>:<port>`.
    pub fn load(home: &Home) -> Result<Option<RemoteConfig>, RemoteError> {
        let path = home.remote_json();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(RemoteError::JsonUnreadable(format!(
                    "{}: {e}",
                    path.display()
                )));
            }
        };
        let s: Stored = serde_json::from_slice(&bytes)
            .map_err(|e| RemoteError::JsonUnreadable(format!("{}: {e}", path.display())))?;
        if s.version != FILE_VERSION {
            return Err(RemoteError::JsonUnreadable(format!(
                "{}: version {} (this build reads {FILE_VERSION})",
                path.display(),
                s.version
            )));
        }
        let url = DaemonUrl::parse(&s.url)
            .map_err(|e| RemoteError::JsonUnreadable(format!("{}: {e}", path.display())))?;
        let person = s.person.unwrap_or(StoredPerson {
            public_id: None,
            display_name: None,
        });
        Ok(Some(RemoteConfig {
            url: url.base,
            grant_id: s.grant_id,
            access_token: Secret::new(s.access_token),
            manage_token: Secret::new(s.manage_token),
            continue_token: Secret::new(s.continue_token),
            person_name: person.display_name,
            person_id: person.public_id,
            owner_name: s.owner_name,
            own_machine: s.own_machine,
            client_name: s.client_name,
            connected_at: s.connected_at,
        }))
    }

    /// Writes `<home>/remote.json`, 0600, replacing it whole.
    ///
    /// # Errors
    /// The I/O error.
    pub fn save(&self, home: &Home) -> std::io::Result<()> {
        let s = Stored {
            version: FILE_VERSION,
            url: self.url.clone(),
            grant_id: self.grant_id.clone(),
            access_token: self.access_token.expose().to_string(),
            manage_token: self.manage_token.expose().to_string(),
            continue_token: self.continue_token.expose().to_string(),
            person: Some(StoredPerson {
                public_id: self.person_id.clone(),
                display_name: self.person_name.clone(),
            }),
            owner_name: self.owner_name.clone(),
            own_machine: self.own_machine,
            client_name: self.client_name.clone(),
            connected_at: self.connected_at.clone(),
        };
        let mut bytes = serde_json::to_vec_pretty(&s).expect("remote.json serializes");
        bytes.push(b'\n');
        write_private(&home.remote_json(), &bytes)
    }

    /// The `Host` its requests name.
    pub fn host(&self) -> String {
        self.url.trim_start_matches("http://").to_string()
    }
}

/// An ad-hoc `clax connect` waiting for the owner (`remote-pending.json`).
#[derive(Clone)]
pub struct PendingConnect {
    pub url: String,
    pub grant_id: String,
    pub continue_token: Secret,
    pub user_code: String,
    pub client_name: String,
    /// RFC 3339: when the request was made.
    pub requested_at: String,
}

impl std::fmt::Debug for PendingConnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingConnect")
            .field("url", &self.url)
            .field("grant_id", &self.grant_id)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct StoredPending {
    version: u32,
    url: String,
    grant_id: String,
    continue_token: String,
    user_code: String,
    #[serde(default)]
    client_name: String,
    requested_at: String,
}

impl PendingConnect {
    /// Reads `<home>/remote-pending.json`; `None` when it is missing or
    /// cannot be read (a request it named lapses on the daemon by itself).
    pub fn load(home: &Home) -> Option<PendingConnect> {
        let bytes = std::fs::read(home.remote_pending()).ok()?;
        let s: StoredPending = serde_json::from_slice(&bytes).ok()?;
        (s.version == FILE_VERSION).then_some(PendingConnect {
            url: s.url,
            grant_id: s.grant_id,
            continue_token: Secret::new(s.continue_token),
            user_code: s.user_code,
            client_name: s.client_name,
            requested_at: s.requested_at,
        })
    }

    /// Writes `<home>/remote-pending.json`, 0600.
    ///
    /// # Errors
    /// The I/O error.
    pub fn save(&self, home: &Home) -> std::io::Result<()> {
        let s = StoredPending {
            version: FILE_VERSION,
            url: self.url.clone(),
            grant_id: self.grant_id.clone(),
            continue_token: self.continue_token.expose().to_string(),
            user_code: self.user_code.clone(),
            client_name: self.client_name.clone(),
            requested_at: self.requested_at.clone(),
        };
        let mut bytes = serde_json::to_vec_pretty(&s).expect("pending serializes");
        bytes.push(b'\n');
        write_private(&home.remote_pending(), &bytes)
    }
}

/// Writes `bytes` to `path` with mode 0600: to a new file beside it, made
/// 0600 before any byte is written, then renamed over `path`.
///
/// # Errors
/// The I/O error; the temporary file is removed.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", ulid::Ulid::new()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Reads the key in `<home>/remote.key`.
///
/// # Errors
/// [`RemoteError::KeyUnreadable`], naming the file and why.
pub fn load_key(home: &Home) -> Result<KeyPair, RemoteError> {
    let path = home.remote_key();
    let bytes = std::fs::read(&path)
        .map_err(|e| RemoteError::KeyUnreadable(format!("{}: {e}", path.display())))?;
    KeyPair::from_pkcs8(&bytes).map_err(|_| {
        RemoteError::KeyUnreadable(format!("{} is not an Ed25519 key", path.display()))
    })
}

/// Writes `key` to `<home>/remote.key`, 0600.
///
/// # Errors
/// The I/O error.
pub fn save_key(home: &Home, key: &KeyPair) -> std::io::Result<()> {
    write_private(&home.remote_key(), key.pkcs8().expose())
}

/// Seconds since the Unix epoch, as a [`Signer`] reads its clock.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The system clock.
pub fn system_clock() -> Clock {
    Arc::new(|| chrono::Utc::now().timestamp())
}

/// Who a signed request authenticates as.
#[derive(Clone, Copy, Debug)]
pub enum Credential<'a> {
    /// No token: the grant request (`POST /api/grants`).
    None,
    /// `Authorization: GNAP <token>`: continuation and management.
    Gnap(&'a Secret),
    /// `Authorization: Clax-Grant <token>`: every other request.
    Grant(&'a Secret),
}

impl Credential<'_> {
    /// The `Authorization` header's value, when there is one.
    pub fn authorization(&self) -> Option<String> {
        match self {
            Credential::None => None,
            Credential::Gnap(t) => Some(format!("GNAP {}", t.expose())),
            Credential::Grant(t) => Some(format!("Clax-Grant {}", t.expose())),
        }
    }

    fn token(&self) -> Option<&str> {
        match self {
            Credential::None => None,
            Credential::Gnap(t) | Credential::Grant(t) => Some(t.expose()),
        }
    }
}

/// Signs requests to one daemon with one key (spec §6.6), keeping the
/// offset between this machine's clock and the daemon's that a
/// `stale_request` taught it.
pub struct Signer {
    key: KeyPair,
    base: String,
    host: String,
    clock: Clock,
    offset: AtomicI64,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("key", &self.key)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Signer {
    /// A signer for the daemon at `url` (`http://<IP>:<port>`) with `key`,
    /// reading the system clock.
    ///
    /// # Errors
    /// As [`DaemonUrl::parse`].
    pub fn new(key: KeyPair, url: &str) -> Result<Signer, String> {
        let u = DaemonUrl::parse(url)?;
        Ok(Signer {
            key,
            base: u.base,
            host: u.host,
            clock: system_clock(),
            offset: AtomicI64::new(0),
        })
    }

    /// This signer reading `clock`.
    pub fn with_clock(mut self, clock: Clock) -> Signer {
        self.clock = clock;
        self
    }

    /// `http://<IP>:<port>`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The `Host` every request names (and signs).
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn key(&self) -> &KeyPair {
        &self.key
    }

    /// The key's fingerprint.
    pub fn fingerprint(&self) -> String {
        sig::fingerprint(&self.key.public())
    }

    /// Seconds the daemon's clock is ahead of this machine's, as last
    /// learned ([`Signer::learn_offset`]); 0 until then.
    pub fn offset(&self) -> i64 {
        self.offset.load(Ordering::Relaxed)
    }

    /// The time a request is signed at: this machine's clock plus the
    /// offset.
    pub fn now(&self) -> i64 {
        (self.clock)() + self.offset()
    }

    /// Learns the offset from a response's `Date` header (an HTTP date):
    /// the daemon's time less this machine's, when it is at most
    /// [`MAX_OFFSET_S`].
    ///
    /// # Errors
    /// [`Learned::Unreadable`] when `date` does not parse;
    /// [`Learned::TooFar`] with the offset when it is beyond
    /// [`MAX_OFFSET_S`], which is then not learned.
    pub fn learn_offset(&self, date: &str) -> Result<i64, Learned> {
        let o = daemon_offset(date, (self.clock)()).ok_or(Learned::Unreadable)?;
        if o.abs() > MAX_OFFSET_S {
            return Err(Learned::TooFar(o));
        }
        self.offset.store(o, Ordering::Relaxed);
        Ok(o)
    }

    /// The `Clax-Signature` value for a request: `method`, `path_and_query`
    /// as sent, this signer's host, now, a fresh nonce, `body`, the token
    /// `cred` carries and `agent` (the [`sig::AGENT_HEADERS`] values the
    /// request sends, which the caller sends trimmed by
    /// [`sig::trim_header_value`], as they are signed here).
    ///
    /// # Errors
    /// As [`KeyPair::sign`].
    pub fn sign(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        cred: Credential<'_>,
        agent: &AgentHeaderValues<'_>,
    ) -> Result<String, SigError> {
        let nonce = sig::new_nonce();
        let input = SignInput {
            method,
            path_and_query,
            host: &self.host,
            t: self.now(),
            nonce: &nonce,
            body_sha256: Sha256::digest(body).into(),
            token_sha256: cred.token().map(crate::secret::sha256),
            agent_headers_sha256: sig::agent_headers_sha256(&sig::signable_agent_headers(agent)),
        };
        self.key.sign(&input).map(|h| h.to_string())
    }
}

/// Why [`Signer::learn_offset`] learned nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Learned {
    /// The `Date` does not parse.
    Unreadable,
    /// The `Date` is this many seconds off, beyond [`MAX_OFFSET_S`].
    TooFar(i64),
}

/// The refusal of a daemon whose clock is `offset` seconds from this
/// machine's: `clock_skew: …`, telling the person to set the clock.
pub fn clock_skew_message(offset: i64) -> String {
    format!(
        "the daemon's clock is {offset:+} s from this machine's, more than the {MAX_OFFSET_S} s a signed request may be moved by: set this machine's clock (nothing was signed for that time)"
    )
}

/// The path and query of `url` as a request sends them: what a signature
/// covers.
pub fn path_and_query(url: &url::Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    }
}

/// Readies `headers` of a request to the daemon `signer` signs for, as
/// `cred`: any `Authorization` is replaced by `cred`'s, `Host` names the
/// signer's host, and each of [`sig::AGENT_HEADERS`] is trimmed of
/// surrounding spaces and tabs ([`sig::trim_header_value`]), so what is
/// sent is what [`sign_headers`] signs.
///
/// # Errors
/// A value that cannot be a header.
pub fn prepare_headers(
    signer: &Signer,
    headers: &mut http::HeaderMap,
    cred: Credential<'_>,
) -> Result<(), String> {
    let value = |v: &str| {
        http::HeaderValue::from_str(v).map_err(|_| "a header that cannot be sent".to_string())
    };
    headers.remove(http::header::AUTHORIZATION);
    headers.insert(http::header::HOST, value(signer.host())?);
    for name in sig::AGENT_HEADERS {
        let trimmed: Vec<http::HeaderValue> = headers
            .get_all(name)
            .iter()
            .map(|v| http::HeaderValue::from_bytes(sig::trim_header_value(v.as_bytes())))
            .collect::<Result<_, _>>()
            .map_err(|_| "a header that cannot be sent".to_string())?;
        if !trimmed.is_empty() {
            headers.remove(name);
            for v in trimmed {
                headers.append(name, v);
            }
        }
    }
    if let Some(a) = cred.authorization() {
        headers.insert(http::header::AUTHORIZATION, value(&a)?);
    }
    Ok(())
}

/// Signs one attempt of a request [`prepare_headers`] readied: sets
/// `Clax-Signature` over `method`, `path_and_query`, `body`, `cred`'s token
/// and the agent headers `headers` carries, signed now.
///
/// # Errors
/// An agent header given twice, or an input [`KeyPair::sign`] refuses.
pub fn sign_headers(
    signer: &Signer,
    method: &str,
    path_and_query: &str,
    body: &[u8],
    cred: Credential<'_>,
    headers: &mut http::HeaderMap,
) -> Result<(), String> {
    let signature = {
        let values =
            sig::agent_header_values(headers.iter().map(|(k, v)| (k.as_str(), v.as_bytes())))
                .map_err(|_| "an agent header appears more than once".to_string())?;
        signer
            .sign(method, path_and_query, body, cred, &values)
            .map_err(|e| format!("signing the request: {e}"))?
    };
    headers.insert(
        "clax-signature",
        http::HeaderValue::from_str(&signature).map_err(|e| e.to_string())?,
    );
    Ok(())
}

/// The message of a refusal a client gave up on: the daemon's, but for
/// `grant_revoked`, which says the machine was disconnected from the
/// signer's daemon and what to do.
pub fn refusal_message(code: &str, message: &str, signer: &Signer) -> String {
    if code == "grant_revoked" {
        format!(
            "this machine was disconnected from {}: run `clax connect --off`, then connect again with a new invite",
            signer.base()
        )
    } else {
        message.to_string()
    }
}

/// The daemon's time in HTTP date `date` less `local` (Unix seconds).
pub fn daemon_offset(date: &str, local: i64) -> Option<i64> {
    let t = chrono::DateTime::parse_from_rfc2822(date.trim()).ok()?;
    Some(t.timestamp() - local)
}

/// The header that carries a session's secret on a request naming that
/// session (spec 2026-10-08 §6.8).
pub const SESSION_SECRET_HEADER: &str = "x-clax-session-secret";

/// One agent session's credential on the remote daemon: its ID and its
/// secret (spec 2026-10-08 §6.8). Its `Debug` never prints the secret.
#[derive(Clone, Debug)]
pub struct SessionCredential {
    pub session: String,
    pub secret: Secret,
}

impl SessionCredential {
    /// Whether `other` is the same session with the same secret.
    pub fn same_as(&self, other: &SessionCredential) -> bool {
        self.session == other.session
            && crate::secret::constant_time_eq(
                self.secret.expose().as_bytes(),
                other.secret.expose().as_bytes(),
            )
    }
}

/// What a session credential's file holds: the credential, the harness
/// session ID its row had when written (none for a row known by its parent
/// PID alone), and, in a parent-PID file, when that process started, so a
/// later process given the same PID never takes the file as its own.
#[derive(Serialize, Deserialize)]
struct CredentialFile {
    session: String,
    secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    harness_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_started: Option<String>,
}

/// A name a session's credential file is found by (spec 2026-10-08 §6.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKey<'a> {
    /// The harness's own session ID.
    Harness { harness: &'a str, id: &'a str },
    /// The harness process, for a client that knows it but not its session
    /// ID (Codex's shim).
    Parent { harness: &'a str, pid: u32 },
}

impl SessionKey<'_> {
    /// The file name, which no two keys share: `<harness>-s-<harness
    /// session ID>.json` for an ID of 1–128 characters of `[A-Za-z0-9._-]`,
    /// else `<harness>-h-<the first 32 hex characters of its
    /// SHA-256>.json`; `<harness>-p-<pid>.json` for a parent PID. `None`
    /// for a harness name that is not a plain lowercase word.
    pub fn file_name(&self) -> Option<String> {
        let plain =
            |h: &str| !h.is_empty() && h.len() <= 32 && h.bytes().all(|b| b.is_ascii_lowercase());
        match *self {
            SessionKey::Harness { harness, id } if plain(harness) && !id.is_empty() => {
                let safe = id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
                Some(if safe {
                    format!("{harness}-s-{id}.json")
                } else {
                    let h = crate::secret::sha256_hex(id);
                    format!("{harness}-h-{}.json", &h[..32])
                })
            }
            SessionKey::Parent { harness, pid } if plain(harness) && pid > 1 => {
                Some(format!("{harness}-p-{pid}.json"))
            }
            _ => None,
        }
    }
}

/// When process `pid` started, as `ps` reports it; `None` when it cannot be
/// told (no such process, or no `ps`). A parent-PID credential file
/// records it, and is taken only while the PID still names that process.
pub fn process_started(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Why a session's credential file could not be used.
#[derive(Debug)]
pub struct CredentialUnreadable {
    pub path: PathBuf,
    pub why: String,
}

impl std::fmt::Display for CredentialUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session_credential_unreadable: {}: {}",
            self.path.display(),
            self.why
        )
    }
}

impl std::error::Error for CredentialUnreadable {}

/// Who looks for a session's credential (spec 2026-10-08 §6.8): its harness,
/// the harness session ID it knows, and the process IDs join or
/// registration would match a row by, nearest first (a registration's
/// parent; a hook's parent, then its ancestors).
#[derive(Clone, Copy, Debug)]
pub struct Seeker<'a> {
    pub harness: &'a str,
    pub harness_session_id: Option<&'a str>,
    pub pids: &'a [u32],
}

/// The agent sessions' credential files of a connected machine,
/// `<home>/remote-sessions/` (spec 2026-10-08 §6.8): the directory 0700,
/// each file 0600, written whole and renamed into place
/// ([`write_private`]). Neither the directory nor a file is followed
/// through a symbolic link.
#[derive(Clone, Debug)]
pub struct SessionFiles {
    dir: PathBuf,
}

impl SessionFiles {
    /// The files of `home`.
    pub fn of(home: &Home) -> SessionFiles {
        SessionFiles {
            dir: home.remote_sessions(),
        }
    }

    /// The files in `dir`.
    pub fn at(dir: PathBuf) -> SessionFiles {
        SessionFiles { dir }
    }

    /// The directory the files are in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of `key`; `None` for a key with no file name.
    pub fn path(&self, key: &SessionKey<'_>) -> Option<PathBuf> {
        key.file_name().map(|n| self.dir.join(n))
    }

    /// The file of `key` as written, `None` when there is no such file.
    fn read_file(
        &self,
        key: &SessionKey<'_>,
    ) -> Result<Option<(PathBuf, CredentialFile)>, CredentialUnreadable> {
        let Some(path) = self.path(key) else {
            return Ok(None);
        };
        let unreadable = |why: String| CredentialUnreadable {
            path: path.clone(),
            why,
        };
        for p in [self.dir.as_path(), path.as_path()] {
            match std::fs::symlink_metadata(p) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(unreadable(format!(
                        "{} is a symbolic link, which is never followed",
                        p.display()
                    )));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(unreadable(e.to_string())),
            }
        }
        let bytes = std::fs::read(&path).map_err(|e| unreadable(e.to_string()))?;
        let f: CredentialFile = serde_json::from_slice(&bytes)
            .map_err(|_| unreadable("not a session credential".into()))?;
        if f.session.is_empty() || !f.secret.starts_with(crate::secret::SESSION_PREFIX) {
            return Err(unreadable("not a session credential".into()));
        }
        Ok(Some((path, f)))
    }

    /// The credential in `key`'s file, whatever row it was written for:
    /// `None` when there is no such file.
    ///
    /// # Errors
    /// [`CredentialUnreadable`], naming the file, when it exists but
    /// cannot be read or is not a credential.
    pub fn read(
        &self,
        key: &SessionKey<'_>,
    ) -> Result<Option<SessionCredential>, CredentialUnreadable> {
        Ok(self.read_file(key)?.map(|(_, f)| SessionCredential {
            session: f.session,
            secret: Secret::new(f.secret),
        }))
    }

    /// The credential `seeker` may use, found as registration and join
    /// match a row (spec 2026-10-08 §6.7, §6.8): the file of its harness
    /// session ID; else, for each of its PIDs in order, a parent-PID file
    /// written while that PID named the process it names now, for a row
    /// that join or registration would match: one with no harness session
    /// ID, the seeker's own, or any when the seeker knows none. `None` when
    /// no file qualifies.
    ///
    /// # Errors
    /// [`CredentialUnreadable`] for the first file that exists but cannot
    /// be used.
    pub fn find(
        &self,
        seeker: &Seeker<'_>,
    ) -> Result<Option<SessionCredential>, CredentialUnreadable> {
        if let Some(id) = seeker.harness_session_id
            && let Some(c) = self.read(&SessionKey::Harness {
                harness: seeker.harness,
                id,
            })?
        {
            return Ok(Some(c));
        }
        for &pid in seeker.pids {
            let key = SessionKey::Parent {
                harness: seeker.harness,
                pid,
            };
            let Some((_, f)) = self.read_file(&key)? else {
                continue;
            };
            let row_matches = match (seeker.harness_session_id, f.harness_session_id.as_deref()) {
                (None, _) | (_, None) => true,
                (Some(mine), Some(row)) => mine == row,
            };
            let same_process =
                f.parent_started.is_some() && f.parent_started == process_started(pid);
            if row_matches && same_process {
                return Ok(Some(SessionCredential {
                    session: f.session,
                    secret: Secret::new(f.secret),
                }));
            }
        }
        Ok(None)
    }

    /// Writes `cred`, the credential of a `harness` row with harness session
    /// ID `row_hsid` and parent PID `parent_pid`, under every name it is
    /// found by: the row's harness session ID and `also` (a hook's own,
    /// which may differ when the row has none yet), and the parent PID
    /// while that process can be told apart from a later one with its PID.
    /// The directory is made 0700, or set so when it exists.
    ///
    /// # Errors
    /// The first I/O error; a directory that is a symbolic link.
    pub fn write(
        &self,
        harness: &str,
        row_hsid: Option<&str>,
        parent_pid: Option<u32>,
        also: Option<&str>,
        cred: &SessionCredential,
    ) -> std::io::Result<()> {
        self.private_dir()?;
        let file = |parent_started: Option<String>| {
            let mut bytes = serde_json::to_vec(&CredentialFile {
                session: cred.session.clone(),
                secret: cred.secret.expose().to_string(),
                harness_session_id: row_hsid.map(str::to_string),
                parent_started,
            })
            .expect("a credential serializes");
            bytes.push(b'\n');
            bytes
        };
        let mut ids: Vec<&str> = row_hsid.into_iter().chain(also).collect();
        ids.dedup();
        for id in ids.into_iter().filter(|i| !i.is_empty()) {
            if let Some(p) = self.path(&SessionKey::Harness { harness, id }) {
                write_private(&p, &file(None))?;
            }
        }
        if let Some(pid) = parent_pid
            && let Some(started) = process_started(pid)
            && let Some(p) = self.path(&SessionKey::Parent { harness, pid })
        {
            write_private(&p, &file(Some(started)))?;
        }
        Ok(())
    }

    /// The directory, made 0700 or set so; refused when it is a symbolic
    /// link.
    fn private_dir(&self) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match std::fs::symlink_metadata(&self.dir) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                return Err(std::io::Error::other(format!(
                    "{} is not a directory of its own",
                    self.dir.display()
                )));
            }
            Ok(m) if m.permissions().mode() & 0o777 != 0o700 => {
                std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&self.dir)?;
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Removes every one of `keys`' files that holds session `session`,
    /// leaving another session's file alone.
    pub fn remove(&self, keys: &[SessionKey<'_>], session: &str) {
        for k in keys {
            let Some(path) = self.path(k) else { continue };
            if let Ok(Some(c)) = self.read(k)
                && c.session == session
            {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// The names a session row is found by: its harness session ID, when it
/// has one, and its parent PID, when it has one.
pub fn keys_of_row<'a>(
    harness: &'a str,
    harness_session_id: Option<&'a str>,
    parent_pid: Option<u32>,
) -> Vec<SessionKey<'a>> {
    let mut keys = Vec::new();
    if let Some(id) = harness_session_id.filter(|s| !s.is_empty()) {
        keys.push(SessionKey::Harness { harness, id });
    }
    if let Some(pid) = parent_pid {
        keys.push(SessionKey::Parent { harness, pid });
    }
    keys
}

/// A connected machine: `remote.json`, the signer of `remote.key`, and its
/// agent sessions' credential files.
#[derive(Debug)]
pub struct Remote {
    pub config: RemoteConfig,
    pub signer: Arc<Signer>,
    pub sessions: SessionFiles,
}

impl Remote {
    /// The remote this home is connected to; `None` without `remote.json`.
    ///
    /// # Errors
    /// [`RemoteError`] when `remote.json` or `remote.key` cannot be used.
    pub fn load(home: &Home) -> Result<Option<Remote>, RemoteError> {
        let Some(config) = RemoteConfig::load(home)? else {
            return Ok(None);
        };
        let key = load_key(home)?;
        let signer = Signer::new(key, &config.url).map_err(RemoteError::JsonUnreadable)?;
        Ok(Some(Remote {
            config,
            signer: Arc::new(signer),
            sessions: SessionFiles::of(home),
        }))
    }

    /// The access token's credential.
    pub fn grant(&self) -> Credential<'_> {
        Credential::Grant(&self.config.access_token)
    }

    /// The URL people open for `path` on the remote daemon.
    pub fn browser_url(&self, path: &str) -> String {
        format!("{}{path}", self.config.url)
    }
}

/// Which daemon this home's clients use.
#[derive(Debug, Clone)]
pub enum Mode {
    /// No `remote.json`, or `CLAX_REMOTE=0`: the local daemon, started when
    /// needed.
    Local,
    /// `remote.json` and its key: the remote daemon, signed; nothing starts
    /// a local one.
    Remote(Arc<Remote>),
    /// `remote.json` exists but cannot be used: every command fails with
    /// this, and nothing falls back to a local daemon.
    Broken(RemoteError),
}

impl Mode {
    /// The mode of `home` under this process's `CLAX_REMOTE`.
    pub fn of(home: &Home) -> Mode {
        Mode::with_env(home, std::env::var(REMOTE_ENV).ok().as_deref())
    }

    /// [`Mode::of`] with `CLAX_REMOTE`'s value given.
    pub fn with_env(home: &Home, env: Option<&str>) -> Mode {
        if env.map(str::trim) == Some("0") {
            return Mode::Local;
        }
        match Remote::load(home) {
            Ok(None) => Mode::Local,
            Ok(Some(r)) => Mode::Remote(Arc::new(r)),
            Err(e) => Mode::Broken(e),
        }
    }

    /// Whether this home is in remote mode, usable or not.
    pub fn is_remote(&self) -> bool {
        !matches!(self, Mode::Local)
    }
}

/// What a client does after a signed request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Give up: the refusal is the answer.
    GiveUp,
    /// Sign again and send at once (`stale_request`: the offset was learned).
    Resign,
    /// Wait this long, then sign again and send.
    After(Duration),
    /// The daemon's `Date` is this many seconds off, beyond
    /// [`MAX_OFFSET_S`]: give up with [`clock_skew_message`].
    ClockSkew(i64),
}

/// The retry rule every signing client follows (spec §6.6): one retry on
/// `stale_request`, with the offset the daemon's `Date` gives; three on
/// `limit_reached` (429) and `busy` (503), after [`BACKOFF_S`] seconds
/// each with up to a quarter more of jitter. Every retry is signed anew.
#[derive(Debug, Default)]
pub struct Retry {
    stale_retried: bool,
    backoffs: usize,
}

impl Retry {
    /// What to do after a refusal with `status` and error `code`, the
    /// response's `Date` header in `date`; `jitter` in `[0, 1)` scales the
    /// added quarter.
    pub fn next(
        &mut self,
        status: u16,
        code: &str,
        date: Option<&str>,
        signer: &Signer,
        jitter: f64,
    ) -> Next {
        match (status, code) {
            (401, "stale_request") if !self.stale_retried => {
                self.stale_retried = true;
                match date.map(|d| signer.learn_offset(d)) {
                    Some(Ok(_)) => Next::Resign,
                    Some(Err(Learned::TooFar(o))) => Next::ClockSkew(o),
                    _ => Next::GiveUp,
                }
            }
            (429, "limit_reached") | (503, "busy") if self.backoffs < BACKOFF_S.len() => {
                let base = Duration::from_secs(BACKOFF_S[self.backoffs]);
                self.backoffs += 1;
                Next::After(base + base.mul_f64(jitter.clamp(0.0, 1.0) / 4.0))
            }
            _ => Next::GiveUp,
        }
    }
}

/// A jitter value in `[0, 1)`.
pub fn jitter() -> f64 {
    rand::random::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_urls_are_ip_literals_with_an_optional_invite() {
        let u = DaemonUrl::parse("http://192.168.1.5:7480/i/K7QF-M2XD-9PLA").unwrap();
        assert_eq!(u.base, "http://192.168.1.5:7480");
        assert_eq!(u.host, "192.168.1.5:7480");
        assert_eq!(u.invite.as_deref(), Some("K7QF-M2XD-9PLA"));
        let u = DaemonUrl::parse("http://192.168.1.5:7480/").unwrap();
        assert_eq!(
            (u.base.as_str(), u.invite),
            ("http://192.168.1.5:7480", None)
        );
        let u = DaemonUrl::parse("http://[fe80::1]:7480").unwrap();
        assert_eq!(u.host, "[fe80::1]:7480");
        for bad in [
            "https://192.168.1.5:7480",
            "http://alex-mbp.local:7480",
            "http://localhost:7480",
            "http://192.168.1.5:7480/x",
            "http://192.168.1.5:7480/i/",
            "http://192.168.1.5:7480/?a=1",
            "http://u:p@192.168.1.5:7480",
            "192.168.1.5:7480",
        ] {
            assert!(DaemonUrl::parse(bad).is_err(), "{bad}");
        }
        assert!(DaemonUrl::is_ip_literal("http://10.0.0.2:7480"));
        assert!(!DaemonUrl::is_ip_literal("http://box.lan:7480"));
    }

    fn home() -> (tempfile::TempDir, Home) {
        let d = tempfile::tempdir().unwrap();
        let h = Home::at(d.path().join("ax"));
        (d, h)
    }

    fn config() -> RemoteConfig {
        RemoteConfig {
            url: "http://192.168.1.5:7480".into(),
            grant_id: "01J9ZK3M4N5P6Q7R8S9T0V1W2X".into(),
            access_token: Secret::new("cxg_access".into()),
            manage_token: Secret::new("cxm_manage".into()),
            continue_token: Secret::new("cxc_continue".into()),
            person_name: Some("Sam".into()),
            person_id: Some("pub1".into()),
            owner_name: Some("Alex".into()),
            own_machine: false,
            client_name: "sam-mbp".into(),
            connected_at: "2026-10-11T10:00:00Z".into(),
        }
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn remote_json_and_key_round_trip_at_0600() {
        let (_d, home) = home();
        assert!(matches!(Mode::with_env(&home, None), Mode::Local));
        config().save(&home).unwrap();
        let key = KeyPair::generate();
        save_key(&home, &key).unwrap();
        assert_eq!(mode_of(&home.remote_json()), 0o600);
        assert_eq!(mode_of(&home.remote_key()), 0o600);
        let Mode::Remote(r) = Mode::with_env(&home, None) else {
            panic!("remote mode");
        };
        assert_eq!(r.config.url, "http://192.168.1.5:7480");
        assert_eq!(r.config.access_token.expose(), "cxg_access");
        assert_eq!(r.signer.key().public(), key.public());
        assert_eq!(r.signer.host(), "192.168.1.5:7480");
        // Debug prints no token.
        let shown = format!("{r:?}");
        assert!(!shown.contains("cxg_") && !shown.contains("cxm_") && !shown.contains("cxc_"));
        // CLAX_REMOTE=0 turns it off.
        assert!(matches!(Mode::with_env(&home, Some("0")), Mode::Local));
    }

    #[test]
    fn an_unreadable_key_breaks_remote_mode_with_no_fallback() {
        let (_d, home) = home();
        config().save(&home).unwrap();
        let Mode::Broken(e) = Mode::with_env(&home, None) else {
            panic!("broken");
        };
        assert_eq!(e.code(), "remote_key_unreadable");
        std::fs::write(home.remote_key(), b"not a key").unwrap();
        let Mode::Broken(e) = Mode::with_env(&home, None) else {
            panic!("broken");
        };
        assert_eq!(e.code(), "remote_key_unreadable");
        assert!(e.to_string().contains("clax connect"), "{e}");
        std::fs::write(home.remote_json(), b"{").unwrap();
        let Mode::Broken(e) = Mode::with_env(&home, None) else {
            panic!("broken");
        };
        assert_eq!(e.code(), "remote_json_unreadable");
    }

    #[test]
    fn pending_connects_round_trip_at_0600() {
        let (_d, home) = home();
        assert!(PendingConnect::load(&home).is_none());
        PendingConnect {
            url: "http://192.168.1.5:7480".into(),
            grant_id: "g".into(),
            continue_token: Secret::new("cxc_x".into()),
            user_code: "WXYZ-4821".into(),
            client_name: "sam-mbp".into(),
            requested_at: "2026-10-11T10:00:00Z".into(),
        }
        .save(&home)
        .unwrap();
        assert_eq!(mode_of(&home.remote_pending()), 0o600);
        let p = PendingConnect::load(&home).unwrap();
        assert_eq!(
            (p.user_code.as_str(), p.continue_token.expose()),
            ("WXYZ-4821", "cxc_x")
        );
    }

    fn signer_at(now: i64) -> Signer {
        Signer::new(KeyPair::generate(), "http://192.168.1.5:7480")
            .unwrap()
            .with_clock(Arc::new(move || now))
    }

    #[test]
    fn signatures_verify_with_the_header_values_the_daemon_reads() {
        let s = signer_at(1_760_000_000);
        let token = Secret::new("cxg_t".into());
        let sent = [
            None,
            Some(b"agent ".as_slice()),
            Some(b"\tcli".as_slice()),
            None,
            None,
        ];
        let header = s
            .sign(
                "GET",
                "/api/artifacts",
                b"",
                Credential::Grant(&token),
                &sent,
            )
            .unwrap();
        let h = sig::parse_header(&header).unwrap();
        assert_eq!(h.t, 1_760_000_000);
        let received = [
            None,
            Some(b"agent".as_slice()),
            Some(b"cli".as_slice()),
            None,
            None,
        ];
        let input = SignInput {
            method: "GET",
            path_and_query: "/api/artifacts",
            host: "192.168.1.5:7480",
            t: h.t,
            nonce: &h.nonce,
            body_sha256: Sha256::digest(b"").into(),
            token_sha256: Some(crate::secret::sha256("cxg_t")),
            agent_headers_sha256: sig::agent_headers_sha256(&received),
        };
        assert!(sig::verify(&s.key().public(), &input, &h.sig));
    }

    #[test]
    fn stale_requests_retry_once_with_the_dates_offset() {
        let s = signer_at(1_760_000_000);
        let mut r = Retry::default();
        // The daemon is 300 s ahead.
        let date = chrono::DateTime::from_timestamp(1_760_000_300, 0)
            .unwrap()
            .to_rfc2822();
        assert_eq!(
            r.next(401, "stale_request", Some(&date), &s, 0.0),
            Next::Resign
        );
        assert_eq!(s.offset(), 300);
        assert_eq!(s.now(), 1_760_000_300);
        assert_eq!(
            r.next(401, "stale_request", Some(&date), &s, 0.0),
            Next::GiveUp
        );
        let mut r = Retry::default();
        assert_eq!(r.next(401, "stale_request", None, &s, 0.0), Next::GiveUp);
        // A forged Date a year ahead is not learned: nothing is signed for
        // that time.
        let s = signer_at(1_760_000_000);
        let year = chrono::DateTime::from_timestamp(1_760_000_000 + 365 * 86_400, 0)
            .unwrap()
            .to_rfc2822();
        let mut r = Retry::default();
        assert_eq!(
            r.next(401, "stale_request", Some(&year), &s, 0.0),
            Next::ClockSkew(365 * 86_400)
        );
        assert_eq!((s.offset(), s.now()), (0, 1_760_000_000));
        let edge = chrono::DateTime::from_timestamp(1_760_000_000 - MAX_OFFSET_S, 0)
            .unwrap()
            .to_rfc2822();
        assert_eq!(s.learn_offset(&edge), Ok(-MAX_OFFSET_S));
        let past = chrono::DateTime::from_timestamp(1_760_000_000 - MAX_OFFSET_S - 1, 0)
            .unwrap()
            .to_rfc2822();
        assert_eq!(
            s.learn_offset(&past),
            Err(Learned::TooFar(-MAX_OFFSET_S - 1))
        );
        let mut r = Retry::default();
        assert_eq!(
            r.next(401, "invalid_signature", Some(&date), &s, 0.0),
            Next::GiveUp
        );
        // An HTTP date as hyper writes it.
        assert_eq!(
            daemon_offset("Sun, 11 Oct 2026 10:00:00 GMT", 0),
            Some(1_791_712_800)
        );
    }

    #[test]
    fn limits_and_busy_back_off_three_times_with_jitter() {
        let s = signer_at(0);
        let mut r = Retry::default();
        let got: Vec<Next> = (0..4).map(|_| r.next(503, "busy", None, &s, 0.0)).collect();
        assert_eq!(
            got,
            [
                Next::After(Duration::from_secs(1)),
                Next::After(Duration::from_secs(2)),
                Next::After(Duration::from_secs(4)),
                Next::GiveUp
            ]
        );
        let mut r = Retry::default();
        assert_eq!(
            r.next(429, "limit_reached", None, &s, 0.999),
            Next::After(Duration::from_secs(1) + Duration::from_secs(1).mul_f64(0.999 / 4.0))
        );
        // Another 429 or 503 is the answer.
        let mut r = Retry::default();
        assert_eq!(r.next(429, "too_many", None, &s, 0.0), Next::GiveUp);
        assert_eq!(r.next(503, "unavailable", None, &s, 0.0), Next::GiveUp);
    }
}

#[cfg(test)]
mod session_file_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn cred(session: &str) -> SessionCredential {
        SessionCredential {
            session: session.into(),
            secret: crate::secret::token(crate::secret::SESSION_PREFIX),
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn no_two_keys_share_a_file_name() {
        let names = [
            SessionKey::Harness {
                harness: "claude",
                id: "p-123",
            },
            SessionKey::Harness {
                harness: "claude",
                id: "ppid-123",
            },
            SessionKey::Parent {
                harness: "claude",
                pid: 123,
            },
            SessionKey::Harness {
                harness: "claude",
                id: "h-0123",
            },
            SessionKey::Harness {
                harness: "claude",
                id: "a/b",
            },
            SessionKey::Harness {
                harness: "claude",
                id: ".hidden",
            },
        ]
        .map(|k| k.file_name().unwrap());
        let mut unique = names.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        assert_eq!(names[2], "claude-p-123.json");
        assert_eq!(names[1], "claude-s-ppid-123.json");
        assert!(names[4].starts_with("claude-h-"), "{}", names[4]);
    }

    #[test]
    fn the_directory_is_made_private_and_never_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("remote-sessions");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let files = SessionFiles::at(dir.clone());
        files
            .write("claude", Some("cc-1"), None, None, &cred("S1"))
            .unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("claude-s-cc-1.json")), 0o600);
        // A file that is a symbolic link is refused, not followed.
        let elsewhere = tmp.path().join("elsewhere.json");
        std::fs::rename(dir.join("claude-s-cc-1.json"), &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("claude-s-cc-1.json")).unwrap();
        let key = SessionKey::Harness {
            harness: "claude",
            id: "cc-1",
        };
        let e = files.read(&key).unwrap_err();
        assert!(e.to_string().contains("symbolic link"), "{e}");
        // A directory that is a symbolic link is neither written nor read.
        let linked = tmp.path().join("linked");
        std::os::unix::fs::symlink(&dir, &linked).unwrap();
        let through = SessionFiles::at(linked);
        assert!(
            through
                .write("claude", Some("cc-2"), None, None, &cred("S2"))
                .is_err()
        );
        assert!(through.read(&key).is_err());
    }

    #[test]
    fn a_parent_pid_file_is_taken_only_as_join_would_match_and_by_the_same_process() {
        let tmp = tempfile::tempdir().unwrap();
        let files = SessionFiles::at(tmp.path().join("remote-sessions"));
        let me = std::process::id();
        let pids = [me];
        let seek = |hsid: Option<&'static str>| Seeker {
            harness: "codex",
            harness_session_id: hsid,
            pids: &pids,
        };
        // A row with no harness session ID, whose parent is this process.
        files
            .write("codex", None, Some(me), None, &cred("S0"))
            .unwrap();
        assert_eq!(
            files.find(&seek(Some("x-1"))).unwrap().unwrap().session,
            "S0"
        );
        assert_eq!(files.find(&seek(None)).unwrap().unwrap().session, "S0");
        // A row with another harness session ID: only a client that knows
        // none, or that one, takes it.
        files
            .write("codex", Some("b-h"), Some(me), None, &cred("SB"))
            .unwrap();
        assert!(files.find(&seek(Some("c-h"))).unwrap().is_none());
        assert_eq!(
            files.find(&seek(Some("b-h"))).unwrap().unwrap().session,
            "SB"
        );
        assert_eq!(files.find(&seek(None)).unwrap().unwrap().session, "SB");
        // Written while another process had this PID: never taken.
        let path = files
            .path(&SessionKey::Parent {
                harness: "codex",
                pid: me,
            })
            .unwrap();
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["harness_session_id"] = serde_json::Value::Null;
        v["parent_started"] = "Thu Jan  1 00:00:00 1970".into();
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(files.find(&seek(None)).unwrap().is_none());
        // Nor without a recorded start.
        v.as_object_mut().unwrap().remove("parent_started");
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(files.find(&seek(None)).unwrap().is_none());
    }
}
