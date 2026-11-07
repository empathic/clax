//! A queue's link to a repository, and the instance's GitHub App (migration
//! 64): the route a queue admin has to give ClaxBot a repository, beside the
//! operator's `workspace.toml` (`crates/clax-bot-workspace/src/config.rs`).
//! A link's fields mean what `TargetConfig`'s do, but for
//! `container_repo_url`, which a link does not have: a container clones from
//! `repo_url`.
//!
//! A link reaches its repository in one of two ways:
//!
//! - `token`: fine-grained tokens the queue's admins paste, a read token for
//!   the container's clone and a write token for the host, never the same
//!   value. They are kept sealed in the queue's own slots
//!   ([`SecretSlot::RepoRead`], [`SecretSlot::RepoWrite`]) and bound to the
//!   link: changing its repository, its `repo_url` or its `forge_api_url`
//!   clears them in the same transaction, so nobody who cannot read a token
//!   can send it somewhere new.
//! - `github_app`: an installation of the instance's App, whose private key
//!   only the owner sets ([`SecretSlot::GithubApp`]). The App's tokens go to
//!   the instance's forge alone: such a link stores no `repo_url` or
//!   `forge_api_url` and reads both from the App's row at every use.
//!
//! Every link has a `rev`, new at each change, and a write names the `rev`
//! it was made from: 400 `link_changed` otherwise. Two admins at one panel
//! cannot undo each other's change by accident, and nobody seals a token to
//! an address they did not see.
//!
//! Nobody types an App link's installation: the route asks the forge, with the
//! App's own JWT, which installation holds the repository, and the link keeps
//! the account the forge names. That proves nothing about who asked, and the
//! App's key reaches every installation, so an App link is usable only once
//! the owner has approved it, reading that account. A token link needs the same when the owner says so,
//! which until they do is on any instance but a local daemon
//! ([`Store::repo_token_approval`]). The owner's own new link is approved as
//! it is made. A change to its repository, URLs or installation voids an
//! approval, and so does anyone else's change to what else the owner read:
//! `public`, `allow_open_egress`, `reviewed_previews`, `wake_on_reply`, the
//! preview's URL and readiness path, `protected_paths`, `ci_accepted`. The
//! owner sees all of it
//! on every link, whoever made it ([`Store::repo_links`]), and may disconnect
//! any. A link on a queue left with no live admin when a person is revoked is
//! removed, tokens and all.
//!
//! An approval says where a link goes, not whose token goes there. A token
//! does not change `rev`: it is its admin's own to give, it can reach only the
//! addresses the owner read, since those clear it, and the owner never sees
//! the account behind one, so an approval given before any token was pasted
//! means the same as one given after. Replacing a token is recorded as
//! `admin.key`.
//!
//! Everything a person enters here is a place the host will contact, so
//! [`check_url`] holds it to `https://`, or `http://` on loopback for the
//! owner of a local daemon ([`BaseUrlPolicy`]); no user information, no
//! option-like value, no private address literal for anyone else. A name
//! that resolves to a private address is the connecting client's to refuse:
//! [`ReadyTarget::local_forge`] and [`ReadyTarget::local_preview`] say whether
//! it must, and a workspace's clients and host-side git do.
//!
//! A link is confidential unless its admin says it is `public`: whether a
//! repository is private cannot be told from here, and guessing wrong would
//! show private source to a reporter. A confidential link opens only where
//! egress is held by an allow-list, or with `allow_open_egress`, the rule
//! `Config::check_egress` holds an operator's target to. It needs a read
//! token, on `token` access, for the container's clone, and a queue admin's
//! `ci_accepted`: the repository's CI may run ClaxBot's branch, with that CI's
//! secrets and network, before anyone reviews it. Clax cannot see what a
//! repository's CI runs, so nothing here looks. The owner's approval does not
//! stand in for it, and it does not move to another repository with the link.
//!
//! Reads give metadata. [`Store::open_queue_repo`] and, for the lookup of an
//! installation, [`Store::open_github_app`] alone open a sealed value.
//!
//! What became of a link's credentials is a fact of its own row, never of a
//! key it shares with other links: whoever used one says so
//! ([`Store::note_repo_ok`], [`Store::note_repo_failure`]), and a person reads
//! `last_ok_at` and `last_failure` beside it.

use super::Store;
use super::bot_settings::BaseUrlPolicy;
use super::queues::queue_in;
use super::roles::{OWNER, set_by};
use super::secrets::{
    Kek, Sealing, SecretMeta, SecretSlot, check_key, clear_in, last4_of, meta_in, open_in, put_in,
};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::secret::Secret;
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Deserialize;

/// GitHub's API, the forge a link names none of.
pub const GITHUB_API: &str = "https://api.github.com";

/// The longest URL accepted, in bytes.
const MAX_URL_BYTES: usize = 512;
/// The most protected paths a link may add.
const MAX_PROTECTED_PATHS: usize = 64;
/// The longest protected path or readiness path, in bytes.
const MAX_PATH_BYTES: usize = 256;
/// The longest wait for a preview, in seconds: a day.
const MAX_PREVIEW_SECS: u64 = 86_400;
/// The longest PEM document accepted as the App's key, in bytes.
const MAX_APP_KEY_BYTES: usize = 8192;
/// The shortest repository token accepted, in bytes: a forge's are longer,
/// and anything less is a slip of the paste.
const MIN_TOKEN_BYTES: usize = 20;

/// How a link reaches its repository, as a person sets it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepoAccess {
    /// An installation of the instance's GitHub App. A person names only the
    /// kind: whoever handles the request fills both from the forge's answer,
    /// and a link with neither is refused.
    GithubApp {
        #[serde(skip_deserializing)]
        installation_id: u64,
        /// The account the forge says holds the installation.
        #[serde(skip_deserializing)]
        account: String,
    },
    /// Tokens kept in the queue's slots ([`Store::set_queue_repo_token`]).
    Token,
}

/// A link as a person sets it ([`Store::set_queue_repo`]). A field left out
/// takes `TargetConfig`'s default.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoLinkFields {
    /// `owner/name`.
    pub repo: String,
    /// The branch work starts from: `main` when left out.
    #[serde(default)]
    pub base: Option<String>,
    /// Where the host pushes and a container clones from. `token` access:
    /// GitHub's address of `repo` when left out. `github_app` access: the
    /// instance forge's, and anything else is refused.
    #[serde(default)]
    pub repo_url: Option<String>,
    /// The forge's API. `token` access: [`GITHUB_API`] when left out.
    /// `github_app` access: the instance's, and anything else is refused.
    #[serde(default)]
    pub forge_api_url: Option<String>,
    /// On top of the workspace's defaults: an exact file, or a directory
    /// ending in `/`.
    #[serde(default)]
    pub protected_paths: Vec<String>,
    /// The preview's URL, with `{pr}` for the pull request's number.
    pub preview_url: String,
    /// `/healthz` when left out.
    #[serde(default)]
    pub preview_ready_path: Option<String>,
    /// Whether the answer there must name the commit: `true` when left out.
    #[serde(default)]
    pub preview_names_commit: Option<bool>,
    /// 1200 when left out.
    #[serde(default)]
    pub preview_timeout_secs: Option<u64>,
    /// 10 when left out.
    #[serde(default)]
    pub preview_poll_secs: Option<u64>,
    pub access: RepoAccess,
    /// That the repository may be shown to anyone. Left out, the link is
    /// confidential.
    #[serde(default)]
    pub public: bool,
    /// That a confidential link may run with egress no allow-list holds.
    #[serde(default)]
    pub allow_open_egress: bool,
    /// That each deploy needs a person's approval of the commit.
    #[serde(default)]
    pub reviewed_previews: bool,
    /// That a reporter's plain reply to a waiting fix starts a run.
    #[serde(default)]
    pub wake_on_reply: bool,
    /// That the repository's CI may run ClaxBot's branch, with that CI's
    /// secrets and network, before anyone reviews it. A confidential link
    /// waits for it ([`LinkStatus::NeedsCiAcceptance`]).
    #[serde(default)]
    pub ci_accepted: bool,
    /// The `rev` this was made from; `None` for no link.
    #[serde(default)]
    pub if_rev: Option<String>,
}

/// Which of a queue's tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Crosses into the container, once, for its clone.
    Read,
    /// Stays on the host.
    Write,
}

impl TokenKind {
    /// The token named `s` in a route's path: `read` or `write`.
    #[must_use]
    pub fn parse(s: &str) -> Option<TokenKind> {
        match s {
            "read" => Some(TokenKind::Read),
            "write" => Some(TokenKind::Write),
            _ => None,
        }
    }

    fn slot(self, qid: &str) -> SecretSlot {
        match self {
            TokenKind::Read => SecretSlot::RepoRead(qid.into()),
            TokenKind::Write => SecretSlot::RepoWrite(qid.into()),
        }
    }

    fn other(self) -> TokenKind {
        match self {
            TokenKind::Read => TokenKind::Write,
            TokenKind::Write => TokenKind::Read,
        }
    }
}

/// Whether a link can be used now, and what it waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    Ready,
    /// `token` access with no write token.
    NeedsToken,
    /// `token` access to a confidential repository with no read token: a
    /// container has nothing to clone it with.
    NeedsReadToken,
    /// `github_app` access, and the instance has no App with a key.
    NeedsApp,
    /// A confidential repository, and no queue admin has said its CI may run
    /// ClaxBot's branch. Clax cannot see what that CI runs, and the owner's
    /// approval does not say it: acceptance comes first, and is what the
    /// owner then reads.
    NeedsCiAcceptance,
    /// `github_app` access, or `token` access while
    /// [`Store::repo_token_approval`], the owner has not approved.
    NeedsApproval,
}

impl LinkStatus {
    /// The name routes give it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LinkStatus::Ready => "ready",
            LinkStatus::NeedsToken => "needs_token",
            LinkStatus::NeedsReadToken => "needs_read_token",
            LinkStatus::NeedsApp => "needs_app",
            LinkStatus::NeedsCiAcceptance => "needs_ci_acceptance",
            LinkStatus::NeedsApproval => "needs_approval",
        }
    }
}

