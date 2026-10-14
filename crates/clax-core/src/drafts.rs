//! Agents' drafts (spec 2026-10-11-clax-dev-hosted-design §16.4): the work
//! in progress an agent shows the people viewing an artifact before it
//! publishes, at most one per agent per artifact.
//!
//! A draft is the files an agent is changing over the artifact's current
//! version (its base): each `draft` call replaces the one before. People
//! see that the agent is drafting and can show the draft in the frame, from
//!
//!   /c/<artifact ID>/draft/<agent handle>/<token>/<path>
//!
//! where the token is minted afresh for every revision and travels only in
//! the `drafts` event and the people's listing, so the URL is a capability
//! held by the people who were shown that revision; it stops answering
//! when the draft is revised or ends. A drafted HTML page is stored as it is
//! served: its changed elements marked against the base file at its path
//! ([`crate::draft_diff`]; only a base file the drafting agent may see) and
//! the tint script ([`tint_script`]) added, so it carries nothing of the
//! base. A path the draft does not write is served from the base version,
//! unless the base file is withheld from the drafting agent.
//!
//! Drafts live in memory only, by design: a draft is minutes of work in
//! progress that its agent sends again as it goes, and that ends when it
//! publishes, discards it, its session ends, or it goes [`DRAFT_IDLE_SECS`]
//! without revising it. A daemon restart drops them as it drops working
//! records. The registry takes one lock per call and does no I/O, so it
//! runs as it is in a single-threaded host.

use crate::secret::{constant_time_eq, random_bytes};
use crate::working::Clock;
use base64::Engine as _;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

/// Most bytes of files one draft may carry: 1 MiB.
pub const MAX_DRAFT_BYTES: usize = 1024 * 1024;
/// Most `draft` calls an agent may make on one artifact in [`DRAFT_RATE_WINDOW_MS`].
pub const DRAFT_RATE: usize = 2;
/// The window [`DRAFT_RATE`] counts calls in.
pub const DRAFT_RATE_WINDOW_MS: i64 = 1000;
/// A draft not revised for this long ends.
pub const DRAFT_IDLE_SECS: i64 = 120;
/// Most drafts live at once on the daemon, every artifact's together: with
/// [`MAX_DRAFT_BYTES`] each, what bounds the memory drafts hold.
pub const MAX_LIVE_DRAFTS: usize = 64;
/// Most drafts one agent session keeps live at once, on different
/// artifacts: no one session takes every one of [`MAX_LIVE_DRAFTS`].
pub const MAX_SESSION_DRAFTS: usize = 8;
/// Longest note, in characters, after [`crate::working::clean_line`].
pub const MAX_NOTE_CHARS: usize = crate::working::MAX_MESSAGE_CHARS;
/// The message the tint script posts to its parent with how many elements
/// it tinted.
pub const DIFF_MESSAGE: &str = "clax:draft-diff";

/// The agent session a draft is of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drafter {
    pub session_id: String,
    /// The session's opaque agent handle (`a_…`), safe to show anyone: the
    /// draft URL names it.
    pub agent: String,
    pub harness: String,
}

/// A live draft as people see it. It never names the session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DraftView {
    /// The agent's handle.
    pub agent: String,
    pub harness: String,
    /// The version the draft changes.
    pub base_version: u32,
    /// The paths the draft writes or removes, sorted.
    pub paths: Vec<String>,
    pub note: Option<String>,
    pub updated_at: String,
    /// This revision's token, for the draft URL.
    pub token: String,
}

/// One path of a draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftFile {
    /// The draft removes the path.
    Removed,
    /// The bytes served at the path (an HTML page already marked and tinted).
    Bytes {
        bytes: Arc<[u8]>,
        content_type: String,
    },
}

/// A draft revision to store, prepared by the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewDraft {
    pub base_version: u32,
    pub note: Option<String>,
    pub files: BTreeMap<String, DraftFile>,
    /// The base version's paths withheld from the drafting agent: never
    /// served through the draft.
    pub withheld: BTreeSet<String>,
}

/// What the draft URL answers for a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Served {
    /// The draft's bytes.
    Draft {
        bytes: Arc<[u8]>,
        content_type: String,
    },
    /// The path of base version `n`, which the draft does not write.
    Base { n: u32 },
}

/// Why a draft was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Past [`DRAFT_RATE`] calls in [`DRAFT_RATE_WINDOW_MS`]: try again after
    /// `retry_after_ms`.
    RateLimited { retry_after_ms: i64 },
    /// [`MAX_LIVE_DRAFTS`] drafts are live already.
    TooManyDrafts,
    /// The session has [`MAX_SESSION_DRAFTS`] drafts live already.
    TooManyOfSession,
}

