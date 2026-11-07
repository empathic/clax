//! A queue's notifiers and their outbox (migration 66): how a person is told
//! that a report needs them, through a route any product can use. Zulip is
//! the first kind.
//!
//! An event is a row of `queue_notices`, written in the transaction of the
//! change it tells of, one per notifier, fix, event and link, so none is lost
//! and none is made twice:
//!
//! - [`NoticeEvent::FixToCheck`]: a fix carries a link its reporter can use,
//!   whoever proposed it, and again when that link changes.
//! - [`NoticeEvent::FixHeld`]: a fix is held for a person. A held fix whose
//!   preview may yet come up alone asks nobody; one from a confidential run,
//!   on a link where a person approves each deploy, whose target names what a
//!   person does to deploy its preview, or whose preview is up with no person
//!   having let it out, does.
//! - [`NoticeEvent::FixWaitEnded`]: the wait for a held fix's preview ended
//!   with none seen, so nothing watches for one now and a preview that is up
//!   or comes up later reaches nobody. It stands in for a `FixHeld` not yet sent, whose
//!   words the end of the wait made false.
//! - [`NoticeEvent::FixRejected`]: a reopen said a fix does not work, whatever
//!   the report's target does next.
//!
//! What asks a person of a held fix goes only to a notifier there at its
//! proposal, counted from when the notifier was first set, so a save of the
//! same notifier loses nothing. One set later hears of what happens to the fix
//! from then on: a link for its reporter, or a rejection.
//!
//! Delivery is the host's (`clax_server::notifiers`). It takes what is due
//! ([`Store::take_due_notices`]), which keeps a notice from any other taker
//! for [`LEASE_SECS`], and answers [`Store::notice_sent`],
//! [`Store::retry_notice`] or [`Store::drop_notice`]. A taker that dies
//! leaves its notice to the next, so one whose message went out and whose
//! answer was lost is told again: Zulip's API has no idempotency key. A
//! notice that has not gone in [`KEEP_SECS`] stops trying, and the panel
//! counts it. One taking goes round the queues, each one's oldest before any
//! one's second, so a backlog on one site keeps no other queue waiting.
//!
//! The site is a place the host will contact, held to the rule of a queue's
//! repository link: `https://` to a public name, or for the owner of a local
//! daemon `https://` to any host or `http://` on loopback ([`check_url`]).
//! That is asked at every save and every use, by the instance's rule of the
//! moment, as an address literal reaches no resolver. A name that resolves to
//! a private address is refused at the connection unless [`OpenedZulip::local_site`] says otherwise. The
//! key is sealed in the queue's own slot ([`SecretSlot::NotifierZulip`]), bound
//! to the site's origin: changing it asks for a new key, so nobody who cannot
//! read a key can send it somewhere new. Reads give metadata;
//! [`Store::open_zulip_notifier`] alone opens it. What failed is kept in fixed
//! words its caller chooses, against the `rev` it was tried under, and for
//! each [`NoticeChannel`] apart, so a stream that works hides no failing
//! direct message.

use super::Store;
use super::bot_settings::BaseUrlPolicy;
use super::queue_repos::{check_url, policy_for};
use super::queues::queue_in;
use super::roles::set_by;
use super::secrets::{
    Kek, Sealing, SecretMeta, SecretSlot, check_key, clear_in, last4_of, meta_in, open_in, put_in,
};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::report::NewFix;
use clax_core::secret::Secret;
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Deserialize;
use std::collections::BTreeMap;

/// How long a taken notice is its taker's.
pub const LEASE_SECS: i64 = 120;
/// How long a notice goes on being tried: a week.
pub const KEEP_SECS: i64 = 7 * 86_400;
/// How long a notice that went, or was dropped, is kept: a month, for what
/// stops the same event being told again.
pub const PRUNE_SECS: i64 = 30 * 86_400;
/// The first wait after a failure; it doubles to [`MAX_BACKOFF_SECS`].
const FIRST_BACKOFF_SECS: i64 = 60;
/// The longest wait after a failure, and the most of a server's own.
const MAX_BACKOFF_SECS: i64 = 3600;
/// The most people a notifier maps.
const MAX_PEOPLE: usize = 500;
/// The longest email address accepted, in bytes.
const MAX_EMAIL_BYTES: usize = 254;
/// The longest GitHub login.
const MAX_LOGIN_BYTES: usize = 39;
/// The longest stream name Zulip takes, in characters.
const MAX_STREAM_CHARS: usize = 60;
/// The shortest time between a queue's test messages.
pub const TEST_EVERY_SECS: i64 = 30;

/// The kind `queue_notifiers` calls Zulip.
const ZULIP: &str = "zulip";

/// How a message went out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeChannel {
    Stream,
    Direct,
}

/// What a notice tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeEvent {
    FixToCheck,
    FixHeld,
    FixWaitEnded,
    FixRejected,
}