/// How a link reaches its repository, as read: no secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAccess {
    GithubApp {
        installation_id: u64,
        /// The account the forge named when the link was set.
        account: String,
        /// The person key of who approved it, when anyone has.
        approved_by: Option<String>,
        approved_at: Option<String>,
    },
    Token {
        read_token: Option<Box<SecretMeta>>,
        write_token: Option<Box<SecretMeta>>,
        /// The person key of who approved it, when anyone has.
        approved_by: Option<String>,
        approved_at: Option<String>,
    },
}

/// How a target's previews work: `Preview` of
/// `crates/clax-bot-workspace/src/preview.rs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoPreview {
    /// With `{pr}` for the pull request's number.
    pub url: String,
    pub ready_path: String,
    pub names_commit: bool,
    pub timeout_secs: u64,
    pub poll_secs: u64,
}

/// A queue's link as a person reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoLink {
    pub queue_id: String,
    /// New at each change; a write names the one it was made from.
    pub rev: String,
    pub repo: String,
    pub base: String,
    /// In effect: for `github_app` access, the instance forge's.
    pub repo_url: String,
    /// In effect: for `github_app` access, the instance's.
    pub forge_api_url: String,
    pub protected_paths: Vec<String>,
    pub preview: RepoPreview,
    pub access: LinkAccess,
    pub public: bool,
    pub allow_open_egress: bool,
    pub reviewed_previews: bool,
    pub wake_on_reply: bool,
    pub ci_accepted: bool,
    pub status: LinkStatus,
    /// When a workspace last reached this link's repository with its
    /// credentials ([`Store::note_repo_ok`]): `None` until one has, and again
    /// once what it reached with has changed. One link's success says nothing
    /// of another's on the same App.
    pub last_ok_at: Option<String>,
    /// What failed the last time a workspace used it, in fixed words, since
    /// nothing has worked after that ([`Store::note_repo_failure`]).
    pub last_failure: Option<String>,
    pub last_failure_at: Option<String>,
    /// [`ReadyTarget::local_preview`], read without opening anything.
    pub local_preview: bool,
    /// The person key of who last changed it.
    pub set_by: String,
    pub set_at: String,
}

/// The credentials of a [`ReadyTarget`], opened.
#[derive(Clone, Debug)]
pub enum RepoCredentials {
    /// The App's key, for minting installation tokens at the target's
    /// `forge_api_url` and nowhere else. The key reaches every installation,
    /// so each token minted must be narrowed: `repositories: [name]` for
    /// this repository alone, with scoped permissions, and the one that goes
    /// into a container `contents: read` and nothing more.
    GithubApp {
        app_id: u64,
        installation_id: u64,
        private_key: Secret,
    },
    Token {
        /// For the container's clone; `None` for a public repository.
        read: Option<Secret>,
        /// For the host alone.
        write: Secret,
    },
}

/// Whether the runtime that would hold a link's container keeps its egress
/// to an allow-list ([`Store::open_queue_repo`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Egress {
    /// A non-empty allow-list, on a runtime that enforces it.
    Held,
    /// Anything else.
    Open,
}

/// A link that can be used now, with its credentials
/// ([`Store::open_queue_repo`]).
///
/// A wiring must treat a link that is not `public` as confidential: a run on
/// it holds confidential material, whatever else it is told, and a failed
/// lookup of this target must leave a run confidential too.
#[derive(Clone, Debug)]
pub struct ReadyTarget {
    pub queue_id: String,
    /// Which credentials these are, for what a run later says of them
    /// ([`Store::note_repo_ok`]): it changes with the link, either token and
    /// the App's key.
    pub stamp: String,
    /// `owner/name`.
    pub repo: String,
    pub base: String,
    pub repo_url: String,
    pub forge_api_url: String,
    pub protected_paths: Vec<String>,
    pub preview: RepoPreview,
    pub credentials: RepoCredentials,
    /// That the repository may be shown to anyone.
    pub public: bool,
    /// That each deploy needs a person's approval of the commit.
    pub reviewed_previews: bool,
    /// That a reporter's plain reply to a waiting fix starts a run.
    pub wake_on_reply: bool,
    /// Whether `repo_url` and `forge_api_url` may be on the daemon's machine
    /// or its network: set by its owner, for a token link or the App. If
    /// not, a name that resolves to a private address must be refused at the
    /// connection, host-side git's included.
    pub local_forge: bool,
    /// The same of the preview's URL, which the link's own rule decides
    /// whoever set the App.
    pub local_preview: bool,
}

/// What a use of a link's credentials asked of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoUse {
    /// A clone: any token that can read does this.
    Read,
    /// A push or a commit, which is what a link is for.
    Write,
}

/// What a link's panel reads when a change could not be published. A read that
/// works says nothing of it.
pub const PUBLISH_FAILED: &str = "The daemon could not publish a change to the repository.";

/// The instance's GitHub App as the owner sets it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubAppFields {
    pub app_id: u64,
    /// The App's name in its URLs, for its install link.
    pub slug: String,
    /// The forge's API: [`GITHUB_API`] when left out.
    #[serde(default)]
    pub api_url: Option<String>,
}

/// The instance's GitHub App with its key ([`Store::open_github_app`]).
#[derive(Clone, Debug)]
pub struct OpenedApp {
    pub app_id: u64,
    pub api_url: String,
    pub private_key: Secret,
    /// [`ReadyTarget::local_forge`], of the App's forge.
    pub local_forge: bool,
}

/// The instance's GitHub App as read: no secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubApp {
    pub app_id: u64,
    pub slug: String,
    /// The forge the App's tokens go to, and no other.
    pub api_url: String,
    /// Where a person installs the App on a repository.
    pub install_url: String,
    /// Its private key's metadata; `last4` is the start of the key's
    /// SHA-256, since a PEM document's end is the same in every key.
    pub key: Option<SecretMeta>,
    pub set_by: String,
    pub set_at: String,
}

/// One link, as the owner lists them ([`Store::repo_links`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoLine {
    pub queue_id: String,
    pub queue_name: String,
    pub repo: String,
    pub repo_url: String,
    pub forge_api_url: String,
    /// `github_app` or `token`.
    pub access: &'static str,
    pub installation_id: Option<u64>,
    /// The account the forge named as holding that installation.
    pub account: Option<String>,
    pub status: LinkStatus,
    pub approved: bool,
    /// Whether an approval decides anything for this link.
    pub approvable: bool,
    pub public: bool,
    pub allow_open_egress: bool,
    pub reviewed_previews: bool,
    pub wake_on_reply: bool,
    pub preview_url: String,
    pub preview_ready_path: String,
    pub protected_paths: Vec<String>,
    pub ci_accepted: bool,
    pub rev: String,
    /// The person key of who made the queue.
    pub maker: String,
    /// The person keys of the queue's admins, its maker among them.
    pub admins: Vec<String>,
    pub set_by: String,
    pub set_at: String,
}

/// A link's checked settings: every column a person chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Settings {
    repo: String,
    base: String,
    /// `None` for `github_app` access.
    repo_url: Option<String>,
    /// `None` for `github_app` access.
    forge_api_url: Option<String>,
    protected_paths: Vec<String>,
    preview: RepoPreview,
    installation_id: Option<u64>,
    /// The account the forge named as holding `installation_id`.
    account: Option<String>,
    public: bool,
    allow_open_egress: bool,
    reviewed_previews: bool,
    wake_on_reply: bool,
    ci_accepted: bool,
}

impl Settings {
    fn access(&self) -> &'static str {
        if self.installation_id.is_some() {
            "github_app"
        } else {
            "token"
        }
    }

    /// What a credential is bound to, and an approval given for: a change
    /// to any of it clears the tokens and voids the approval.
    fn binding(&self) -> (&str, Option<&str>, Option<&str>, Option<u64>) {
        (
            &self.repo,
            self.repo_url.as_deref(),
            self.forge_api_url.as_deref(),
            self.installation_id,
        )
    }

    /// Whether an approval of `o` holds for this: the same of everything
    /// that makes a link dangerous beside its [`Settings::binding`]. A change
    /// in either direction counts, since a review or a protected path dropped
    /// loosens a link as `public` does.
    fn same_terms(&self, o: &Settings) -> bool {
        self.public == o.public
            && self.allow_open_egress == o.allow_open_egress
            && self.reviewed_previews == o.reviewed_previews
            && self.wake_on_reply == o.wake_on_reply
            && self.preview.url == o.preview.url
            && self.preview.ready_path == o.preview.ready_path
            && self.protected_paths == o.protected_paths
            && self.ci_accepted == o.ci_accepted
    }

    /// The addresses the host contacts for this link.
    fn addresses(&self) -> (Option<&str>, Option<&str>, &str) {
        (
            self.repo_url.as_deref(),
            self.forge_api_url.as_deref(),
            &self.preview.url,
        )
    }

    /// The name of each field that differs from `old`, every one for a
    /// link with no `old`.
    fn changed_from(&self, old: Option<&Settings>) -> Vec<&'static str> {
        let Some(o) = old else {
            return vec![
                "repo",
                "base",
                "repo_url",
                "forge_api_url",
                "protected_paths",
                "preview_url",
                "preview_ready_path",
                "preview_names_commit",
                "preview_timeout_secs",
                "preview_poll_secs",
                "installation_id",
                "installation_account",
                "public",
                "allow_open_egress",
                "reviewed_previews",
                "wake_on_reply",
                "ci_accepted",
            ];
        };
        [
            ("repo", self.repo != o.repo),
            ("base", self.base != o.base),
            ("repo_url", self.repo_url != o.repo_url),
            ("forge_api_url", self.forge_api_url != o.forge_api_url),
            ("protected_paths", self.protected_paths != o.protected_paths),
            ("preview_url", self.preview.url != o.preview.url),
            (
                "preview_ready_path",
                self.preview.ready_path != o.preview.ready_path,
            ),
            (
                "preview_names_commit",
                self.preview.names_commit != o.preview.names_commit,
            ),
            (
                "preview_timeout_secs",
                self.preview.timeout_secs != o.preview.timeout_secs,
            ),
            (
                "preview_poll_secs",
                self.preview.poll_secs != o.preview.poll_secs,
            ),
            ("installation_id", self.installation_id != o.installation_id),
            ("installation_account", self.account != o.account),
            ("public", self.public != o.public),
            (
                "allow_open_egress",
                self.allow_open_egress != o.allow_open_egress,
            ),
            (
                "reviewed_previews",
                self.reviewed_previews != o.reviewed_previews,
            ),
            ("wake_on_reply", self.wake_on_reply != o.wake_on_reply),
            ("ci_accepted", self.ci_accepted != o.ci_accepted),
        ]
        .into_iter()
        .filter_map(|(name, differs)| differs.then_some(name))
        .collect()
    }
}