struct Draft {
    agent: String,
    harness: String,
    base_version: u32,
    note: Option<String>,
    token: String,
    updated_at: DateTime<Utc>,
    files: BTreeMap<String, DraftFile>,
    withheld: BTreeSet<String>,
}

impl Draft {
    fn view(&self) -> DraftView {
        DraftView {
            agent: self.agent.clone(),
            harness: self.harness.clone(),
            base_version: self.base_version,
            paths: self.files.keys().cloned().collect(),
            note: self.note.clone(),
            updated_at: self.updated_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            token: self.token.clone(),
        }
    }
}

/// (artifact ID, session ID).
type Key = (String, String);

#[derive(Default)]
struct Inner {
    drafts: BTreeMap<Key, Draft>,
    /// Each agent's recent `draft` calls per artifact, for the rate limit.
    calls: BTreeMap<Key, VecDeque<DateTime<Utc>>>,
}

/// A new draft token: 128 random bits in unpadded base64url (22 characters).
fn new_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<16>())
}

/// Whether `s` has a draft token's form.
pub fn is_token(s: &str) -> bool {
    s.len() == 22
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The registry of live drafts.
pub struct Drafts {
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
}

impl Drafts {
    pub fn new(clock: Arc<dyn Clock>) -> Drafts {
        Drafts {
            clock,
            inner: Mutex::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Every change leaves the maps consistent: a poisoned lock still holds them.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn live(d: &Draft, now: DateTime<Utc>) -> bool {
        now - d.updated_at < Duration::seconds(DRAFT_IDLE_SECS)
    }

    /// Drops idle drafts and stale call times; the artifacts whose drafts
    /// ended, which the caller announces.
    fn prune(inner: &mut Inner, now: DateTime<Utc>) -> BTreeSet<String> {
        let mut ended = BTreeSet::new();
        inner.drafts.retain(|(aid, _), d| {
            let keep = Self::live(d, now);
            if !keep {
                ended.insert(aid.clone());
            }
            keep
        });
        Self::prune_calls(inner, now);
        ended
    }

    /// Drops call times past the rate window.
    fn prune_calls(inner: &mut Inner, now: DateTime<Utc>) {
        let window = Duration::milliseconds(DRAFT_RATE_WINDOW_MS);
        inner.calls.retain(|_, ts| {
            while ts.front().is_some_and(|t| now - *t >= window) {
                ts.pop_front();
            }
            !ts.is_empty()
        });
    }

    /// Counts a `draft` call of `who` on `aid` against [`DRAFT_RATE`], or
    /// refuses it. A refused call is not counted.
    ///
    /// # Errors
    /// [`Refusal::RateLimited`] past the rate.
    pub fn admit(&self, who: &Drafter, aid: &str) -> Result<(), Refusal> {
        let now = self.clock.now();
        let mut g = self.lock();
        // Idle drafts stay for the next put or sweep, which announce their end.
        Self::prune_calls(&mut g, now);
        let calls = g
            .calls
            .entry((aid.to_string(), who.session_id.clone()))
            .or_default();
        if calls.len() >= DRAFT_RATE {
            let first = calls.front().copied().unwrap_or(now);
            let retry = (first + Duration::milliseconds(DRAFT_RATE_WINDOW_MS) - now)
                .num_milliseconds()
                .max(1);
            return Err(Refusal::RateLimited {
                retry_after_ms: retry,
            });
        }
        calls.push_back(now);
        Ok(())
    }

    /// Stores `d` as `who`'s draft on `aid`, replacing any before it, under a
    /// new token. Answers its view and the artifacts whose drafts changed
    /// (`aid`, and any whose drafts went idle meanwhile).
    ///
    /// # Errors
    /// [`Refusal::TooManyDrafts`] when it would be a new draft past
    /// [`MAX_LIVE_DRAFTS`], [`Refusal::TooManyOfSession`] past
    /// [`MAX_SESSION_DRAFTS`] of the session's.
    pub fn put(
        &self,
        who: &Drafter,
        aid: &str,
        d: NewDraft,
    ) -> Result<(DraftView, BTreeSet<String>), Refusal> {
        let now = self.clock.now();
        let mut g = self.lock();
        let mut changed = Self::prune(&mut g, now);
        let key = (aid.to_string(), who.session_id.clone());
        if !g.drafts.contains_key(&key) {
            let mine = g.drafts.keys().filter(|(_, s)| *s == key.1).count();
            if mine >= MAX_SESSION_DRAFTS {
                return Err(Refusal::TooManyOfSession);
            }
            if g.drafts.len() >= MAX_LIVE_DRAFTS {
                return Err(Refusal::TooManyDrafts);
            }
        }
        let draft = Draft {
            agent: who.agent.clone(),
            harness: who.harness.clone(),
            base_version: d.base_version,
            note: d.note,
            token: new_token(),
            updated_at: now,
            files: d.files,
            withheld: d.withheld,
        };
        let view = draft.view();
        g.drafts.insert(key, draft);
        changed.insert(aid.to_string());
        Ok((view, changed))
    }

    /// Ends session `sid`'s draft on `aid`; whether it had one.
    pub fn end(&self, sid: &str, aid: &str) -> bool {
        let mut g = self.lock();
        g.drafts
            .remove(&(aid.to_string(), sid.to_string()))
            .is_some()
    }

    /// Ends every draft of session `sid`; the artifacts they were on.
    pub fn end_session(&self, sid: &str) -> BTreeSet<String> {
        let mut g = self.lock();
        let mut ended = BTreeSet::new();
        g.drafts.retain(|(aid, s), _| {
            let keep = s != sid;
            if !keep {
                ended.insert(aid.clone());
            }
            keep
        });
        ended
    }

    /// Ends every draft on `aid` (it was deleted); whether there were any.
    pub fn end_artifact(&self, aid: &str) -> bool {
        let mut g = self.lock();
        let before = g.drafts.len();
        g.drafts.retain(|(a, _), _| a != aid);
        g.drafts.len() != before
    }

    /// Removes idle drafts; the artifacts they were on.
    pub fn sweep(&self) -> BTreeSet<String> {
        let now = self.clock.now();
        let mut g = self.lock();
        Self::prune(&mut g, now)
    }

    /// The live drafts on `aid`, oldest revision first.
    pub fn for_artifact(&self, aid: &str) -> Vec<DraftView> {
        let now = self.clock.now();
        let g = self.lock();
        let mut list: Vec<&Draft> = g
            .drafts
            .iter()
            .filter(|((a, _), d)| a == aid && Self::live(d, now))
            .map(|(_, d)| d)
            .collect();
        list.sort_by(|a, b| {
            a.updated_at
                .cmp(&b.updated_at)
                .then_with(|| a.agent.cmp(&b.agent))
        });
        list.into_iter().map(Draft::view).collect()
    }

    /// Session `sid`'s live draft on `aid`.
    pub fn of_session(&self, sid: &str, aid: &str) -> Option<DraftView> {
        let now = self.clock.now();
        let g = self.lock();
        g.drafts
            .get(&(aid.to_string(), sid.to_string()))
            .filter(|d| Self::live(d, now))
            .map(Draft::view)
    }

    /// What the draft URL of agent `agent`'s revision `token` on `aid`
    /// answers at `path`: the draft's bytes for a path it writes, the base
    /// version for one it does not, and `None` for a path it removes, a base
    /// path withheld from its agent, or an unknown, revised or ended draft.
    /// The token is compared in constant time.
    pub fn file(&self, aid: &str, agent: &str, token: &str, path: &str) -> Option<Served> {
        let now = self.clock.now();
        let g = self.lock();
        let d = g
            .drafts
            .iter()
            .find(|((a, _), d)| a == aid && d.agent == agent)
            .map(|(_, d)| d)
            .filter(|d| Self::live(d, now))?;
        if !constant_time_eq(d.token.as_bytes(), token.as_bytes()) {
            return None;
        }
        match d.files.get(path) {
            Some(DraftFile::Bytes {
                bytes,
                content_type,
            }) => Some(Served::Draft {
                bytes: bytes.clone(),
                content_type: content_type.clone(),
            }),
            Some(DraftFile::Removed) => None,
            None if d.withheld.contains(path) => None,
            None => Some(Served::Base { n: d.base_version }),
        }
    }
}

/// The script added to a draft's HTML. The server has already marked each
/// changed element with [`crate::draft_diff::CHANGED_ATTR`], so the page
/// carries nothing of the base version. The script tints the marked
/// elements in the colour the frame URL's fragment names (`#tint=%23rrggbb`),
/// more strongly when the page is shown dark, scrolls the first into view,
/// and posts `{type: "clax:draft-diff", changed: n, marked: boolean}` to its
/// parent: with `marked` false (too many changes to mark,
/// [`crate::draft_diff::MAX_MARKS`]) `changed` is the server's count and
/// nothing is tinted.
pub fn tint_script(changed: usize, marked: bool) -> String {
    let attr = crate::draft_diff::CHANGED_ATTR;
    format!(
        r##"(()=>{{const N={changed},K={marked};const m=/(?:^#|&)tint=(?:%23|#)?([0-9a-fA-F]{{6}})(?:&|$)/.exec(location.hash);const C="#"+(m?m[1]:"2f6f2a");
const S="[{attr}]";
const st=document.createElement("style");st.textContent=S+"{{--clax-t:color-mix(in srgb,"+C+" 18%,transparent);box-shadow:inset 0 0 0 100vmax var(--clax-t),0 0 0 4px var(--clax-t)!important;outline:1.5px solid "+C+"!important;outline-offset:4px!important;border-radius:2px}}@media (prefers-color-scheme:dark){{"+S+"{{--clax-t:color-mix(in srgb,"+C+" 34%,transparent);outline-width:2px!important}}}}";
(document.head||document.documentElement).appendChild(st);
let first=true;const run=()=>{{const els=document.querySelectorAll(S);if(first&&els[0]){{first=false;els[0].scrollIntoView({{block:"center"}})}}parent.postMessage({{type:"{DIFF_MESSAGE}",changed:K?els.length:N,marked:K}},"*")}};
run();addEventListener("load",run)}})();"##
    )
}

