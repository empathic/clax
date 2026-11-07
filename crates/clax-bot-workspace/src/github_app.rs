//! A GitHub App as a target's credentials. The App's private key stays in
//! this process and signs a JWT; the JWT buys installation tokens, each
//! narrowed to one repository and to what its holder needs. A container gets
//! at most a read token, and never the key. A read token is minted for one
//! clone and ended when that clone returns, so no two containers share one.
//!
//! GitHub's answers are not quoted in errors: a status says what went wrong,
//! and an answer could echo the JWT, which no [`crate::repo::Redactor`] holds.

use crate::repo::{RepoAccess, Token};
use crate::runtime::{bounded, control_client};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// The most of an answer that is read.
const MAX_ANSWER: usize = 1024 * 1024;

/// How far back a JWT's `iat` is set, for a clock that runs ahead of GitHub's.
const JWT_BACKDATE_SECS: u64 = 60;

/// How long a JWT lasts from now. GitHub allows ten minutes; this leaves room
/// for a clock that runs behind.
const JWT_LIFE_SECS: u64 = 540;

/// How long before `expires_at` a token stops being handed out: as long as
/// the longest git command, which holds its token from start to finish.
const TOKEN_MARGIN_SECS: u64 = 600;

const PEM_BEGIN: &str = "-----BEGIN RSA PRIVATE KEY-----";
const PEM_END: &str = "-----END RSA PRIVATE KEY-----";

/// Seconds since the epoch.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

fn system_clock() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The PKCS#1 key GitHub issues, as PEM. What is wrong with it is never said
/// by quoting it.
fn key_pair(pem: &str) -> Result<RsaKeyPair> {
    let body = pem
        .split_once(PEM_BEGIN)
        .and_then(|(_, rest)| rest.split_once(PEM_END))
        .map(|(body, _)| body)
        .context("the GitHub App's private key is not a PKCS#1 PEM")?;
    let clean: String = body.split_whitespace().collect();
    let der = STANDARD
        .decode(clean)
        .map_err(|_| anyhow!("the GitHub App's private key is not valid base64"))?;
    RsaKeyPair::from_der(&der).map_err(|_| anyhow!("the GitHub App's private key was refused"))
}

fn headers(r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    r.header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "clax-bot-workspace")
}

fn encoded(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}

/// `r`'s answer, or its status and nothing of its text.
async fn reply<T: DeserializeOwned>(r: reqwest::Response, what: &str) -> Result<T> {
    let status = r.status();
    if !status.is_success() {
        bail!("the forge answered {status} to {what}");
    }
    let body = bounded(r, MAX_ANSWER)
        .await?
        .with_context(|| format!("the forge's answer to {what} is over the limit"))?;
    serde_json::from_slice(&body).map_err(|_| anyhow!("the forge's answer to {what} is unreadable"))
}

/// A GitHub App, at one forge.
pub struct GitHubApp {
    api_url: String,
    app_id: u64,
    key: RsaKeyPair,
    http: reqwest::Client,
    clock: Clock,
}

#[derive(Deserialize)]
struct InstallationAnswer {
    id: u64,
    account: Account,
}

#[derive(Deserialize)]
struct Account {
    login: String,
}

/// An installation of the App, as its forge names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installation {
    pub id: u64,
    /// The account it is installed on.
    pub account: String,
}

#[derive(Deserialize)]
struct Minted {
    token: String,
    expires_at: String,
}

/// A token and when it stops working, in seconds since the epoch.
struct Issued {
    token: Token,
    expires_at: u64,
}

impl GitHubApp {
    pub fn new(api_url: &str, app_id: u64, private_key_pem: &str) -> Result<GitHubApp> {
        Ok(GitHubApp {
            api_url: api_url.trim_end_matches('/').to_string(),
            app_id,
            key: key_pair(private_key_pem)?,
            http: control_client(),
            clock: Arc::new(system_clock),
        })
    }

    /// This App, making its calls with `http`.
    #[must_use]
    pub fn with_http(self, http: reqwest::Client) -> GitHubApp {
        GitHubApp { http, ..self }
    }

    /// This App, telling the time by `clock`.
    #[must_use]
    pub fn with_clock(self, clock: Clock) -> GitHubApp {
        GitHubApp { clock, ..self }
    }