impl NoticeEvent {
    fn as_str(self) -> &'static str {
        match self {
            NoticeEvent::FixToCheck => "fix_to_check",
            NoticeEvent::FixHeld => "fix_held",
            NoticeEvent::FixWaitEnded => "fix_wait_ended",
            NoticeEvent::FixRejected => "fix_rejected",
        }
    }

    fn parse(s: &str) -> NoticeEvent {
        match s {
            "fix_held" => NoticeEvent::FixHeld,
            "fix_wait_ended" => NoticeEvent::FixWaitEnded,
            "fix_rejected" => NoticeEvent::FixRejected,
            _ => NoticeEvent::FixToCheck,
        }
    }
}

/// A Zulip notifier as a person sets it ([`Store::set_zulip_notifier`]). It
/// has no `Debug`: it may hold a key.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZulipFields {
    /// The organization's address, such as `https://acme.zulipchat.com`.
    pub site: String,
    pub bot_email: String,
    /// Left out, the key set stays, on the same site.
    #[serde(default)]
    pub api_key: Option<String>,
    pub stream: String,
    /// GitHub login to Zulip email.
    #[serde(default)]
    pub people: BTreeMap<String, String>,
    /// The `rev` this was made from; `None` for no notifier.
    #[serde(default)]
    pub if_rev: Option<String>,
}

/// A queue's Zulip notifier as a person reads it: no secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZulipNotifier {
    pub queue_id: String,
    /// New at each change; a write names the one it was made from.
    pub rev: String,
    pub site: String,
    pub bot_email: String,
    pub stream: String,
    /// Lowercase GitHub login to Zulip email.
    pub people: BTreeMap<String, String>,
    pub key: Option<SecretMeta>,
    /// [`OpenedZulip::local_site`].
    pub local_site: bool,
    /// When a message last went, under this `rev`.
    pub last_ok_at: Option<String>,
    /// What failed last on the stream, in fixed words, since no stream message
    /// has gone after it.
    pub last_failure: Option<String>,
    pub last_failure_at: Option<String>,
    /// The same of direct messages.
    pub direct_failure: Option<String>,
    pub direct_failure_at: Option<String>,
    /// Notices still being tried.
    pub pending: u32,
    /// Notices that stopped being tried with none sent.
    pub expired: u32,
    /// The person key of who last changed it.
    pub set_by: String,
    pub set_at: String,
}

/// A notifier with its key ([`Store::open_zulip_notifier`]).
#[derive(Clone, Debug)]
pub struct OpenedZulip {
    pub rev: String,
    pub site: String,
    pub bot_email: String,
    pub api_key: Secret,
    pub stream: String,
    pub people: BTreeMap<String, String>,
    /// Whether the site may be on the daemon's machine or network: its owner
    /// set it, on an instance whose rule allows that now. If not, a name that
    /// resolves to a private address must be refused at the connection.
    pub local_site: bool,
}

/// A notice taken for delivery ([`Store::take_due_notices`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DueNotice {
    pub id: String,
    pub queue_id: String,
    pub thread_id: String,
    /// The fix it is of.
    pub n: u32,
    pub event: NoticeEvent,
    /// The `verify_url` it tells of; empty for any other notice.
    pub link: String,
    /// How many times it has been taken, this one included.
    pub attempts: u32,
}

/// What a run's fix links to, kept apart from what a reporter reads
/// ([`Store::keep_fix_links`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixLinks {
    /// The pull request.
    pub url: Option<String>,
    pub commit: Option<String>,
    /// What deploys its preview, in a sentence of the target's words: none
    /// where the target says nothing or the preview needs no deploying.
    pub preview_step: Option<String>,
}

/// What a notifier's row holds beside its bookkeeping.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
struct ZulipSettings {
    site: String,
    bot_email: String,
    stream: String,
    people: BTreeMap<String, String>,
}

struct NotifierRow {
    rev: String,
    settings: ZulipSettings,
    /// Whether the owner set the site: what it answers to is then the
    /// instance's rule of the moment ([`policy_for`]).
    owner_site: bool,
    set_by: String,
    set_at: String,
    last_ok_at: Option<String>,
    last_failure: Option<String>,
    last_failure_at: Option<String>,
    direct_failure: Option<String>,
    direct_failure_at: Option<String>,
}

fn changed() -> CoreError {
    CoreError::invalid(
        "notifier_changed",
        "this notifier has changed since it was read: read it again",
    )
}