/// A row of `queue_repos`.
#[derive(Clone, Debug)]
struct LinkRow {
    settings: Settings,
    rev: String,
    approved_by: Option<String>,
    approved_at: Option<String>,
    owner_addresses: bool,
    last_ok_at: Option<String>,
    last_failure: Option<String>,
    last_failure_at: Option<String>,
    set_by: String,
    set_at: String,
}

/// A row of `instance_github_app`.
#[derive(Clone, Debug)]
struct AppRow {
    app_id: u64,
    slug: String,
    api_url: String,
    set_by: String,
    set_at: String,
}

const LINK_COLS: &str = "repo, base, repo_url, forge_api_url, protected_paths_json, preview_url,
    preview_ready_path, preview_names_commit, preview_timeout_secs, preview_poll_secs,
    installation_id, approved_by, approved_at, set_by, set_at, rev, owner_addresses, public,
    allow_open_egress, reviewed_previews, wake_on_reply, installation_account, last_failure,
    last_failure_at, ci_accepted, last_ok_at";

fn row_to_link(r: &Row<'_>) -> rusqlite::Result<LinkRow> {
    let paths: String = r.get(4)?;
    // A row that cannot say what it protects is an error, never no protection.
    let protected_paths = serde_json::from_str(&paths).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(LinkRow {
        settings: Settings {
            repo: r.get(0)?,
            base: r.get(1)?,
            repo_url: r.get(2)?,
            forge_api_url: r.get(3)?,
            protected_paths,
            preview: RepoPreview {
                url: r.get(5)?,
                ready_path: r.get(6)?,
                names_commit: r.get(7)?,
                timeout_secs: r.get(8)?,
                poll_secs: r.get(9)?,
            },
            installation_id: r.get(10)?,
            account: r.get(21)?,
            public: r.get(17)?,
            allow_open_egress: r.get(18)?,
            reviewed_previews: r.get(19)?,
            wake_on_reply: r.get(20)?,
            ci_accepted: r.get(24)?,
        },
        approved_by: r.get(11)?,
        approved_at: r.get(12)?,
        set_by: r.get(13)?,
        set_at: r.get(14)?,
        rev: r.get(15)?,
        owner_addresses: r.get(16)?,
        last_ok_at: r.get(25)?,
        last_failure: r.get(22)?,
        last_failure_at: r.get(23)?,
    })
}

fn link_in(c: &Connection, qid: &str) -> Result<Option<LinkRow>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {LINK_COLS} FROM queue_repos WHERE queue_id = ?1"
    ))?
    .query_row(params![qid], row_to_link)
    .optional()?)
}

fn app_in(c: &Connection) -> Result<Option<AppRow>> {
    Ok(c.prepare_cached(
        "SELECT app_id, slug, api_url, set_by, set_at FROM instance_github_app WHERE id = 1",
    )?
    .query_row([], |r| {
        Ok(AppRow {
            app_id: r.get(0)?,
            slug: r.get(1)?,
            api_url: r.get(2)?,
            set_by: r.get(3)?,
            set_at: r.get(4)?,
        })
    })
    .optional()?)
}

/// Whether a token link needs the owner's approval: as the owner said, else
/// on any instance but a local daemon, where the owner is the only person
/// with reason to connect one.
fn token_approval_in(st: &Store, c: &Connection) -> Result<bool> {
    let said: Option<bool> = c
        .prepare_cached("SELECT token_approval FROM instance_repo_policy WHERE id = 1")?
        .query_row([], |r| r.get(0))
        .optional()?;
    Ok(said.unwrap_or(st.base_url_policy() != BaseUrlPolicy::Local))
}

/// The web address of the forge whose API is at `api_url`.
fn forge_web(api_url: &str) -> String {
    if api_url == GITHUB_API {
        return "https://github.com".into();
    }
    api_url.strip_suffix("/api/v3").unwrap_or(api_url).into()
}

/// The address of `repo` on the forge whose API is at `api_url`.
fn repo_url_on(api_url: &str, repo: &str) -> String {
    format!("{}/{repo}.git", forge_web(api_url))
}

/// The forge an App link goes to: the App's, or GitHub's while there is no
/// App, when nothing can use the link.
fn app_forge(app: Option<&AppRow>) -> &str {
    app.map_or(GITHUB_API, |a| a.api_url.as_str())
}

/// The URLs in effect for `s` under `app`: the repository's, the forge's.
fn urls_of(s: &Settings, app: Option<&AppRow>) -> (String, String) {
    match (&s.repo_url, &s.forge_api_url) {
        (Some(r), Some(f)) => (r.clone(), f.clone()),
        _ => {
            let forge = app_forge(app);
            (repo_url_on(forge, &s.repo), forge.to_string())
        }
    }
}

/// The policy `by_owner`'s URLs answer to: the instance's for the owner, who
/// may reach their own machine. Anyone else on a local daemon is held to a
/// hosted instance's rule, since the host would make those requests for them.
pub(crate) fn policy_for(st: &Store, by_owner: bool) -> BaseUrlPolicy {
    match st.base_url_policy() {
        BaseUrlPolicy::Local if !by_owner => BaseUrlPolicy::Hosted,
        p => p,
    }
}

/// `raw` trimmed of space and any last `/`.
fn norm(raw: &str) -> &str {
    raw.trim().trim_end_matches('/')
}

/// `raw` trimmed of space and any last `/`, when it is a URL the host may
/// contact under `policy`. A refusal under `code` never repeats the value,
/// which may hold a credential.
pub(crate) fn check_url(code: &'static str, raw: &str, policy: BaseUrlPolicy) -> Result<String> {
    let bad = |why: &str| CoreError::invalid(code, why);
    let s = raw.trim();
    if s.is_empty() || s.len() > MAX_URL_BYTES {
        return Err(bad("a URL is 1 to 512 bytes"));
    }
    if s.starts_with('-') {
        return Err(bad("a URL may not start with '-'"));
    }
    if s.chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(bad("a URL holds no spaces, control characters or '\\'"));
    }
    let u = url::Url::parse(s).map_err(|_| bad("that is not a URL"))?;
    if !u.username().is_empty() || u.password().is_some() || s.contains('@') {
        return Err(bad(
            "a URL holds no user name or password: credentials are set apart",
        ));
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err(bad("a URL here has no query or fragment"));
    }
    // Both spellings: `url` turns `https://127.1` into its address.
    if !policy.allows(s) || !policy.allows(u.as_str()) {
        return Err(bad(
            "a URL here is https:// to a public host name; only the owner of a local daemon may use http:// on loopback",
        ));
    }
    Ok(s.trim_end_matches('/').to_string())
}

/// `template` trimmed, when it names `{pr}` and is, with a number there, a
/// URL [`check_url`] accepts.
fn check_preview_url(template: &str, policy: BaseUrlPolicy) -> Result<String> {
    let t = template.trim();
    if !t.contains("{pr}") {
        return Err(CoreError::invalid(
            "invalid_preview_url",
            "a preview URL names {pr}, the pull request's number",
        ));
    }
    check_url("invalid_preview_url", &t.replace("{pr}", "1"), policy)?;
    Ok(t.trim_end_matches('/').to_string())
}

/// The host of `url`, lowercase, without a last `.`, so `github.com.` is
/// `github.com`.
fn host_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(u.host_str()?.trim_end_matches('.').to_ascii_lowercase())
}

/// 400 `forge_required` for a `repo_url` off GitHub with no forge named: the
/// default forge is GitHub's, and a write token set for another forge would
/// go to a host nobody chose.
fn check_forge_named(f: &RepoLinkFields) -> Result<()> {
    let elsewhere = f
        .repo_url
        .as_deref()
        .is_some_and(|u| host_of(u).as_deref() != Some("github.com"));
    if f.forge_api_url.is_none() && elsewhere && f.access == RepoAccess::Token {
        return Err(CoreError::invalid(
            "forge_required",
            "a repository off github.com needs its forge's API URL",
        ));
    }
    Ok(())
}

/// 400 `repo_url_mismatch` when `repo_url` is on a forge Clax knows the
/// paths of, GitHub or `app`'s, and is not `repo`'s address there: the
/// container would clone one repository and the pull request open on
/// another. Elsewhere a path is that forge's own.
fn check_repo_path(repo_url: &str, repo: &str, app: Option<&AppRow>) -> Result<()> {
    let Some(host) = host_of(repo_url) else {
        return Ok(());
    };
    let known = host == "github.com"
        || app.is_some_and(|a| host_of(&forge_web(&a.api_url)).as_deref() == Some(&host));
    if !known {
        return Ok(());
    }
    let u = url::Url::parse(repo_url).map_err(|_| CoreError::NotFound)?;
    let path = u.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.eq_ignore_ascii_case(repo) {
        return Ok(());
    }
    Err(CoreError::invalid(
        "repo_url_mismatch",
        "that repository URL is not the repository's address on its forge: leave it empty, or name the same repository",
    ))
}

