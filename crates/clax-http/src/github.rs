//! GitHub sign-in on a hosted daemon (a GitHub OAuth App's web flow, with
//! PKCE). Off unless the daemon has a client ID and secret
//! ([`crate::hosted::Hosted::with_github`]); the gate lets both routes
//! through with no session.
//!
//! - `GET /auth/github` keeps a fresh `state` and PKCE verifier in the
//!   flow cookie (`clax_github`, `__Host-clax_github` on `https`; `HttpOnly`,
//!   `SameSite=Lax`, ten minutes, MACed with a key of the daemon token's)
//!   and redirects to GitHub's authorize page (scope `read:user`).
//! - `GET /auth/github/callback?code&state` checks the `state` against the
//!   cookie, exchanges the code (with the verifier) server-side, reads the
//!   account (`GET /user`), and signs the browser in as it
//!   ([`clax_store::Store::github_sign_in`]): the owner when its login is
//!   the daemon's `owner_github` (compared case-insensitively), else its
//!   person, made at its first sign-in with own-space rights. The browser's
//!   session before is ended (the click on "Sign in with GitHub" is the
//!   confirmation), and the browser goes to the gallery.
//!
//! The callback is rate limited per client address ([`Attempt::SignIn`]),
//! and a new person's sign-up again ([`Attempt::SignUp`]); the redirect,
//! which writes nothing, is not. A failed callback is
//! a browser navigation, so it answers an HTML page with a link to sign in
//! again, not JSON.

/// GitHub's OAuth endpoints' base.
pub const GITHUB_OAUTH_BASE: &str = "https://github.com";
/// GitHub's REST API's base.
pub const GITHUB_API_BASE: &str = "https://api.github.com";
/// The callback's path: the OAuth App's callback URL is the public origin
/// and this.
pub const CALLBACK_PATH: &str = "/auth/github/callback";
/// How long a sign-in may take between the redirect and the callback.
pub const FLOW_TTL_SECS: i64 = 600;

/// A GitHub OAuth App the daemon signs people in with.
#[derive(Clone)]
pub struct GitHub {
    pub client_id: String,
    /// The OAuth App's secret, sent only to GitHub (never in [`Debug`]).
    pub client_secret: String,
    /// The owner's GitHub login, compared case-insensitively.
    pub owner_login: Option<String>,
    /// Where GitHub's OAuth pages are: [`GITHUB_OAUTH_BASE`], or a fake's.
    pub oauth_base: String,
    /// Where GitHub's API is: [`GITHUB_API_BASE`], or a fake's.
    pub api_base: String,
}

impl std::fmt::Debug for GitHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHub")
            .field("client_id", &self.client_id)
            .field("owner_login", &self.owner_login)
            .finish()
    }
}

impl GitHub {
    /// The OAuth App `client_id` with `client_secret`, whose owner's login
    /// is `owner_login`; `None` when either is empty. GitHub's bases are
    /// `oauth_base` and `api_base`, or GitHub's own when `None`.
    pub fn new(
        client_id: &str,
        client_secret: &str,
        owner_login: Option<&str>,
        oauth_base: Option<&str>,
        api_base: Option<&str>,
    ) -> Option<GitHub> {
        let (id, secret) = (client_id.trim(), client_secret.trim());
        if id.is_empty() || secret.is_empty() {
            return None;
        }
        let base = |b: Option<&str>, d: &str| {
            b.map(str::trim)
                .filter(|b| !b.is_empty())
                .unwrap_or(d)
                .trim_end_matches('/')
                .to_string()
        };
        Some(GitHub {
            client_id: id.to_string(),
            client_secret: secret.to_string(),
            owner_login: owner_login
                .map(|o| o.trim().trim_start_matches('@').to_string())
                .filter(|o| !o.is_empty()),
            oauth_base: base(oauth_base, GITHUB_OAUTH_BASE),
            api_base: base(api_base, GITHUB_API_BASE),
        })
    }

    pub fn is_owner(&self, login: &str) -> bool {
        self.owner_login
            .as_deref()
            .is_some_and(|o| o.eq_ignore_ascii_case(login))
    }
}

/// GitHub sign-in on a local Clax (the home's `[github]`,
/// [`clax_core::config::HomeConfig::github`]; [`crate::state::AppState::local_github`]).
/// A sign-in makes the browser the person the account is linked to (made
/// at its first sign-in only for a login a pending queue invite or share
/// names), by a `clax_viewer` cookie, never the owner: the account has no
/// owner login ([`GitHub::owner_login`] is `None`), and one linked to the
/// owner's row is refused ([`clax_store::Store::github_sign_in_local`]).
#[derive(Clone, Debug)]
pub struct LocalGitHub {
    pub github: GitHub,
    /// The origins a sign-in starts and returns on, normalized
    /// (`http://localhost:7480`, `https://box.tail1.ts.net`): the OAuth
    /// App's callback URL is one of them and [`CALLBACK_PATH`].
    pub origins: Vec<String>,
}

impl LocalGitHub {
    /// `[github]` with its secret read by `secret` from the variable it
    /// names, its origins those it lists or `http://localhost:<port>`;
    /// `None` when the secret is unset or empty (sign-in stays off).
    pub fn from_config(
        c: &clax_core::config::LocalGitHubConfig,
        port: u16,
        secret: &dyn Fn(&str) -> Option<String>,
    ) -> Option<LocalGitHub> {
        let s = secret(&c.client_secret_env).unwrap_or_default();
        let github = GitHub::new(
            &c.client_id,
            &s,
            None,
            c.oauth_base.as_deref(),
            c.api_base.as_deref(),
        )?;
        let origins = if c.origins.is_empty() {
            vec![format!("http://localhost:{port}")]
        } else {
            c.origins.clone()
        };
        Some(LocalGitHub { github, origins })
    }

    /// The configured origin whose authority is `host` (a `Host` header's,
    /// compared ignoring case), the only one a sign-in on that host may
    /// name as its redirect.
    pub fn origin_for_host(&self, host: &str) -> Option<&str> {
        self.origins
            .iter()
            .find(|o| {
                o.split_once("://")
                    .is_some_and(|(_, a)| a.eq_ignore_ascii_case(host))
            })
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(origins: &[&str]) -> clax_core::config::LocalGitHubConfig {
        clax_core::config::LocalGitHubConfig {
            client_id: "id".into(),
            client_secret_env: "S".into(),
            origins: origins.iter().map(|o| o.to_string()).collect(),
            api_base: None,
            oauth_base: None,
        }
    }

    #[test]
    fn a_local_sign_in_needs_its_secret_and_names_only_its_origins() {
        let none = |_: &str| None;
        assert!(LocalGitHub::from_config(&cfg(&[]), 7480, &none).is_none());
        let some = |v: &str| (v == "S").then(|| "sec".to_string());
        let l = LocalGitHub::from_config(&cfg(&[]), 7480, &some).unwrap();
        assert_eq!(l.origins, ["http://localhost:7480"]);
        assert_eq!(l.github.owner_login, None);
        assert_eq!(
            l.origin_for_host("LOCALHOST:7480"),
            Some("http://localhost:7480")
        );
        assert_eq!(l.origin_for_host("localhost:7481"), None);
        assert_eq!(l.origin_for_host("127.0.0.1:7480"), None);
        let l = LocalGitHub::from_config(&cfg(&["https://box.tail1.ts.net"]), 7480, &some).unwrap();
        assert_eq!(
            l.origin_for_host("box.tail1.ts.net"),
            Some("https://box.tail1.ts.net")
        );
        assert_eq!(l.origin_for_host("localhost:7480"), None);
    }
}