    /// A JWT that speaks for the App, RS256 as GitHub requires.
    fn jwt(&self) -> Result<String> {
        let now = (self.clock)();
        let head = encoded(&json!({"alg": "RS256", "typ": "JWT"}));
        let claims = encoded(&json!({
            "iat": now.saturating_sub(JWT_BACKDATE_SECS),
            "exp": now + JWT_LIFE_SECS,
            "iss": self.app_id.to_string(),
        }));
        let message = format!("{head}.{claims}");
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                message.as_bytes(),
                &mut signature,
            )
            .map_err(|_| anyhow!("signing as the GitHub App failed"))?;
        Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    fn signed(&self, r: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        Ok(headers(r).bearer_auth(self.jwt()?))
    }

    /// Ends `token`, which is what authenticates the call.
    async fn revoke(&self, token: &Token) -> Result<()> {
        let url = format!("{}/installation/token", self.api_url);
        let r = headers(self.http.delete(url))
            .bearer_auth(token.expose())
            .send()
            .await
            .context("ending an installation token")?;
        let status = r.status();
        if status != reqwest::StatusCode::NO_CONTENT {
            bail!("the forge answered {status} to ending an installation token");
        }
        Ok(())
    }

    /// The installation of this App on `repo`, `owner/name`: `None` when the
    /// forge knows of none. The caller has checked `repo`, which goes into a
    /// path.
    pub async fn installation_of(&self, repo: &str) -> Result<Option<Installation>> {
        let url = format!("{}/repos/{repo}/installation", self.api_url);
        let r = self
            .signed(self.http.get(url))?
            .send()
            .await
            .context("asking for the App's installation")?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let found: InstallationAnswer = reply(r, "the App's installation").await?;
        Ok(Some(Installation {
            id: found.id,
            account: found.account.login,
        }))
    }

    async fn mint(&self, installation: u64, repo: &str, permissions: Value) -> Result<Issued> {
        let url = format!(
            "{}/app/installations/{installation}/access_tokens",
            self.api_url
        );
        let r = self
            .signed(self.http.post(url))?
            .json(&json!({"repositories": [repo], "permissions": permissions}))
            .send()
            .await
            .context("asking for an installation token")?;
        let minted: Minted = reply(r, "an installation token").await?;
        let expires_at = chrono::DateTime::parse_from_rfc3339(&minted.expires_at)
            .context("an installation token's expiry")?
            .timestamp();
        Ok(Issued {
            token: Token::new(minted.token),
            expires_at: u64::try_from(expires_at).unwrap_or(0),
        })
    }
}

/// One repository's tokens from an App's installation. A write token, which
/// stays in this process, is kept until [`TOKEN_MARGIN_SECS`] before it
/// expires, and cannot touch workflows whatever the App may do: it is not
/// asked for that. A read token is never kept.
pub struct GitHubAppAccess {
    app: GitHubApp,
    installation: u64,
    /// The repository's name, without its owner.
    name: String,
    public: bool,
    write: Mutex<Option<Issued>>,
}

impl GitHubAppAccess {
    /// `repo` is `owner/name`. A `public` one gives a container no token.
    pub fn new(
        app: GitHubApp,
        installation: u64,
        repo: &str,
        public: bool,
    ) -> Result<GitHubAppAccess> {
        let (_, name) = repo
            .split_once('/')
            .with_context(|| format!("{repo} is not owner/name"))?;
        Ok(GitHubAppAccess {
            app,
            installation,
            name: name.to_string(),
            public,
            write: Mutex::default(),
        })
    }

    async fn mint(&self, permissions: Value) -> Result<Issued> {
        self.app
            .mint(self.installation, &self.name, permissions)
            .await
    }
}

#[async_trait::async_trait]
impl RepoAccess for GitHubAppAccess {
    async fn read_token(&self) -> Result<Option<Token>> {
        if self.public {
            return Ok(None);
        }
        let issued = self.mint(json!({"contents": "read"})).await?;
        Ok(Some(issued.token))
    }

    /// The lock is held across the call, so that callers together mint one.
    async fn write_token(&self) -> Result<Option<Token>> {
        let mut held = self.write.lock().await;
        let now = (self.app.clock)();
        if let Some(issued) = held.as_ref()
            && now + TOKEN_MARGIN_SECS < issued.expires_at
        {
            return Ok(Some(issued.token.clone()));
        }
        let permissions = json!({"contents": "write", "pull_requests": "write"});
        let issued = self.mint(permissions).await?;
        let token = issued.token.clone();
        *held = Some(issued);
        Ok(Some(token))
    }

    async fn revoke_read(&self, token: &Token) -> Result<()> {
        self.app.revoke(token).await
    }

    async fn write_refused(&self, token: &Token) {
        let mut held = self.write.lock().await;
        if held.as_ref().is_some_and(|i| i.token == *token) {
            *held = None;
        }
    }
}