fn notifier_in(c: &Connection, qid: &str) -> Result<Option<NotifierRow>> {
    let row = c
        .prepare_cached(
            "SELECT rev, settings_json, owner_site, set_by, set_at, last_ok_at, last_failure,
                last_failure_at, direct_failure, direct_failure_at
             FROM queue_notifiers WHERE queue_id = ?1 AND kind = ?2",
        )?
        .query_row(params![qid, ZULIP], |r| {
            Ok((
                r.get::<_, String>(1)?,
                NotifierRow {
                    rev: r.get(0)?,
                    settings: ZulipSettings {
                        site: String::new(),
                        bot_email: String::new(),
                        stream: String::new(),
                        people: BTreeMap::new(),
                    },
                    owner_site: r.get(2)?,
                    set_by: r.get(3)?,
                    set_at: r.get(4)?,
                    last_ok_at: r.get(5)?,
                    last_failure: r.get(6)?,
                    last_failure_at: r.get(7)?,
                    direct_failure: r.get(8)?,
                    direct_failure_at: r.get(9)?,
                },
            ))
        })
        .optional()?;
    row.map(|(json, mut n)| {
        n.settings = serde_json::from_str(&json).map_err(|_| CoreError::Corrupt {
            artifact_id: qid.to_string(),
            column: "queue_notifiers.settings_json",
            version: None,
        })?;
        Ok(n)
    })
    .transpose()
}

/// Whether `row`'s site may be on the daemon's machine or network now.
fn local_site(st: &Store, row: &NotifierRow) -> bool {
    policy_for(st, row.owner_site) == BaseUrlPolicy::Local
}

fn view_in(st: &Store, c: &Connection, qid: &str, row: NotifierRow) -> Result<ZulipNotifier> {
    let count = |state: &str| -> Result<u32> {
        Ok(c.prepare_cached(
            "SELECT COUNT(*) FROM queue_notices WHERE queue_id = ?1 AND notifier = ?2
               AND state = ?3",
        )?
        .query_row(params![qid, ZULIP, state], |r| r.get(0))?)
    };
    Ok(ZulipNotifier {
        local_site: local_site(st, &row),
        queue_id: qid.to_string(),
        rev: row.rev,
        key: meta_in(c, &SecretSlot::NotifierZulip(qid.into()))?,
        site: row.settings.site,
        bot_email: row.settings.bot_email,
        stream: row.settings.stream,
        people: row.settings.people,
        last_ok_at: row.last_ok_at,
        last_failure: row.last_failure,
        last_failure_at: row.last_failure_at,
        direct_failure: row.direct_failure,
        direct_failure_at: row.direct_failure_at,
        pending: count("pending")?,
        expired: count("expired")?,
        set_by: row.set_by,
        set_at: row.set_at,
    })
}

/// Whether `s` is an address Zulip could take: one `@`, no space, control
/// character or comma, which would name a second recipient, and no `:`, which
/// would end the user name of the bot's basic credentials.
fn email_ok(s: &str) -> bool {
    s.len() <= MAX_EMAIL_BYTES
        && s.split('@').count() == 2
        && s.split('@').all(|p| !p.is_empty())
        && !s.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, ',' | '"' | '<' | '>' | ':')
        })
}

/// Whether `s` is a GitHub login.
fn login_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_LOGIN_BYTES
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `f`'s settings but its site, checked, its logins lowercase.
fn check_fields(f: &ZulipFields, site: String) -> Result<ZulipSettings> {
    let bot_email = f.bot_email.trim();
    if !email_ok(bot_email) {
        return Err(CoreError::invalid(
            "invalid_bot_email",
            "that is not the bot's email address",
        ));
    }
    let stream = f.stream.trim();
    if stream.is_empty()
        || stream.chars().count() > MAX_STREAM_CHARS
        || stream.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "invalid_stream",
            format!("a stream's name is 1 to {MAX_STREAM_CHARS} characters"),
        ));
    }
    if stream.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CoreError::invalid(
            "invalid_stream",
            "a stream's name of digits alone could be read as its ID: rename the stream",
        ));
    }
    let bad = |why: String| CoreError::invalid("invalid_people", why);
    if f.people.len() > MAX_PEOPLE {
        return Err(bad(format!("a notifier maps at most {MAX_PEOPLE} people")));
    }
    let mut people = BTreeMap::new();
    for (login, email) in &f.people {
        let (login, email) = (login.trim(), email.trim());
        if !login_ok(login) {
            return Err(bad("a person is named by their GitHub login".into()));
        }
        if !email_ok(email) {
            return Err(bad(format!("{login}'s Zulip email is not an address")));
        }
        if people
            .insert(login.to_ascii_lowercase(), email.to_string())
            .is_some()
        {
            return Err(bad(format!("{login} is named twice")));
        }
    }
    Ok(ZulipSettings {
        site,
        bot_email: bot_email.to_string(),
        stream: stream.to_string(),
        people,
    })
}

/// A `queue.update {queue_id, op: "notifier", kind, ...}` record.
fn notifier_record(at: &str, qid: &str) -> AuditRecord {
    AuditRecord::new(AuditKind::QueueUpdate, at)
        .with("queue_id", qid)
        .with("op", "notifier")
        .with("kind", ZULIP)
}