/// `repo` trimmed, when it is `owner/name` in a forge's characters.
///
/// # Errors
/// `invalid_repo`.
pub fn check_repo(repo: &str) -> Result<String> {
    let bad = || {
        CoreError::invalid(
            "invalid_repo",
            "a repository is owner/name, each up to 100 letters, digits, '.', '_' and '-', not starting with '-' or '.'",
        )
    };
    let r = repo.trim();
    let (owner, name) = r.split_once('/').ok_or_else(bad)?;
    let ok = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && !p.starts_with(['-', '.'])
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !ok(owner) || !ok(name) || name.ends_with(".git") {
        return Err(bad());
    }
    Ok(r.to_string())
}

/// `base` trimmed, when git takes it as a branch and no command takes it
/// as an option.
fn check_base(base: &str) -> Result<String> {
    let b = base.trim();
    let ok = !b.is_empty()
        && b.len() <= 200
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.ends_with(".lock")
        && !b.contains("..")
        && !b.contains("//")
        && b.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/'));
    if !ok {
        return Err(CoreError::invalid(
            "invalid_base",
            "a base branch is a branch name: letters, digits, '.', '_', '-' and '/', not starting with '-'",
        ));
    }
    Ok(b.to_string())
}

/// `path`, when it can only extend a URL's path.
fn check_ready_path(path: &str) -> Result<String> {
    let p = path.trim();
    let ok = p.starts_with('/')
        && !p.starts_with("//")
        && p.len() <= MAX_PATH_BYTES
        && !p.split('/').any(|seg| seg == "..")
        && !p
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '#'));
    if !ok {
        return Err(CoreError::invalid(
            "invalid_ready_path",
            "a readiness path starts with one '/', up to 256 bytes, with no spaces, '..', '\\' or '#'",
        ));
    }
    Ok(p.to_string())
}

/// `paths` trimmed, when each is a path inside a repository.
fn check_protected(paths: &[String]) -> Result<Vec<String>> {
    let bad = || {
        CoreError::invalid(
            "invalid_protected_path",
            "a protected path is a relative file, or directory ending in '/', at most 64 of them",
        )
    };
    if paths.len() > MAX_PROTECTED_PATHS {
        return Err(bad());
    }
    paths
        .iter()
        .map(|p| {
            let p = p.trim();
            let ok = !p.is_empty()
                && p.len() <= MAX_PATH_BYTES
                && !p.starts_with(['/', '-'])
                && !p.split('/').any(|seg| seg == "..")
                && !p.chars().any(|c| c.is_control() || c == '\\');
            ok.then(|| p.to_string()).ok_or_else(bad)
        })
        .collect()
}

/// `f` with its defaults taken, its addresses as given: everything but
/// those checked. [`check_addresses`] checks them.
fn settings_of(f: &RepoLinkFields) -> Result<Settings> {
    let repo = check_repo(&f.repo)?;
    let timeout_secs = f.preview_timeout_secs.unwrap_or(1200);
    let poll_secs = f.preview_poll_secs.unwrap_or(10);
    if !(1..=MAX_PREVIEW_SECS).contains(&timeout_secs) || poll_secs == 0 || poll_secs > timeout_secs
    {
        return Err(CoreError::invalid(
            "invalid_preview_timing",
            "a preview's timeout is 1 second to a day, and its poll at least 1 second and no more than the timeout",
        ));
    }
    let (repo_url, forge_api_url, installation_id, account) = match &f.access {
        RepoAccess::Token => (
            Some(
                f.repo_url
                    .as_deref()
                    .map_or_else(|| repo_url_on(GITHUB_API, &repo), |u| norm(u).to_string()),
            ),
            Some(
                f.forge_api_url
                    .as_deref()
                    .map_or(GITHUB_API, norm)
                    .to_string(),
            ),
            None,
            None,
        ),
        RepoAccess::GithubApp {
            installation_id,
            account,
        } => {
            if *installation_id == 0 || i64::try_from(*installation_id).is_err() {
                return Err(CoreError::invalid(
                    "invalid_installation",
                    "an installation ID is a positive whole number",
                ));
            }
            (None, None, Some(*installation_id), Some(account.clone()))
        }
    };
    Ok(Settings {
        base: check_base(f.base.as_deref().unwrap_or("main"))?,
        repo_url,
        forge_api_url,
        protected_paths: check_protected(&f.protected_paths)?,
        preview: RepoPreview {
            url: norm(&f.preview_url).to_string(),
            ready_path: check_ready_path(f.preview_ready_path.as_deref().unwrap_or("/healthz"))?,
            names_commit: f.preview_names_commit.unwrap_or(true),
            timeout_secs,
            poll_secs,
        },
        installation_id,
        account,
        public: f.public,
        allow_open_egress: f.allow_open_egress,
        reviewed_previews: f.reviewed_previews,
        wake_on_reply: f.wake_on_reply,
        ci_accepted: f.ci_accepted,
        repo,
    })
}

/// Checks `s`'s addresses under `policy`.
fn check_addresses(s: &Settings, policy: BaseUrlPolicy) -> Result<()> {
    if let Some(u) = &s.repo_url {
        check_url("invalid_repo_url", u, policy)?;
    }
    if let Some(u) = &s.forge_api_url {
        check_url("invalid_forge_url", u, policy)?;
    }
    check_preview_url(&s.preview.url, policy)?;
    Ok(())
}

/// 400 `forge_not_instance` unless `f`'s URLs, if it gives any, are the ones
/// `app` gives `repo`: an App's tokens go to the instance's forge alone.
fn check_app_urls(f: &RepoLinkFields, repo: &str, app: &AppRow) -> Result<()> {
    let same =
        |given: &Option<String>, want: &str| given.as_deref().is_none_or(|g| norm(g) == want);
    if same(&f.repo_url, &repo_url_on(&app.api_url, repo)) && same(&f.forge_api_url, &app.api_url) {
        return Ok(());
    }
    Err(CoreError::invalid(
        "forge_not_instance",
        "GitHub App access works only against the instance's forge; use a token for any other",
    ))
}

/// Whether `new`'s addresses are all the owner's: each either as it was and
/// the owner's then, or typed anew by the owner now. One address another
/// person left does not become the owner's because they saved near it.
fn owner_addresses_of(old: Option<&LinkRow>, new: &Settings, by_owner: bool) -> bool {
    let Some(o) = old else {
        return by_owner;
    };
    let (was, now) = (o.settings.addresses(), new.addresses());
    [(was.0, now.0), (was.1, now.1), (Some(was.2), Some(now.2))]
        .into_iter()
        .all(|(a, b)| match (a == b, b) {
            (true, None) => true,
            (true, Some(_)) => o.owner_addresses,
            (false, _) => by_owner,
        })
}

/// The refusal of a write that read another state of the link.
fn link_changed() -> CoreError {
    CoreError::invalid(
        "link_changed",
        "this link changed since it was read: read it again",
    )
}

/// The status of `row` on `c`.
fn status_in(
    st: &Store,
    c: &Connection,
    qid: &str,
    row: &LinkRow,
    app: Option<&AppRow>,
) -> Result<LinkStatus> {
    let s = &row.settings;
    let owner_decides = if s.installation_id.is_none() {
        if meta_in(c, &SecretSlot::RepoWrite(qid.into()))?.is_none() {
            return Ok(LinkStatus::NeedsToken);
        }
        if !s.public && meta_in(c, &SecretSlot::RepoRead(qid.into()))?.is_none() {
            return Ok(LinkStatus::NeedsReadToken);
        }
        token_approval_in(st, c)?
    } else {
        if app.is_none() || meta_in(c, &SecretSlot::GithubApp)?.is_none() {
            return Ok(LinkStatus::NeedsApp);
        }
        true
    };
    Ok(if !s.public && !s.ci_accepted {
        LinkStatus::NeedsCiAcceptance
    } else if owner_decides && row.approved_by.is_none() {
        LinkStatus::NeedsApproval
    } else {
        LinkStatus::Ready
    })
}

/// `row` as a person reads it.
fn view_in(st: &Store, c: &Connection, qid: &str, row: LinkRow) -> Result<RepoLink> {
    let app = app_in(c)?;
    let status = status_in(st, c, qid, &row, app.as_ref())?;
    let (repo_url, forge_api_url) = urls_of(&row.settings, app.as_ref());
    let local_preview = policy_for(st, row.owner_addresses) == BaseUrlPolicy::Local;
    let access = match row.settings.installation_id {
        Some(installation_id) => LinkAccess::GithubApp {
            installation_id,
            account: row.settings.account.clone().unwrap_or_default(),
            approved_by: row.approved_by,
            approved_at: row.approved_at,
        },
        None => LinkAccess::Token {
            read_token: meta_in(c, &SecretSlot::RepoRead(qid.into()))?.map(Box::new),
            write_token: meta_in(c, &SecretSlot::RepoWrite(qid.into()))?.map(Box::new),
            approved_by: row.approved_by,
            approved_at: row.approved_at,
        },
    };
    let Settings {
        repo,
        base,
        protected_paths,
        preview,
        public,
        allow_open_egress,
        reviewed_previews,
        wake_on_reply,
        ci_accepted,
        ..
    } = row.settings;
    Ok(RepoLink {
        queue_id: qid.to_string(),
        rev: row.rev,
        repo,
        base,
        repo_url,
        forge_api_url,
        protected_paths,
        preview,
        access,
        public,
        allow_open_egress,
        reviewed_previews,
        wake_on_reply,
        ci_accepted,
        status,
        last_ok_at: row.last_ok_at,
        last_failure: row.last_failure,
        last_failure_at: row.last_failure_at,
        local_preview,
        set_by: row.set_by,
        set_at: row.set_at,
    })
}

fn repo_record(at: &str, qid: &str, op: &str) -> AuditRecord {
    AuditRecord::new(AuditKind::QueueUpdate, at)
        .with("queue_id", qid)
        .with("op", op)
}

/// Clears both of queue `qid`'s tokens on `tx`, recorded by each slot.
/// Returns whether either was set.
fn clear_tokens_in(st: &Store, tx: &Transaction<'_>, ctx: &AuditCtx, qid: &str) -> Result<bool> {
    let read = clear_in(st, tx, ctx, &SecretSlot::RepoRead(qid.into()))?;
    let write = clear_in(st, tx, ctx, &SecretSlot::RepoWrite(qid.into()))?;
    Ok(read || write)
}