/// `html` with the tint script before its last `</body>` (any letter
/// case), or at the end when it has none.
pub fn inject_tint(html: &str, changed: usize, marked: bool) -> String {
    let tag = format!("<script>{}</script>", tint_script(changed, marked));
    let lower = html.to_ascii_lowercase();
    match lower.rfind("</body>") {
        Some(at) => format!("{}{tag}{}", &html[..at], &html[at..]),
        None => format!("{html}{tag}"),
    }
}

/// The page served for drafted HTML `draft`: its changed elements against
/// `base` marked and the tint script added.
pub fn served_page(draft: &str, base: Option<&str>) -> String {
    let m = crate::draft_diff::mark_changed(draft, base);
    inject_tint(&m.html, m.changed, m.marked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working::ManualClock;

    fn who(sid: &str) -> Drafter {
        Drafter {
            session_id: sid.into(),
            agent: format!("a_{sid}"),
            harness: "claude".into(),
        }
    }

    fn page(html: &str) -> DraftFile {
        DraftFile::Bytes {
            bytes: html.as_bytes().into(),
            content_type: "text/html; charset=utf-8".into(),
        }
    }

    fn draft(files: &[(&str, DraftFile)]) -> NewDraft {
        NewDraft {
            base_version: 3,
            note: Some("restyling".into()),
            files: files
                .iter()
                .map(|(p, f)| (p.to_string(), f.clone()))
                .collect(),
            withheld: ["secret.css".to_string()].into(),
        }
    }

    fn registry() -> (Arc<ManualClock>, Drafts) {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        (clock.clone(), Drafts::new(clock))
    }

    #[test]
    fn a_revision_gets_a_new_token_and_serves_its_files_over_the_base() {
        let (_c, d) = registry();
        let (v1, changed) = d
            .put(
                &who("s1"),
                "art",
                draft(&[("index.html", page("<p>a</p>"))]),
            )
            .unwrap();
        assert_eq!(changed, ["art".to_string()].into());
        assert!(is_token(&v1.token), "{}", v1.token);
        assert_eq!(v1.paths, ["index.html"]);
        assert_eq!(v1.base_version, 3);
        assert!(matches!(
            d.file("art", "a_s1", &v1.token, "index.html"),
            Some(Served::Draft { .. })
        ));
        assert_eq!(
            d.file("art", "a_s1", &v1.token, "style.css"),
            Some(Served::Base { n: 3 })
        );
        assert_eq!(d.file("art", "a_s1", &v1.token, "secret.css"), None);
        let (v2, _) = d
            .put(
                &who("s1"),
                "art",
                draft(&[
                    ("index.html", page("<p>b</p>")),
                    ("old.html", DraftFile::Removed),
                ]),
            )
            .unwrap();
        assert_ne!(v1.token, v2.token);
        assert_eq!(d.file("art", "a_s1", &v1.token, "index.html"), None);
        assert_eq!(d.file("art", "a_s1", &v2.token, "old.html"), None);
        assert_eq!(d.file("art", "a_s2", &v2.token, "index.html"), None);
        assert_eq!(d.file("other", "a_s1", &v2.token, "index.html"), None);
        assert_eq!(d.for_artifact("art"), vec![v2]);
    }

    #[test]
    fn the_rate_allows_two_calls_a_second_per_agent_and_artifact() {
        let (c, d) = registry();
        assert!(d.admit(&who("s1"), "art").is_ok());
        assert!(d.admit(&who("s1"), "art").is_ok());
        assert_eq!(
            d.admit(&who("s1"), "art"),
            Err(Refusal::RateLimited {
                retry_after_ms: 1000
            })
        );
        // Another agent, or another artifact, counts apart.
        assert!(d.admit(&who("s2"), "art").is_ok());
        assert!(d.admit(&who("s1"), "other").is_ok());
        c.advance(1);
        assert!(d.admit(&who("s1"), "art").is_ok());
    }

    #[test]
    fn a_draft_ends_idle_on_publish_session_end_and_artifact_delete() {
        let (c, d) = registry();
        let f = || draft(&[("index.html", page("<p>a</p>"))]);
        d.put(&who("s1"), "a1", f()).unwrap();
        d.put(&who("s1"), "a2", f()).unwrap();
        d.put(&who("s2"), "a1", f()).unwrap();
        assert!(d.end("s2", "a1"));
        assert!(!d.end("s2", "a1"));
        assert_eq!(
            d.end_session("s1"),
            ["a1".to_string(), "a2".to_string()].into()
        );
        d.put(&who("s3"), "a3", f()).unwrap();
        assert!(d.end_artifact("a3"));
        let (v, _) = d.put(&who("s4"), "a4", f()).unwrap();
        c.advance(DRAFT_IDLE_SECS - 1);
        assert!(d.sweep().is_empty());
        assert_eq!(
            d.of_session("s4", "a4").map(|x| x.token),
            Some(v.token.clone())
        );
        c.advance(1);
        // Gone from reads at once, and the sweep announces it.
        assert!(d.for_artifact("a4").is_empty());
        assert_eq!(d.file("a4", "a_s4", &v.token, "index.html"), None);
        assert_eq!(d.sweep(), ["a4".to_string()].into());
        assert!(d.sweep().is_empty());
    }

    #[test]
    fn past_the_live_draft_cap_only_a_revision_is_stored() {
        let (_c, d) = registry();
        let f = || draft(&[("index.html", page("<p>a</p>"))]);
        for i in 0..MAX_LIVE_DRAFTS {
            d.put(&who(&format!("s{i}")), "art", f()).unwrap();
        }
        assert_eq!(d.put(&who("late"), "art", f()), Err(Refusal::TooManyDrafts));
        assert!(d.put(&who("s0"), "art", f()).is_ok());
    }

    #[test]
    fn one_session_keeps_at_most_its_share_of_the_live_drafts() {
        let (_c, d) = registry();
        let f = || draft(&[("index.html", page("<p>a</p>"))]);
        for i in 0..MAX_SESSION_DRAFTS {
            d.put(&who("hog"), &format!("a{i}"), f()).unwrap();
        }
        assert_eq!(
            d.put(&who("hog"), "one-more", f()),
            Err(Refusal::TooManyOfSession)
        );
        // Its revisions still land, and other sessions still draft.
        assert!(d.put(&who("hog"), "a0", f()).is_ok());
        assert!(d.put(&who("other"), "one-more", f()).is_ok());
        assert!(d.end("hog", "a0"));
        assert!(d.put(&who("hog"), "one-more", f()).is_ok());
    }

    #[test]
    fn a_rate_check_leaves_idle_drafts_for_the_sweep_to_announce() {
        let (c, d) = registry();
        d.put(&who("s1"), "a1", draft(&[("index.html", page("<p>a</p>"))]))
            .unwrap();
        c.advance(DRAFT_IDLE_SECS);
        assert!(d.admit(&who("s2"), "a2").is_ok());
        assert_eq!(d.sweep(), ["a1".to_string()].into());
    }

    #[test]
    fn the_served_page_carries_marks_and_the_tint_script_never_the_base_text() {
        let base = "<html><body><h1>Base heading kept</h1><p>Old secret words</p></body></html>";
        let out = served_page(
            "<html><body><h1>Base heading kept</h1><p>New words</p></BODY></html>",
            Some(base),
        );
        assert!(out.contains("<p data-clax-draft-changed>New words</p>"));
        assert!(!out.contains("Old secret words"));
        let script_at = out.find("<script>").unwrap();
        assert!(script_at < out.find("</BODY>").unwrap());
        assert!(out.contains("const N=1,K=true"));
        assert!(out.contains(DIFF_MESSAGE));
        assert!(inject_tint("<p>x</p>", 3, false).ends_with("</script>"));
        assert!(inject_tint("<p>x</p>", 3, false).contains("const N=3,K=false"));
    }

    #[test]
    fn token_form() {
        assert!(is_token(&new_token()));
        assert!(!is_token("short"));
        assert!(!is_token("aaaaaaaaaaaaaaaaaaaaa/"));
    }
}