/// Removes queue `qid`'s notifier on `tx`: its row, its notices and its key,
/// recorded by the slot.
pub(crate) fn remove_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    qid: &str,
) -> Result<bool> {
    tx.execute(
        "DELETE FROM queue_notices WHERE queue_id = ?1",
        params![qid],
    )?;
    clear_in(st, tx, ctx, &SecretSlot::NotifierZulip(qid.into()))?;
    Ok(tx.execute(
        "DELETE FROM queue_notifiers WHERE queue_id = ?1",
        params![qid],
    )? > 0)
}

/// What deleting queue `qid` ends on `tx`: [`remove_in`], and what its
/// reports' fixes linked to.
pub(crate) fn remove_queue_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    qid: &str,
) -> Result<()> {
    remove_in(st, tx, ctx, qid)?;
    tx.execute(
        "DELETE FROM queue_fix_links WHERE thread_id IN
            (SELECT id FROM threads WHERE queue_id = ?1)",
        params![qid],
    )?;
    Ok(())
}

/// Whether the fix numbered `n` of report `tid`, whose wait has not ended,
/// needs a person, as the module documentation has it, on `c`: `preview` is its
/// own. A step kept for it says its preview waits for someone, and a queue with
/// a target of an operator's has no link to say so.
fn needs_person_in(
    c: &Connection,
    qid: &str,
    (tid, n): (&str, u32),
    run: Option<&str>,
    preview: bool,
) -> Result<bool> {
    if preview || run_confidential_in(c, run)? {
        return Ok(true);
    }
    let step: bool = c
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM queue_fix_links l
                JOIN report_fixes f ON f.thread_id = l.thread_id
                    AND f.proposal_key = l.proposal_key
                WHERE f.thread_id = ?1 AND f.n = ?2 AND l.preview_step IS NOT NULL)",
        )?
        .query_row(params![tid, n], |r| r.get(0))?;
    if step {
        return Ok(true);
    }
    Ok(
        c.prepare_cached("SELECT reviewed_previews FROM queue_repos WHERE queue_id = ?1")?
            .query_row(params![qid], |r| r.get::<_, bool>(0))
            .optional()?
            .unwrap_or(false),
    )
}

/// Whether `run` may hold what must not go out: it says so, or it is a run
/// with no row left to ask. No run at all holds nothing.
fn run_confidential_in(c: &Connection, run: Option<&str>) -> Result<bool> {
    let Some(run) = run else {
        return Ok(false);
    };
    Ok(
        c.prepare_cached("SELECT confidential FROM bot_runs WHERE id = ?1")?
            .query_row(params![run], |r| r.get::<_, bool>(0))
            .optional()?
            .unwrap_or(true),
    )
}