/// Removes queue `qid`'s link and its tokens on `tx`, recorded as
/// `queue.update {queue_id, op: "repo_clear", reason?}` when it had a link.
/// Returns whether it did.
fn remove_because_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    qid: &str,
    reason: Option<&str>,
) -> Result<bool> {
    clear_tokens_in(st, tx, ctx, qid)?;
    let gone = tx.execute("DELETE FROM queue_repos WHERE queue_id = ?1", params![qid])? > 0;
    if gone {
        let mut rec = repo_record(&st.now(), qid, "repo_clear");
        if let Some(r) = reason {
            rec = rec.with("reason", r);
        }
        st.record_audit(tx, ctx, rec)?;
    }
    Ok(gone)
}

/// [`remove_because_in`], for no reason beyond someone asking.
pub(crate) fn remove_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    qid: &str,
) -> Result<bool> {
    remove_because_in(st, tx, ctx, qid, None)
}

/// Whether queue `qid`, made by `maker`, has an admin who can still reach
/// it: the owner's queue always does.
fn has_live_admin_in(c: &Connection, qid: &str, maker: Option<&str>) -> Result<bool> {
    let Some(maker) = maker else {
        return Ok(true);
    };
    Ok(c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM viewers v WHERE (v.owner = 1 OR (v.person = 1
                AND v.person_revoked_at IS NULL)) AND (v.public_id = ?2 OR v.public_id IN
                (SELECT person FROM queue_members WHERE queue_id = ?1 AND role = 'admin')))",
    )?
    .query_row(params![qid, maker], |r| r.get(0))?)
}

