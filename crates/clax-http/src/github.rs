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