/// The queue of report `tid` and the kinds of notifier it has, when it has any.
fn notifiers_of_in(tx: &Transaction<'_>, tid: &str) -> Result<Option<(String, Vec<String>)>> {
    let Some(qid) = tx
        .prepare_cached("SELECT queue_id FROM threads WHERE id = ?1")?
        .query_row(params![tid], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
    else {
        return Ok(None);
    };
    let kinds: Vec<String> = tx
        .prepare_cached("SELECT kind FROM queue_notifiers WHERE queue_id = ?1")?
        .query_map(params![qid], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok((!kinds.is_empty()).then_some((qid, kinds)))
}

/// The kinds of notifier queue `qid` had at `proposed`: one first set later is
/// told nothing that asks a person of a fix proposed then.
fn notifiers_at_in(tx: &Transaction<'_>, qid: &str, proposed: &str) -> Result<Vec<String>> {
    Ok(tx
        .prepare_cached(
            "SELECT kind FROM queue_notifiers WHERE queue_id = ?1 AND created_at <= ?2",
        )?
        .query_map(params![qid, proposed], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Writes on `tx` one notice of `event` on the fix numbered `n` of report
/// `tid`, for each of `kinds`.
fn write_notices_in(
    tx: &Transaction<'_>,
    now: &str,
    (qid, kinds): (&str, &[String]),
    (tid, n): (&str, u32),
    event: NoticeEvent,
    link: &str,
) -> Result<()> {
    for kind in kinds {
        tx.execute(
            "INSERT OR IGNORE INTO queue_notices (id, queue_id, notifier, thread_id, fix_n, event,
                link, state, attempts, next_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0, ?8, ?8)",
            params![new_ulid(), qid, kind, tid, n, event.as_str(), link, now],
        )?;
    }
    Ok(())
}

/// Writes on `tx` that a reopen rejected the fix numbered `n` of report `tid`.
pub(crate) fn note_fix_rejected_in(
    tx: &Transaction<'_>,
    now: &str,
    tid: &str,
    n: u32,
) -> Result<()> {
    let Some((qid, kinds)) = notifiers_of_in(tx, tid)? else {
        return Ok(());
    };
    let event = NoticeEvent::FixRejected;
    write_notices_in(tx, now, (&qid, &kinds), (tid, n), event, "")
}

/// Writes on `tx` the notice the fix numbered `n` of report `tid` now calls
/// for, if any, once for each of its queue's notifiers. Asked at every change
/// of a fix that could call for one; what was written stays the one notice.
pub(crate) fn note_fix_in(tx: &Transaction<'_>, now: &str, tid: &str, n: u32) -> Result<()> {
    let Some((qid, kinds)) = notifiers_of_in(tx, tid)? else {
        return Ok(());
    };
    type Row = (String, bool, bool, bool, Option<String>, String);
    let (json, held, preview, missed, run, proposed): Row = tx
        .prepare_cached(
            "SELECT fix_json, held, preview_url IS NOT NULL, preview_missed, proposer_run_id,
                created_at
             FROM report_fixes WHERE thread_id = ?1 AND n = ?2",
        )?
        .query_row(params![tid, n], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
    let fix: NewFix = serde_json::from_str(&json).map_err(|_| CoreError::Corrupt {
        artifact_id: tid.to_string(),
        column: "report_fixes.fix_json",
        version: None,
    })?;
    let (event, link) = match fix.verify_url {
        Some(url) if !held => (NoticeEvent::FixToCheck, url),
        _ if held && missed => (NoticeEvent::FixWaitEnded, String::new()),
        _ if held && needs_person_in(tx, &qid, (tid, n), run.as_deref(), preview)? => {
            (NoticeEvent::FixHeld, String::new())
        }
        _ => return Ok(()),
    };
    let kinds = if held {
        notifiers_at_in(tx, &qid, &proposed)?
    } else {
        kinds
    };
    write_notices_in(tx, now, (&qid, &kinds), (tid, n), event, &link)
}

/// [`note_fix_in`] of the fix run `run` has waiting on report `tid`.
pub(crate) fn note_run_fix_in(tx: &Transaction<'_>, now: &str, tid: &str, run: &str) -> Result<()> {
    let n: Option<u32> = tx
        .prepare_cached(
            "SELECT n FROM report_fixes WHERE thread_id = ?1 AND proposer_run_id = ?2
               AND outcome = 'pending' ORDER BY n DESC LIMIT 1",
        )?
        .query_row(params![tid, run], |r| r.get(0))
        .optional()?;
    n.map_or(Ok(()), |n| note_fix_in(tx, now, tid, n))
}

impl Store {
    /// Queue `qid`'s Zulip notifier, or `None`. No secret.
    ///
    /// # Errors
    /// The store's.
    pub fn zulip_notifier(&self, qid: &str) -> Result<Option<ZulipNotifier>> {
        self.with_read(|c| {
            notifier_in(c, qid)?
                .map(|row| view_in(self, c, qid, row))
                .transpose()
        })
    }

    /// Sets queue `qid`'s Zulip notifier as `ctx`, whom the caller has checked
    /// is one of its admins and, with `by_owner`, the instance's owner.
    /// `f.if_rev` is the `rev` it was made from, `None` for no notifier.
    /// Recorded as `queue.update {queue_id, op: "notifier", kind, site,
    /// bot_email, stream, people}`, `people` a count, and the key as
    /// `admin.key`. A site as it was answers to the rule of who set it, so
    /// an admin may save the rest of a form holding the owner's loopback
    /// address, while the instance's rule still allows it. What was found of
    /// the last settings is forgotten, a test message may go at once, and
    /// every notice still being tried is tried at once.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `notifier_changed` when
    /// `f.if_rev` is not its `rev`; `invalid_site`, `invalid_bot_email`,
    /// `invalid_stream`, `invalid_people` or `invalid_key` for bad input;
    /// `api_key_required` with no key for a new notifier or a new site's
    /// origin; the store's.
    pub fn set_zulip_notifier(
        &self,
        ctx: &AuditCtx,
        kek: &Kek,
        qid: &str,
        f: &ZulipFields,
        by_owner: bool,
    ) -> Result<ZulipNotifier> {
        let key = f.api_key.as_deref().map(check_key).transpose()?;
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let old = notifier_in(tx, qid)?;
            if old.as_ref().map(|o| o.rev.as_str()) != f.if_rev.as_deref() {
                return Err(changed());
            }
            let trimmed = f.site.trim().trim_end_matches('/');
            let kept = old.as_ref().filter(|o| o.settings.site == trimmed);
            let (site, owner_site) = match kept {
                Some(o) => {
                    let policy = policy_for(self, o.owner_site);
                    check_url("invalid_site", &o.settings.site, policy)?;
                    (o.settings.site.clone(), o.owner_site)
                }
                None => {
                    let policy = policy_for(self, by_owner);
                    (check_url("invalid_site", &f.site, policy)?, by_owner)
                }
            };
            let settings = check_fields(f, site)?;
            let host = clax_core::config::key_host(&settings.site);
            let same_host = old
                .as_ref()
                .is_some_and(|o| clax_core::config::key_host(&o.settings.site) == host);
            let slot = SecretSlot::NotifierZulip(qid.into());
            match key {
                Some(value) => {
                    let last4 = last4_of(value);
                    put_in(
                        self,
                        tx,
                        ctx,
                        kek,
                        &slot,
                        &Sealing {
                            value,
                            shown: &last4,
                            host: host.as_deref(),
                            forge_host: None,
                        },
                    )?;
                }
                None if same_host && meta_in(tx, &slot)?.is_some() => {}
                None => {
                    return Err(CoreError::invalid(
                        "api_key_required",
                        "a new site needs the bot's API key",
                    ));
                }
            }
            let now = self.now();
            let json = serde_json::to_string(&settings).expect("settings serialise");
            tx.execute(
                "INSERT INTO queue_notifiers (queue_id, kind, rev, settings_json, owner_site,
                    set_by, set_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT (queue_id, kind) DO UPDATE SET rev = excluded.rev,
                    settings_json = excluded.settings_json, owner_site = excluded.owner_site,
                    set_by = excluded.set_by, set_at = excluded.set_at, last_ok_at = NULL,
                    last_failure = NULL, last_failure_at = NULL, direct_failure = NULL,
                    direct_failure_at = NULL, last_test_at = NULL",
                params![qid, ZULIP, new_ulid(), json, owner_site, set_by(ctx), now],
            )?;
            tx.execute(
                "UPDATE queue_notices SET next_at = ?3 WHERE queue_id = ?1 AND notifier = ?2
                   AND state = 'pending'",
                params![qid, ZULIP, now],
            )?;
            let rec = notifier_record(&now, qid)
                .with("site", settings.site.as_str())
                .with("bot_email", settings.bot_email.as_str())
                .with("stream", settings.stream.as_str())
                .with("people", settings.people.len());
            self.record_audit(tx, ctx, rec)?;
            let row = notifier_in(tx, qid)?.expect("the notifier was just written");
            view_in(self, tx, qid, row)
        })
    }

    /// Removes queue `qid`'s Zulip notifier, its notices and its key, recorded
    /// as `queue.update {queue_id, op: "notifier", kind, removed: true}`.
    /// Answers whether it had one.
    ///
    /// # Errors
    /// `notifier_changed` when it has one and `if_rev` is not its `rev`; the
    /// store's.
    pub fn clear_zulip_notifier(&self, ctx: &AuditCtx, qid: &str, if_rev: &str) -> Result<bool> {
        self.with_tx(|tx| {
            let Some(row) = notifier_in(tx, qid)? else {
                return Ok(false);
            };
            if row.rev != if_rev {
                return Err(changed());
            }
            remove_in(self, tx, ctx, qid)?;
            let rec = notifier_record(&self.now(), qid).with("removed", true);
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
        })
    }

    /// Answers whether a person may send queue `qid`'s test message now: none
    /// went in the last [`TEST_EVERY_SECS`] under these settings, since a save
    /// is what a person tests next. One that may is recorded, as it uses the
    /// key: `queue.update {queue_id, op: "notifier", kind, test: true}`.
    ///
    /// # Errors
    /// The store's.
    pub fn record_notifier_test(&self, ctx: &AuditCtx, qid: &str) -> Result<bool> {
        let at = self.now_utc();
        let now = clax_core::time::stamp(at);
        let since = clax_core::time::stamp(at - chrono::Duration::seconds(TEST_EVERY_SECS));
        self.with_tx(|tx| {
            let admitted = tx.execute(
                "UPDATE queue_notifiers SET last_test_at = ?3 WHERE queue_id = ?1 AND kind = ?2
                   AND (last_test_at IS NULL OR last_test_at <= ?4)",
                params![qid, ZULIP, now, since],
            )? > 0;
            if admitted {
                let rec = notifier_record(&now, qid).with("test", true);
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(admitted)
        })
    }

    /// Queue `qid`'s Zulip notifier with its key, for a delivery alone.
    /// `None` with no notifier or no key.
    ///
    /// # Errors
    /// `invalid_site` for a site the instance's rule no longer allows;
    /// `kek_mismatch` for a key that does not open; `key_not_for_site` for
    /// one set for another origin; the store's.
    pub fn open_zulip_notifier(&self, kek: &Kek, qid: &str) -> Result<Option<OpenedZulip>> {
        self.with_read(|c| {
            let Some(row) = notifier_in(c, qid)? else {
                return Ok(None);
            };
            let policy = policy_for(self, row.owner_site);
            check_url("invalid_site", &row.settings.site, policy)?;
            let slot = SecretSlot::NotifierZulip(qid.into());
            let Some(meta) = meta_in(c, &slot)? else {
                return Ok(None);
            };
            if meta.host_problem(&row.settings.site).is_some() {
                return Err(CoreError::invalid(
                    "key_not_for_site",
                    "the bot's API key was set for another site",
                ));
            }
            let Some(api_key) = open_in(c, kek, &slot)? else {
                return Ok(None);
            };
            Ok(Some(OpenedZulip {
                local_site: local_site(self, &row),
                rev: row.rev,
                site: row.settings.site,
                bot_email: row.settings.bot_email,
                api_key,
                stream: row.settings.stream,
                people: row.settings.people,
            }))
        })
    }

    /// Notes that a message went by `channel` under `rev`, which an older
    /// `rev` of queue `qid`'s notifier leaves unsaid. It ends that channel's
    /// failure alone.
    ///
    /// # Errors
    /// The store's.
    pub fn note_notifier_ok(&self, qid: &str, rev: &str, channel: NoticeChannel) -> Result<()> {
        let now = self.now();
        let sql = match channel {
            NoticeChannel::Stream => {
                "UPDATE queue_notifiers SET last_ok_at = ?3, last_failure = NULL,
                    last_failure_at = NULL
                 WHERE queue_id = ?1 AND kind = 'zulip' AND rev = ?2"
            }
            NoticeChannel::Direct => {
                "UPDATE queue_notifiers SET last_ok_at = ?3, direct_failure = NULL,
                    direct_failure_at = NULL
                 WHERE queue_id = ?1 AND kind = 'zulip' AND rev = ?2"
            }
        };
        self.with_tx(|tx| {
            tx.execute(sql, params![qid, rev, now])?;
            Ok(())
        })
    }

    /// Notes `what`, fixed words, as what failed by `channel` under `rev`.
    ///
    /// # Errors
    /// The store's.
    pub fn note_notifier_failure(
        &self,
        qid: &str,
        rev: &str,
        channel: NoticeChannel,
        what: &str,
    ) -> Result<()> {
        let now = self.now();
        let sql = match channel {
            NoticeChannel::Stream => {
                "UPDATE queue_notifiers SET last_failure = ?3, last_failure_at = ?4
                 WHERE queue_id = ?1 AND kind = 'zulip' AND rev = ?2"
            }
            NoticeChannel::Direct => {
                "UPDATE queue_notifiers SET direct_failure = ?3, direct_failure_at = ?4
                 WHERE queue_id = ?1 AND kind = 'zulip' AND rev = ?2"
            }
        };
        self.with_tx(|tx| {
            tx.execute(sql, params![qid, rev, what, now])?;
            Ok(())
        })
    }

    /// Takes up to `limit` notices due now for [`LEASE_SECS`]: no other taker
    /// gets them until then. Each queue's oldest comes before any queue's
    /// second. Notices past [`KEEP_SECS`] stop being tried instead, and those
    /// sent or dropped [`PRUNE_SECS`] ago are forgotten.
    ///
    /// # Errors
    /// The store's.
    pub fn take_due_notices(&self, limit: u32) -> Result<Vec<DueNotice>> {
        let at = self.now_utc();
        let now = clax_core::time::stamp(at);
        let until = clax_core::time::stamp(at + chrono::Duration::seconds(LEASE_SECS));
        let cutoff = clax_core::time::stamp(at - chrono::Duration::seconds(KEEP_SECS));
        let forgotten = clax_core::time::stamp(at - chrono::Duration::seconds(PRUNE_SECS));
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE queue_notices SET state = 'expired'
                 WHERE state = 'pending' AND created_at < ?1",
                params![cutoff],
            )?;
            tx.execute(
                "DELETE FROM queue_notices WHERE state IN ('sent', 'dropped') AND created_at < ?1",
                params![forgotten],
            )?;
            let due: Vec<DueNotice> = tx
                .prepare_cached(
                    "SELECT id, queue_id, thread_id, fix_n, event, attempts, link FROM
                       (SELECT *, ROW_NUMBER() OVER
                            (PARTITION BY queue_id ORDER BY next_at, id) AS turn
                        FROM queue_notices WHERE state = 'pending' AND next_at <= ?1)
                     ORDER BY turn, next_at, id LIMIT ?2",
                )?
                .query_map(params![now, limit], |r| {
                    Ok(DueNotice {
                        id: r.get(0)?,
                        queue_id: r.get(1)?,
                        thread_id: r.get(2)?,
                        n: r.get(3)?,
                        event: NoticeEvent::parse(&r.get::<_, String>(4)?),
                        attempts: r.get::<_, u32>(5)? + 1,
                        link: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            for d in &due {
                tx.execute(
                    "UPDATE queue_notices SET attempts = ?2, next_at = ?3 WHERE id = ?1",
                    params![d.id, d.attempts, until],
                )?;
            }
            Ok(due)
        })
    }

    /// Marks a taken notice sent. It is never tried again.
    ///
    /// # Errors
    /// The store's.
    pub fn notice_sent(&self, id: &str) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE queue_notices SET state = 'sent', sent_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            Ok(())
        })
    }

    /// Ends a taken notice with nothing sent: what it told is no longer so.
    ///
    /// # Errors
    /// The store's.
    pub fn drop_notice(&self, id: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE queue_notices SET state = 'dropped' WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
    }

    /// Puts off a taken notice that failed: double the wait of the try
    /// before, from a minute to an hour, or `wait_secs`, a server's own word,
    /// when longer and within that hour.
    ///
    /// # Errors
    /// The store's.
    pub fn retry_notice(&self, id: &str, wait_secs: Option<i64>) -> Result<()> {
        let at = self.now_utc();
        self.with_tx(|tx| {
            let attempts: Option<u32> = tx
                .query_row(
                    "SELECT attempts FROM queue_notices WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(attempts) = attempts else {
                return Ok(());
            };
            let backoff = FIRST_BACKOFF_SECS
                .saturating_mul(1i64 << attempts.saturating_sub(1).min(16))
                .min(MAX_BACKOFF_SECS);
            let wait = wait_secs
                .map_or(backoff, |w| w.max(backoff))
                .min(MAX_BACKOFF_SECS);
            let next = clax_core::time::stamp(at + chrono::Duration::seconds(wait));
            tx.execute(
                "UPDATE queue_notices SET next_at = ?2 WHERE id = ?1",
                params![id, next],
            )?;
            Ok(())
        })
    }

    /// When the next notice is due, for a host that sleeps until then.
    ///
    /// # Errors
    /// The store's.
    pub fn notices_next_due(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.with_read(|c| {
            let at: Option<String> = c
                .prepare_cached("SELECT MIN(next_at) FROM queue_notices WHERE state = 'pending'")?
                .query_row([], |r| r.get(0))?;
            Ok(at
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                .map(|t| t.with_timezone(&chrono::Utc)))
        })
    }

    /// Whether the fix numbered `n` of report `tid` came of a run that may
    /// hold confidential material. A person's fix that took over from such a
    /// run's stays with it.
    ///
    /// # Errors
    /// The store's.
    pub fn fix_is_confidential(&self, tid: &str, n: u32) -> Result<bool> {
        self.with_read(|c| {
            let run: Option<String> = c
                .prepare_cached(
                    "SELECT proposer_run_id FROM report_fixes WHERE thread_id = ?1 AND n = ?2",
                )?
                .query_row(params![tid, n], |r| r.get(0))
                .optional()?
                .flatten();
            run_confidential_in(c, run.as_deref())
        })
    }

    /// Keeps what the proposal `key` on report `tid` links to, for a held
    /// fix's notice to the queue's people: none of it is in its reporter's
    /// view in `fix_json`. Made before the proposal, so that its notice finds
    /// it; the first word on a key stands.
    ///
    /// # Errors
    /// The store's.
    pub fn keep_fix_links(&self, tid: &str, key: &str, links: &FixLinks) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO queue_fix_links (thread_id, proposal_key, url, commit_sha,
                    preview_step)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![tid, key, links.url, links.commit, links.preview_step],
            )?;
            Ok(())
        })
    }

    /// The commit of the latest fix run `run` proposed on report `tid`
    /// ([`Store::keep_fix_links`]).
    ///
    /// # Errors
    /// The store's.
    pub fn run_fix_commit(&self, tid: &str, run: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT l.commit_sha FROM queue_fix_links l
                 JOIN report_fixes f ON f.thread_id = l.thread_id
                    AND f.proposal_key = l.proposal_key
                 WHERE f.thread_id = ?1 AND f.proposer_run_id = ?2
                 ORDER BY f.n DESC LIMIT 1",
            )?
            .query_row(params![tid, run], |r| r.get(0))
            .optional()?
            .flatten())
        })
    }

    /// What the proposal behind the fix numbered `n` of report `tid` links to
    /// ([`Store::keep_fix_links`]).
    ///
    /// # Errors
    /// The store's.
    pub fn fix_links(&self, tid: &str, n: u32) -> Result<Option<FixLinks>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT l.url, l.commit_sha, l.preview_step FROM queue_fix_links l
                 JOIN report_fixes f ON f.thread_id = l.thread_id
                    AND f.proposal_key = l.proposal_key
                 WHERE f.thread_id = ?1 AND f.n = ?2",
            )?
            .query_row(params![tid, n], |r| {
                Ok(FixLinks {
                    url: r.get(0)?,
                    commit: r.get(1)?,
                    preview_step: r.get(2)?,
                })
            })
            .optional()?)
        })
    }
}

#[cfg(test)]
mod tests;