/// Removes the links on the queues `person`, just revoked, was an admin of
/// and that no live admin is left on, on `tx`: nobody but the owner could
/// reach them, and they would go on working. Each is recorded as
/// `queue.update {queue_id, op: "repo_clear", reason: "no_live_admin"}`.
pub(crate) fn remove_orphans_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    person: &str,
) -> Result<()> {
    let queues: Vec<(String, Option<String>)> = tx
        .prepare_cached(
            "SELECT q.id, q.person FROM queue_repos r JOIN queues q ON q.id = r.queue_id
              WHERE q.deleted_at IS NULL AND (q.person = ?1 OR q.id IN
                (SELECT queue_id FROM queue_members WHERE person = ?1 AND role = 'admin'))",
        )?
        .query_map(params![person], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (qid, maker) in queues {
        if !has_live_admin_in(tx, &qid, maker.as_deref())? {
            remove_because_in(st, tx, ctx, &qid, Some("no_live_admin"))?;
        }
    }
    Ok(())
}

/// `pem` trimmed, when it is a PEM private key.
fn check_app_key(pem: &str) -> Result<&str> {
    let p = pem.trim();
    let ok = p.len() <= MAX_APP_KEY_BYTES
        && p.is_ascii()
        && p.starts_with("-----BEGIN ")
        && p.ends_with("PRIVATE KEY-----")
        && p.contains("PRIVATE KEY-----\n");
    if !ok {
        return Err(CoreError::invalid(
            "invalid_app_key",
            "the App's key is the PEM document GitHub gave you, from -----BEGIN to its END line",
        ));
    }
    Ok(p)
}

/// `token` trimmed, checked as [`check_key`] does and at least
/// [`MIN_TOKEN_BYTES`] long.
fn check_token(token: &str) -> Result<&str> {
    let t = check_key(token)?;
    if t.len() < MIN_TOKEN_BYTES {
        return Err(CoreError::invalid(
            "invalid_key",
            format!("a repository token is at least {MIN_TOKEN_BYTES} characters"),
        ));
    }
    Ok(t)
}

impl Store {
    /// Queue `qid`'s link, or `None` when it has none. No secret. This is
    /// the metadata read; [`Store::open_queue_repo`] opens credentials.
    ///
    /// # Errors
    /// The store's.
    pub fn queue_repo(&self, qid: &str) -> Result<Option<RepoLink>> {
        self.with_read(|c| {
            link_in(c, qid)?
                .map(|row| view_in(self, c, qid, row))
                .transpose()
        })
    }

    /// Sets queue `qid`'s link as `ctx`, whom the caller has checked is one
    /// of its admins and, with `by_owner`, the instance's owner. `f.if_rev`
    /// is the `rev` the change was made from, `None` for no link. Recorded
    /// as `queue.update {queue_id, op: "repo", fields, repo, base, access,
    /// repo_url, forge_api_url, installation_id, installation_account,
    /// protected_paths,
    /// preview_url, preview_ready_path, preview_names_commit,
    /// preview_timeout_secs, preview_poll_secs, public, allow_open_egress,
    /// reviewed_previews, wake_on_reply, ci_accepted, approved,
    /// tokens_cleared}`, `fields` naming what changed; a change to nothing
    /// records nothing.
    ///
    /// A change to the link's repository, either URL or its installation
    /// clears its tokens, which were set for the old one, voids its approval
    /// and leaves `ci_accepted` false whatever `f` says, since that was said
    /// of the old one too. The owner's change to one of those, or a new link
    /// of theirs, approves it; any other save leaves approval as it was, but
    /// that anyone else's change to `public`, `allow_open_egress`,
    /// `reviewed_previews`, `wake_on_reply`, `preview_url`,
    /// `preview_ready_path`, `protected_paths` or `ci_accepted`, either way,
    /// voids it.
    ///
    /// Addresses as they were answer to the rule of who set them, so an
    /// admin may save the rest of a form holding the owner's loopback URL;
    /// changing any one holds all of them to the caller's rule.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `link_changed` when
    /// `f.if_rev` is not the link's `rev`; `invalid_repo`, `invalid_base`,
    /// `invalid_repo_url`, `invalid_forge_url`, `forge_required`,
    /// `repo_url_mismatch`,
    /// `invalid_preview_url`, `invalid_ready_path`,
    /// `invalid_preview_timing`, `invalid_protected_path` or
    /// `invalid_installation` for bad input; `no_github_app` for `github_app`
    /// access with no App; `forge_not_instance` for `github_app` access
    /// naming any other forge; the store's.
    pub fn set_queue_repo(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        f: &RepoLinkFields,
        by_owner: bool,
    ) -> Result<RepoLink> {
        let new = settings_of(f)?;
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let old = link_in(tx, qid)?;
            if f.if_rev.as_deref() != old.as_ref().map(|o| o.rev.as_str()) {
                return Err(link_changed());
            }
            let app = app_in(tx)?;
            let owner_addresses = owner_addresses_of(old.as_ref(), &new, by_owner);
            check_addresses(&new, policy_for(self, owner_addresses))?;
            check_forge_named(f)?;
            if new.installation_id.is_some() {
                let a = app.as_ref().ok_or_else(|| {
                    CoreError::invalid("no_github_app", "this instance has no GitHub App")
                })?;
                check_app_urls(f, &new.repo, a)?;
            }
            if let Some(u) = &new.repo_url {
                check_repo_path(u, &new.repo, app.as_ref())?;
            }
            let rebound = old
                .as_ref()
                .is_none_or(|o| o.settings.binding() != new.binding());
            // An acceptance speaks of one repository's CI, and a form that
            // still holds it has not said it of the next.
            let new = Settings {
                ci_accepted: new.ci_accepted && (old.is_none() || !rebound),
                ..new.clone()
            };
            // An approval was given for what the owner read: nobody else may
            // change that under it.
            let same_terms = old.as_ref().is_some_and(|o| new.same_terms(&o.settings));
            let now = self.now();
            let (approved_by, approved_at) = match &old {
                Some(o) if !rebound && (by_owner || same_terms) => {
                    (o.approved_by.clone(), o.approved_at.clone())
                }
                _ if by_owner => (Some(set_by(ctx)), Some(now.clone())),
                _ => (None, None),
            };
            if let Some(o) = &old
                && o.settings == new
            {
                return view_in(self, tx, qid, o.clone());
            }
            let tokens_cleared = rebound && old.is_some() && clear_tokens_in(self, tx, ctx, qid)?;
            let paths = serde_json::to_string(&new.protected_paths).expect("strings serialise");
            tx.execute(
                "INSERT INTO queue_repos (queue_id, repo, base, repo_url, forge_api_url,
                    protected_paths_json, preview_url, preview_ready_path, preview_names_commit,
                    preview_timeout_secs, preview_poll_secs, access, installation_id, approved_by,
                    approved_at, set_by, set_at, rev, owner_addresses, public, allow_open_egress,
                    reviewed_previews, wake_on_reply, installation_account, ci_accepted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                    ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
                 ON CONFLICT (queue_id) DO UPDATE SET repo = excluded.repo, base = excluded.base,
                    repo_url = excluded.repo_url, forge_api_url = excluded.forge_api_url,
                    protected_paths_json = excluded.protected_paths_json,
                    preview_url = excluded.preview_url,
                    preview_ready_path = excluded.preview_ready_path,
                    preview_names_commit = excluded.preview_names_commit,
                    preview_timeout_secs = excluded.preview_timeout_secs,
                    preview_poll_secs = excluded.preview_poll_secs, access = excluded.access,
                    installation_id = excluded.installation_id, approved_by = excluded.approved_by,
                    approved_at = excluded.approved_at, set_by = excluded.set_by,
                    set_at = excluded.set_at, rev = excluded.rev,
                    owner_addresses = excluded.owner_addresses, public = excluded.public,
                    allow_open_egress = excluded.allow_open_egress,
                    reviewed_previews = excluded.reviewed_previews,
                    wake_on_reply = excluded.wake_on_reply,
                    installation_account = excluded.installation_account,
                    ci_accepted = excluded.ci_accepted,
                    last_ok_at = CASE WHEN ?26 THEN NULL ELSE last_ok_at END,
                    last_failure = NULL, last_failure_at = NULL",
                params![
                    qid,
                    new.repo,
                    new.base,
                    new.repo_url,
                    new.forge_api_url,
                    paths,
                    new.preview.url,
                    new.preview.ready_path,
                    new.preview.names_commit,
                    new.preview.timeout_secs,
                    new.preview.poll_secs,
                    new.access(),
                    new.installation_id,
                    approved_by,
                    approved_at,
                    set_by(ctx),
                    now,
                    new_ulid(),
                    owner_addresses,
                    new.public,
                    new.allow_open_egress,
                    new.reviewed_previews,
                    new.wake_on_reply,
                    new.account,
                    new.ci_accepted,
                    rebound,
                ],
            )?;
            let rec = repo_record(&now, qid, "repo")
                .with(
                    "fields",
                    new.changed_from(old.as_ref().map(|o| &o.settings)),
                )
                .with("repo", new.repo.as_str())
                .with("base", new.base.as_str())
                .with("access", new.access())
                .with("repo_url", new.repo_url.as_deref())
                .with("forge_api_url", new.forge_api_url.as_deref())
                .with("installation_id", new.installation_id)
                .with("installation_account", new.account.as_deref())
                .with("protected_paths", new.protected_paths.clone())
                .with("preview_url", new.preview.url.as_str())
                .with("preview_ready_path", new.preview.ready_path.as_str())
                .with("preview_names_commit", new.preview.names_commit)
                .with("preview_timeout_secs", new.preview.timeout_secs)
                .with("preview_poll_secs", new.preview.poll_secs)
                .with("public", new.public)
                .with("allow_open_egress", new.allow_open_egress)
                .with("reviewed_previews", new.reviewed_previews)
                .with("wake_on_reply", new.wake_on_reply)
                .with("ci_accepted", new.ci_accepted)
                .with("approved", approved_by.is_some())
                .with("tokens_cleared", tokens_cleared);
            self.record_audit(tx, ctx, rec)?;
            let row = link_in(tx, qid)?.expect("the link was just written");
            view_in(self, tx, qid, row)
        })
    }

    /// Removes queue `qid`'s link and its tokens, recorded as `queue.update
    /// {queue_id, op: "repo_clear"}` and each token's `admin.key`. Returns
    /// whether it had a link.
    ///
    /// # Errors
    /// The store's.
    pub fn clear_queue_repo(&self, ctx: &AuditCtx, qid: &str) -> Result<bool> {
        self.with_tx(|tx| remove_in(self, tx, ctx, qid))
    }

    /// [`Store::clear_queue_repo`], for the link as read at `if_rev`: a link
    /// already gone is fine.
    ///
    /// # Errors
    /// `link_changed` when the link is not at `if_rev`; the store's.
    pub fn clear_queue_repo_at(&self, ctx: &AuditCtx, qid: &str, if_rev: &str) -> Result<bool> {
        self.with_tx(|tx| {
            if link_in(tx, qid)?.is_some_and(|l| l.rev != if_rev) {
                return Err(link_changed());
            }
            remove_in(self, tx, ctx, qid)
        })
    }

    /// Seals `token` as queue `qid`'s `kind` token, bound to its link's
    /// `repo_url` and `forge_api_url`, for the link as read at `if_rev`, so that a token goes
    /// only where its person saw it would. Recorded as `admin.key {scope,
    /// op: set, last4, queue_id}`. The read and write tokens are never the
    /// same value: a container gets the read one.
    ///
    /// When the other token does not open with `kek`, the key-encryption key
    /// has changed since it was set: it is unusable, nobody can compare
    /// it, and this one is sealed all the same. The other must then be set
    /// again before [`Store::open_queue_repo`] opens the link.
    ///
    /// # Errors
    /// `no_repo_link` when the queue has no link; `link_changed` when
    /// `if_rev` is not its `rev`; `wrong_access` when it is not `token`
    /// access; `invalid_key`; `same_token` for the value of the other
    /// token; the store's.
    pub fn set_queue_repo_token(
        &self,
        ctx: &AuditCtx,
        kek: &Kek,
        qid: &str,
        kind: TokenKind,
        token: &str,
        if_rev: &str,
    ) -> Result<RepoLink> {
        let token = check_token(token)?;
        self.with_tx(|tx| {
            let row = token_link_in(tx, qid)?;
            if row.rev != if_rev {
                return Err(link_changed());
            }
            let other = match open_in(tx, kek, &kind.other().slot(qid)) {
                Err(CoreError::Invalid {
                    code: "kek_mismatch",
                    ..
                }) => None,
                opened => opened?,
            };
            if other.is_some_and(|o| o.expose() == token) {
                return Err(CoreError::invalid(
                    "same_token",
                    "the read and write tokens must differ: a container gets the read token, and never one that can write",
                ));
            }
            let host = row
                .settings
                .repo_url
                .as_deref()
                .and_then(clax_core::config::key_host);
            let forge_host = row
                .settings
                .forge_api_url
                .as_deref()
                .and_then(clax_core::config::key_host);
            let last4 = last4_of(token);
            put_in(
                self,
                tx,
                ctx,
                kek,
                &kind.slot(qid),
                &Sealing {
                    value: token,
                    shown: &last4,
                    host: host.as_deref(),
                    forge_host: forge_host.as_deref(),
                },
            )?;
            token_changed_in(tx, qid, kind)?;
            let row = link_in(tx, qid)?.expect("the link was just read");
            view_in(self, tx, qid, row)
        })
    }

    /// Clears queue `qid`'s `kind` token, recorded as `admin.key {scope, op:
    /// clear, last4, queue_id}` when it held one. What it failed at, and with
    /// the write token what it worked at, goes with it.
    ///
    /// # Errors
    /// `no_repo_link`, `wrong_access` as [`Store::set_queue_repo_token`];
    /// the store's.
    pub fn clear_queue_repo_token(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        kind: TokenKind,
    ) -> Result<RepoLink> {
        self.with_tx(|tx| {
            token_link_in(tx, qid)?;
            if clear_in(self, tx, ctx, &kind.slot(qid))? {
                token_changed_in(tx, qid, kind)?;
            }
            let row = link_in(tx, qid)?.expect("the link was just read");
            view_in(self, tx, qid, row)
        })
    }

    /// Queue `qid`'s link with its credentials opened, for a workspace to
    /// use now: `None` unless the link is [`LinkStatus::Ready`], and
    /// [`Store::queue_repo`] says why not. This is the one way a workspace
    /// slot is read, and it is the workspace's alone: no route answers with
    /// it. Open per run: a credential cleared later is not taken back from
    /// one already open.
    ///
    /// A confidential link opens only under [`Egress::Held`], or with
    /// `allow_open_egress`: `egress` is what the caller's runtime does.
    ///
    /// Each stored URL is held again to the rule of who set it under the
    /// instance's policy: an App's forge is the owner's whoever made the
    /// link, its preview the link's. A credential is held to the origins it
    /// was set for. So a link made before an instance became hosted is
    /// refused here.
    ///
    /// # Errors
    /// `egress_open` for a confidential link no allow-list holds;
    /// `invalid_repo_url`, `invalid_forge_url` or `invalid_preview_url` for
    /// a stored URL the policy now refuses; `key_host_mismatch` for a
    /// credential set for another origin; `kek_mismatch` when one does not
    /// open with `kek`; the store's.
    pub fn open_queue_repo(
        &self,
        kek: &Kek,
        qid: &str,
        egress: Egress,
    ) -> Result<Option<ReadyTarget>> {
        self.with_read(|c| {
            if queue_in(c, qid)?.is_none() {
                return Ok(None);
            }
            let Some(row) = link_in(c, qid)? else {
                return Ok(None);
            };
            let app = app_in(c)?;
            if status_in(self, c, qid, &row, app.as_ref())? != LinkStatus::Ready {
                return Ok(None);
            }
            let s = &row.settings;
            if !s.public && !s.allow_open_egress && egress == Egress::Open {
                return Err(CoreError::invalid(
                    "egress_open",
                    "a confidential repository needs egress held by an allow-list, allow_open_egress, or public",
                ));
            }
            let (repo_url, forge_api_url) = urls_of(s, app.as_ref());
            let link_rule = policy_for(self, row.owner_addresses);
            let forge_rule = if s.installation_id.is_some() {
                self.base_url_policy()
            } else {
                link_rule
            };
            check_url("invalid_repo_url", &repo_url, forge_rule)?;
            check_url("invalid_forge_url", &forge_api_url, forge_rule)?;
            check_preview_url(&s.preview.url, link_rule)?;
            let credentials =
                credentials_in(c, kek, qid, &row, app.as_ref(), &repo_url, &forge_api_url)?;
            let stamp = stamp_in(c, qid, &row)?;
            Ok(Some(ReadyTarget {
                stamp,
                queue_id: qid.to_string(),
                repo: row.settings.repo,
                base: row.settings.base,
                repo_url,
                forge_api_url,
                protected_paths: row.settings.protected_paths,
                preview: row.settings.preview,
                credentials,
                public: row.settings.public,
                reviewed_previews: row.settings.reviewed_previews,
                wake_on_reply: row.settings.wake_on_reply,
                local_forge: forge_rule == BaseUrlPolicy::Local,
                local_preview: link_rule == BaseUrlPolicy::Local,
            }))
        })
    }

    /// Notes that a workspace used queue `qid`'s credentials, those of
    /// `stamp` ([`ReadyTarget::stamp`]), and it worked. A publish means this
    /// link has now worked, the App's key or the write token with it, and
    /// whatever failed before is over. A read proves only that much: any token
    /// that reaches the repository can do it, so it ends what a read failed at
    /// and nothing else. Credentials since replaced, or a link gone meanwhile,
    /// say nothing.
    ///
    /// # Errors
    /// The store's.
    pub fn note_repo_ok(&self, qid: &str, stamp: &str, used: RepoUse) -> Result<()> {
        let slot = self.with_tx(|tx| {
            let Some(row) = link_in(tx, qid)?.filter(|r| is_stamped(tx, qid, r, stamp)) else {
                return Ok(None);
            };
            if used == RepoUse::Read {
                tx.execute(
                    "UPDATE queue_repos SET last_failure = NULL, last_failure_at = NULL
                      WHERE queue_id = ?1 AND last_failure IS NOT ?2",
                    params![qid, PUBLISH_FAILED],
                )?;
                return Ok(None);
            }
            tx.execute(
                "UPDATE queue_repos SET last_ok_at = ?2, last_failure = NULL,
                    last_failure_at = NULL WHERE queue_id = ?1",
                params![qid, self.now()],
            )?;
            Ok(Some(match row.settings.installation_id {
                Some(_) => SecretSlot::GithubApp,
                None => SecretSlot::RepoWrite(qid.into()),
            }))
        })?;
        match slot {
            Some(slot) => self.mark_secret_ok(&slot),
            None => Ok(()),
        }
    }

    /// Which credentials queue `qid`'s link holds as of now; `None` for no
    /// link. It changes with the link, either token and the App's key.
    ///
    /// # Errors
    /// The store's.
    pub fn queue_repo_stamp(&self, qid: &str) -> Result<Option<String>> {
        self.with_read(|c| link_in(c, qid)?.map(|r| stamp_in(c, qid, &r)).transpose())
    }

    /// Notes that a workspace could not use queue `qid`'s link, in `what`,
    /// fixed words of the caller's: this reaches a person's panel, and a
    /// remote's own text can quote anything. A later success or any change
    /// to the link or a token clears it. `stamp` names the credentials used
    /// ([`ReadyTarget::stamp`]): since replaced, they say nothing of the link.
    /// `None` is for credentials that would not open, which are the ones it
    /// holds.
    ///
    /// # Errors
    /// The store's.
    pub fn note_repo_failure(&self, qid: &str, stamp: Option<&str>, what: &str) -> Result<()> {
        self.with_tx(|tx| {
            if let Some(stamp) = stamp
                && !link_in(tx, qid)?.is_some_and(|r| is_stamped(tx, qid, &r, stamp))
            {
                return Ok(());
            }
            tx.execute(
                "UPDATE queue_repos SET last_failure = ?2, last_failure_at = ?3
                  WHERE queue_id = ?1",
                params![qid, what, self.now()],
            )?;
            Ok(())
        })
    }

    /// The instance's GitHub App with its key opened, to ask its forge which
    /// installation holds a repository: `None` until it has an App and a key.
    /// It is for that one question, on behalf of whoever is linking a
    /// repository, and no route answers with it.
    ///
    /// # Errors
    /// `invalid_forge_url` for a forge the instance's policy now refuses;
    /// `key_host_mismatch` for a key set for another forge; `kek_mismatch`
    /// when the key does not open with `kek`; the store's.
    pub fn open_github_app(&self, kek: &Kek) -> Result<Option<OpenedApp>> {
        self.with_read(|c| {
            let Some(app) = app_in(c)? else {
                return Ok(None);
            };
            let rule = self.base_url_policy();
            check_url("invalid_forge_url", &app.api_url, rule)?;
            let opened = open_bound(c, kek, &SecretSlot::GithubApp, &app.api_url, None)?;
            Ok(opened.map(|private_key| OpenedApp {
                app_id: app.app_id,
                api_url: app.api_url,
                private_key,
                local_forge: rule == BaseUrlPolicy::Local,
            }))
        })
    }

    /// The instance's GitHub App, or `None` when it has none. No secret.
    ///
    /// # Errors
    /// The store's.
    pub fn github_app(&self) -> Result<Option<GithubApp>> {
        self.with_read(github_app_in)
    }

    /// Sets the instance's GitHub App, and with `key` its private key,
    /// sealed under `kek` and bound to its forge. The caller has checked
    /// that `ctx` is the owner. Recorded as `admin.provider {fields, values,
    /// approvals_voided}`, the last the number of approvals this ended, and
    /// `admin.key {scope: github_app, ...}` for its key.
    ///
    /// A first App, or one moved to another forge or App ID, clears any key
    /// and voids every App approval, since they were given for another;
    /// each voided link is recorded as `queue.update {op: "repo_approval",
    /// approved: false}`, and is then `needs_app` or `needs_approval`.
    ///
    /// # Errors
    /// `invalid_app` for an App ID of 0 or a slug that is no App's;
    /// `invalid_forge_url`; `invalid_app_key`; the store's.
    pub fn set_github_app(
        &self,
        ctx: &AuditCtx,
        kek: &Kek,
        f: &GithubAppFields,
        key: Option<&str>,
    ) -> Result<GithubApp> {
        let slug = f.slug.trim();
        let slug_ok = !slug.is_empty()
            && slug.len() <= 64
            && !slug.starts_with('-')
            && slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if f.app_id == 0 || i64::try_from(f.app_id).is_err() || !slug_ok {
            return Err(CoreError::invalid(
                "invalid_app",
                "an App has a positive ID, and a slug of letters, digits, '-' and '_'",
            ));
        }
        let api_url = match &f.api_url {
            Some(u) => check_url("invalid_forge_url", u, self.base_url_policy())?,
            None => GITHUB_API.to_string(),
        };
        let key = key.map(check_app_key).transpose()?;
        self.with_tx(|tx| {
            let moved = app_in(tx)?.is_none_or(|o| o.api_url != api_url || o.app_id != f.app_id);
            let now = self.now();
            let voided = if moved {
                clear_in(self, tx, ctx, &SecretSlot::GithubApp)?;
                void_app_approvals_in(self, tx, ctx)?
            } else {
                0
            };
            if moved || key.is_some() {
                forget_app_outcomes_in(tx)?;
            }
            tx.execute(
                "INSERT INTO instance_github_app (id, app_id, slug, api_url, set_by, set_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (id) DO UPDATE SET app_id = excluded.app_id, slug = excluded.slug,
                    api_url = excluded.api_url, set_by = excluded.set_by, set_at = excluded.set_at",
                params![f.app_id, slug, api_url, set_by(ctx), now],
            )?;
            let rec = AuditRecord::new(AuditKind::AdminProvider, now)
                .with("fields", ["github_app"])
                .with(
                    "values",
                    serde_json::json!({"github_app": {
                        "app_id": f.app_id, "slug": slug, "api_url": api_url,
                    }}),
                )
                .with("approvals_voided", voided);
            self.record_audit(tx, ctx, rec)?;
            if let Some(pem) = key {
                let digest = clax_core::secret::sha256_hex(pem);
                put_in(
                    self,
                    tx,
                    ctx,
                    kek,
                    &SecretSlot::GithubApp,
                    &Sealing {
                        value: pem,
                        shown: &digest[..4],
                        host: clax_core::config::key_host(&api_url).as_deref(),
                        forge_host: None,
                    },
                )?;
            }
            Ok(github_app_in(tx)?.expect("the App was just written"))
        })
    }

    /// Removes the instance's GitHub App and its key, and voids every App
    /// approval, each recorded as [`Store::set_github_app`] records one.
    /// Links stay, `needs_app`. Recorded as `admin.provider {fields, values,
    /// approvals_voided}`. Returns whether there was one.
    ///
    /// # Errors
    /// The store's.
    pub fn clear_github_app(&self, ctx: &AuditCtx) -> Result<bool> {
        self.with_tx(|tx| {
            clear_in(self, tx, ctx, &SecretSlot::GithubApp)?;
            let voided = void_app_approvals_in(self, tx, ctx)?;
            forget_app_outcomes_in(tx)?;
            let gone = tx.execute("DELETE FROM instance_github_app", [])? > 0;
            if gone {
                let rec = AuditRecord::new(AuditKind::AdminProvider, self.now())
                    .with("fields", ["github_app"])
                    .with("values", serde_json::json!({"github_app": null}))
                    .with("approvals_voided", voided);
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(gone)
        })
    }

    /// Every live queue's link, whoever made it, those waiting on the owner
    /// first, then by when each was set. No secret.
    ///
    /// # Errors
    /// The store's.
    pub fn repo_links(&self) -> Result<Vec<RepoLine>> {
        self.with_read(|c| {
            let queues: Vec<(String, String, Option<String>)> = c
                .prepare_cached(
                    "SELECT q.id, q.name, q.person FROM queue_repos r
                       JOIN queues q ON q.id = r.queue_id WHERE q.deleted_at IS NULL",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let token_approval = token_approval_in(self, c)?;
            let mut out = Vec::with_capacity(queues.len());
            for (qid, queue_name, maker) in queues {
                let row = link_in(c, &qid)?.ok_or(CoreError::NotFound)?;
                let maker = maker.unwrap_or_else(|| OWNER.to_string());
                let mut admins: Vec<String> = c
                    .prepare_cached(
                        "SELECT person FROM queue_members WHERE queue_id = ?1 AND role = 'admin'
                          ORDER BY created_at, person",
                    )?
                    .query_map(params![qid], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                if !admins.contains(&maker) {
                    admins.insert(0, maker.clone());
                }
                let installation_id = row.settings.installation_id;
                let account = row.settings.account.clone();
                let view = view_in(self, c, &qid, row.clone())?;
                out.push(RepoLine {
                    queue_id: qid,
                    queue_name,
                    access: row.settings.access(),
                    installation_id,
                    account,
                    approved: row.approved_by.is_some(),
                    approvable: installation_id.is_some() || token_approval,
                    public: view.public,
                    allow_open_egress: view.allow_open_egress,
                    reviewed_previews: view.reviewed_previews,
                    wake_on_reply: view.wake_on_reply,
                    preview_url: view.preview.url,
                    preview_ready_path: view.preview.ready_path,
                    protected_paths: view.protected_paths,
                    ci_accepted: view.ci_accepted,
                    maker,
                    admins,
                    repo: view.repo,
                    repo_url: view.repo_url,
                    forge_api_url: view.forge_api_url,
                    status: view.status,
                    rev: view.rev,
                    set_by: view.set_by,
                    set_at: view.set_at,
                });
            }
            out.sort_by(|a, b| {
                (
                    a.status != LinkStatus::NeedsApproval,
                    &a.set_at,
                    &a.queue_id,
                )
                    .cmp(&(
                        b.status != LinkStatus::NeedsApproval,
                        &b.set_at,
                        &b.queue_id,
                    ))
            });
            Ok(out)
        })
    }

    /// Whether a token link waits for the owner's approval, as
    /// [`LinkStatus::NeedsApproval`] does. Until the owner says, on any
    /// instance but a local daemon.
    ///
    /// # Errors
    /// The store's.
    pub fn repo_token_approval(&self) -> Result<bool> {
        self.with_read(|c| token_approval_in(self, c))
    }

    /// Says whether a token link waits for the owner's approval, as `ctx`,
    /// whom the caller has checked is the owner. Recorded as
    /// `admin.provider {fields: ["token_approval"], values: {token_approval},
    /// links_released}`, the last how many token links this lets run with no
    /// approval.
    ///
    /// # Errors
    /// The store's.
    pub fn set_repo_token_approval(&self, ctx: &AuditCtx, on: bool) -> Result<()> {
        self.with_tx(|tx| {
            let now = self.now();
            let released: i64 = if !on && token_approval_in(self, tx)? {
                tx.query_row(
                    "SELECT COUNT(*) FROM queue_repos r JOIN queues q ON q.id = r.queue_id
                      WHERE q.deleted_at IS NULL AND r.access = 'token'
                        AND r.approved_by IS NULL AND r.queue_id IN
                          (SELECT person FROM sealed_secrets WHERE scope = 'repo_write')
                        AND (r.public = 1 OR (r.ci_accepted = 1 AND r.queue_id IN
                          (SELECT person FROM sealed_secrets WHERE scope = 'repo_read')))",
                    [],
                    |r| r.get(0),
                )?
            } else {
                0
            };
            tx.execute(
                "INSERT INTO instance_repo_policy (id, token_approval, set_by, set_at)
                 VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT (id) DO UPDATE SET token_approval = excluded.token_approval,
                    set_by = excluded.set_by, set_at = excluded.set_at",
                params![on, set_by(ctx), now],
            )?;
            let rec = AuditRecord::new(AuditKind::AdminProvider, now)
                .with("fields", ["token_approval"])
                .with("values", serde_json::json!({"token_approval": on}))
                .with("links_released", released);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Approves or, with `approved` false, withdraws queue `qid`'s link, as
    /// `ctx`, whom the caller has checked is the owner. `rev` is the link as
    /// the owner read it, so an approval covers only what they saw.
    /// Recorded as `queue.update {queue_id, op: "repo_approval", repo,
    /// installation_id, approved}`.
    ///
    /// # Errors
    /// `no_repo_link` when the queue has no link; `link_changed` when it is
    /// not at `rev`; `no_github_app` for an App link with no App; the
    /// store's.
    pub fn approve_queue_repo(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        rev: &str,
        approved: bool,
    ) -> Result<()> {
        self.with_tx(|tx| {
            let row = link_in(tx, qid)?.ok_or_else(no_link)?;
            if row.rev != rev {
                return Err(link_changed());
            }
            if approved && row.settings.installation_id.is_some() && app_in(tx)?.is_none() {
                return Err(CoreError::invalid(
                    "no_github_app",
                    "this instance has no GitHub App",
                ));
            }
            if row.approved_by.is_some() == approved {
                return Ok(());
            }
            let now = self.now();
            let (by, at) = if approved {
                (Some(set_by(ctx)), Some(now.clone()))
            } else {
                (None, None)
            };
            tx.execute(
                "UPDATE queue_repos SET approved_by = ?2, approved_at = ?3 WHERE queue_id = ?1",
                params![qid, by, at],
            )?;
            let rec = repo_record(&now, qid, "repo_approval")
                .with("repo", row.settings.repo.as_str())
                .with("installation_id", row.settings.installation_id)
                .with("approved", approved);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }
}

/// [`ReadyTarget::stamp`] of queue `qid`'s link `row` as of now.
fn stamp_in(c: &Connection, qid: &str, row: &LinkRow) -> Result<String> {
    let slots = match row.settings.installation_id {
        Some(_) => vec![SecretSlot::GithubApp],
        None => vec![TokenKind::Write.slot(qid), TokenKind::Read.slot(qid)],
    };
    let mut stamp = row.rev.clone();
    for slot in &slots {
        if let Some(m) = meta_in(c, slot)? {
            stamp.push_str(&format!("|{}:{}", m.set_at, m.last4));
        }
    }
    Ok(stamp)
}

/// Whether link `row` still holds the credentials of `stamp`. One that cannot
/// be told is taken not to.
fn is_stamped(c: &Connection, qid: &str, row: &LinkRow, stamp: &str) -> bool {
    stamp_in(c, qid, row).is_ok_and(|s| s == stamp)
}

/// Queue `qid`'s `kind` token is another now, or gone: what failed is over,
/// and what worked with a write token says nothing of the next. A read token
/// has no part in publishing, so what publishing failed at stands through it.
fn token_changed_in(tx: &Transaction<'_>, qid: &str, kind: TokenKind) -> Result<()> {
    tx.execute(
        "UPDATE queue_repos SET
            last_failure_at = CASE WHEN ?2 OR last_failure IS NOT ?3 THEN NULL
                                   ELSE last_failure_at END,
            last_failure = CASE WHEN ?2 OR last_failure IS NOT ?3 THEN NULL
                                ELSE last_failure END,
            last_ok_at = CASE WHEN ?2 THEN NULL ELSE last_ok_at END
          WHERE queue_id = ?1",
        params![qid, kind == TokenKind::Write, PUBLISH_FAILED],
    )?;
    Ok(())
}

/// The App's key or forge is another now, or gone: no App link has yet worked
/// or failed with what it has.
fn forget_app_outcomes_in(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "UPDATE queue_repos SET last_ok_at = NULL, last_failure = NULL, last_failure_at = NULL
          WHERE access = 'github_app'",
        [],
    )?;
    Ok(())
}

fn no_link() -> CoreError {
    CoreError::invalid("no_repo_link", "this queue has no repository link")
}

/// Queue `qid`'s link, which must be `token` access.
fn token_link_in(c: &Connection, qid: &str) -> Result<LinkRow> {
    let row = link_in(c, qid)?.ok_or_else(no_link)?;
    if row.settings.installation_id.is_some() {
        return Err(CoreError::invalid(
            "wrong_access",
            "this link uses the GitHub App, which takes no tokens",
        ));
    }
    Ok(row)
}

/// Voids every App link's approval on `tx`, each recorded as `queue.update
/// {queue_id, op: "repo_approval", repo, installation_id, approved: false}`.
/// Token links' stay: they do not hang on the App. Each gets a new `rev`, so
/// an approval read before this does not land after it. Returns how many.
fn void_app_approvals_in(st: &Store, tx: &Transaction<'_>, ctx: &AuditCtx) -> Result<usize> {
    let voided: Vec<(String, String, u64)> = tx
        .prepare_cached(
            "SELECT queue_id, repo, installation_id FROM queue_repos
              WHERE access = 'github_app' AND approved_by IS NOT NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (qid, repo, installation_id) in &voided {
        tx.execute(
            "UPDATE queue_repos SET approved_by = NULL, approved_at = NULL, rev = ?2
              WHERE queue_id = ?1",
            params![qid, new_ulid()],
        )?;
        let rec = repo_record(&st.now(), qid, "repo_approval")
            .with("repo", repo.as_str())
            .with("installation_id", *installation_id)
            .with("approved", false);
        st.record_audit(tx, ctx, rec)?;
    }
    Ok(voided.len())
}

fn github_app_in(c: &Connection) -> Result<Option<GithubApp>> {
    let Some(a) = app_in(c)? else {
        return Ok(None);
    };
    Ok(Some(GithubApp {
        install_url: format!(
            "{}/apps/{}/installations/new",
            forge_web(&a.api_url),
            a.slug
        ),
        key: meta_in(c, &SecretSlot::GithubApp)?,
        app_id: a.app_id,
        slug: a.slug,
        api_url: a.api_url,
        set_by: a.set_by,
        set_at: a.set_at,
    }))
}

/// `slot`'s value with `kek`, which must be set for `target`'s origin and,
/// for a token, for `forge`'s.
fn open_bound(
    c: &Connection,
    kek: &Kek,
    slot: &SecretSlot,
    target: &str,
    forge: Option<&str>,
) -> Result<Option<Secret>> {
    let Some(meta) = meta_in(c, slot)? else {
        return Ok(None);
    };
    if meta.host != clax_core::config::key_host(target)
        || meta.forge_host != forge.and_then(clax_core::config::key_host)
    {
        return Err(CoreError::invalid(
            "key_host_mismatch",
            "this credential was set for another address; set it again",
        ));
    }
    open_in(c, kek, slot)
}

/// A ready link's credentials.
fn credentials_in(
    c: &Connection,
    kek: &Kek,
    qid: &str,
    row: &LinkRow,
    app: Option<&AppRow>,
    repo_url: &str,
    forge_api_url: &str,
) -> Result<RepoCredentials> {
    let gone = || CoreError::invalid("no_repo_link", "the link's credentials are gone");
    if let (Some(installation_id), Some(a)) = (row.settings.installation_id, app) {
        return Ok(RepoCredentials::GithubApp {
            app_id: a.app_id,
            installation_id,
            private_key: open_bound(c, kek, &SecretSlot::GithubApp, forge_api_url, None)?
                .ok_or_else(gone)?,
        });
    }
    Ok(RepoCredentials::Token {
        read: open_bound(
            c,
            kek,
            &SecretSlot::RepoRead(qid.into()),
            repo_url,
            Some(forge_api_url),
        )?,
        write: open_bound(
            c,
            kek,
            &SecretSlot::RepoWrite(qid.into()),
            repo_url,
            Some(forge_api_url),
        )?
        .ok_or_else(gone)?,
    })
}

#[cfg(test)]
mod tests;
