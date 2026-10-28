//! Web Push to the owner's devices ([`clax_core::webpush`]): when an item
//! that waits on the owner lands in the owner's inbox (an agent's question,
//! an approval request, an agent's reply on a thread, a machine asking to
//! connect), each subscribed device gets a push whose payload is a one-line
//! title, the item's shell URL and a tag. Nothing of a page, a comment or a
//! question's text goes in it. A push service that says the subscription
//! is gone (404, 410) has it forgotten. A local daemon's alone: a hosted
//! one sends none.

use crate::state::AppState;
use clax_core::Home;
use clax_core::webpush::{self, Message, Subscription, Vapid};
use clax_store::Store;
use clax_store::inbox::{InboxChange, ItemRow, Kind};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

/// How long one push service request may take.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// The longest title, in characters, "…" included.
pub const TITLE_MAX: usize = 80;

/// The kinds of items that wait on the owner, and so are pushed.
pub fn waits_on_owner(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Question | Kind::Approval | Kind::Reply | Kind::Connection
    )
}

/// Serialises the reads and writes of the home's push files, and the
/// key's first making.
fn files() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// The home's VAPID key, made on first use.
pub fn vapid(home: &Home) -> Result<Vapid, webpush::PushError> {
    let _g = files().lock().unwrap_or_else(|e| e.into_inner());
    Vapid::load_or_create(home)
}

/// The home's subscriptions.
pub fn subscriptions(home: &Home) -> Result<Vec<Subscription>, webpush::PushError> {
    let _g = files().lock().unwrap_or_else(|e| e.into_inner());
    webpush::load_subscriptions(home)
}

/// Keeps `sub`; the number kept.
pub fn subscribe(home: &Home, sub: Subscription) -> Result<usize, webpush::PushError> {
    let _g = files().lock().unwrap_or_else(|e| e.into_inner());
    webpush::add_subscription(home, sub)
}

/// Forgets `endpoint`'s subscription; whether there was one.
pub fn unsubscribe(home: &Home, endpoint: &str) -> Result<bool, webpush::PushError> {
    let _g = files().lock().unwrap_or_else(|e| e.into_inner());
    webpush::remove_subscription(home, endpoint)
}

/// A client for one batch of sends. Not kept between batches: a client's
/// connections belong to the runtime that made them.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(SEND_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("an HTTP client")
}

/// `s` with control and bidirectional characters removed, runs of
/// whitespace made one space, cut to [`TITLE_MAX`] characters with "…".
pub fn one_line(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let words: Vec<&str> = cleaned.split(' ').filter(|w| !w.is_empty()).collect();
    let line = words.join(" ");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() > TITLE_MAX {
        let cut: String = chars[..TITLE_MAX - 1].iter().collect();
        format!("{}…", cut.trim_end())
    } else {
        line
    }
}

/// The push for item `i`, from its inbox view `v` (spec 2026-10-06 §7.6):
/// who it is from, what it wants, and where a tap goes. `None` for an item
/// that does not wait on the owner, is read, is a person's, or is a
/// machine that connected with an invite (nothing to decide).
pub fn message(i: &ItemRow, v: &Value) -> Option<Message> {
    if i.recipient.is_some() || i.read_at.is_some() || !waits_on_owner(i.kind) {
        return None;
    }
    let agent = v["agent"]["name"]
        .as_str()
        .or_else(|| v["agent"]["harness"].as_str())
        .map(one_line)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "An agent".to_string());
    let title = match i.kind {
        Kind::Question => format!("{agent} asked you a question"),
        Kind::Approval => format!("{agent} needs your approval"),
        Kind::Reply => format!("{agent} replied to you"),
        Kind::Connection => {
            if v["connection"]["via"] == "invite" {
                return None;
            }
            "A machine asks to connect".to_string()
        }
        _ => return None,
    };
    let url = v["url"].as_str().unwrap_or("/inbox");
    // A path of the shell's, never another origin.
    let url = if url.starts_with('/') && !url.starts_with("//") {
        url
    } else {
        "/inbox"
    };
    // The tag the shell's in-tab notifications use (web/shell/src/q/notify.ts),
    // so one replaces the other.
    let tag = i
        .question_id
        .as_deref()
        .or(i.artifact_id.as_deref())
        .unwrap_or(&i.id);
    Some(Message {
        title: one_line(&title),
        url: url.to_string(),
        tag: format!("clax-inbox-{tag}"),
    })
}

/// The pushes `changes` make: one per item made in the owner's inbox that
/// waits on the owner.
fn messages(db: &Store, changes: &[InboxChange]) -> clax_core::Result<Vec<Message>> {
    let seqs: Vec<i64> = changes.iter().filter(|c| c.made).map(|c| c.seq).collect();
    if seqs.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = db.inbox_items_by_seq(&seqs)?;
    rows.retain(|i| i.recipient.is_none() && i.read_at.is_none() && waits_on_owner(i.kind));
    // Oldest first, as they were made.
    rows.sort_by_key(|i| i.seq);
    let views = crate::views::inbox::views(db, &rows, false)?;
    Ok(rows
        .iter()
        .zip(&views)
        .filter_map(|(i, v)| message(i, v))
        .collect())
}

/// Sends `msg` to every subscription of the home; a subscription its push
/// service says is gone is forgotten. Failures are logged, never returned.
pub async fn send_all(home: &Home, msgs: &[Message]) {
    let subs = match subscriptions(home) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "push subscriptions not read");
            return;
        }
    };
    if subs.is_empty() || msgs.is_empty() {
        return;
    }
    let client = client();
    for sub in &subs {
        for m in msgs {
            send_with(&client, home, sub, m).await;
        }
    }
}

/// Sends `msg` to `sub` alone (a device that just subscribed).
pub async fn send_one(home: &Home, sub: &Subscription, msg: &Message) {
    send_with(&client(), home, sub, msg).await;
}

async fn send_with(client: &reqwest::Client, home: &Home, sub: &Subscription, msg: &Message) {
    let vapid = match vapid(home) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "VAPID key not read; no push sent");
            return;
        }
    };
    let payload = serde_json::to_vec(msg).expect("a message serializes");
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        + webpush::TOKEN_LIFETIME_S;
    let req = match webpush::request(&vapid, sub, &payload, expires) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "push not built");
            return;
        }
    };
    let mut b = client.post(req.endpoint.clone()).body(req.body);
    for (k, v) in &req.headers {
        b = b.header(*k, v);
    }
    match b.send().await {
        Ok(res) if res.status().is_success() => {}
        Ok(res) if matches!(res.status().as_u16(), 404 | 410) => {
            tracing::info!(host = ?req.endpoint.host_str(), "push subscription gone; forgotten");
            if let Err(e) = unsubscribe(home, &sub.endpoint) {
                tracing::warn!(error = %e, "push subscription not forgotten");
            }
        }
        Ok(res) => {
            tracing::warn!(status = res.status().as_u16(), host = ?req.endpoint.host_str(), "push refused")
        }
        Err(e) => tracing::warn!(error = %e, host = ?req.endpoint.host_str(), "push not sent"),
    }
}

/// Pushes what `changes` (one committed transaction's inbox changes) made
/// for the owner to the owner's devices. Reads nothing while no device is
/// subscribed.
pub async fn announce(s: &AppState, changes: Vec<InboxChange>) {
    if s.hosted.is_some() || !changes.iter().any(|c| c.made) {
        return;
    }
    match subscriptions(&s.home) {
        Ok(subs) if !subs.is_empty() => {}
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "push subscriptions not read");
            return;
        }
    }
    match s.store.call(move |db| messages(db, &changes)).await {
        Ok(msgs) => send_all(&s.home, &msgs).await,
        Err(e) => tracing::warn!(error = %e, "inbox change not pushed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(kind: Kind) -> ItemRow {
        ItemRow {
            seq: 1,
            id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".into(),
            kind,
            artifact_id: Some("7q3k9mzx2b4t".into()),
            thread_id: Some("t1".into()),
            comment_id: None,
            version_n: None,
            question_id: None,
            session_id: Some("s1".into()),
            harness: Some("claude".into()),
            detail: None,
            created_at: "2026-10-27T00:00:00Z".into(),
            read_at: None,
            recipient: None,
            request_id: None,
            grant_id: None,
        }
    }

    #[test]
    fn a_push_is_a_title_a_shell_url_and_a_tag() {
        let v = json!({"agent": {"name": "clax-claude", "harness": "claude"}, "url": "/a/7q3k9mzx2b4t?thread=t1"});
        assert_eq!(
            message(&item(Kind::Reply), &v),
            Some(Message {
                title: "clax-claude replied to you".into(),
                url: "/a/7q3k9mzx2b4t?thread=t1".into(),
                tag: "clax-inbox-7q3k9mzx2b4t".into(),
            })
        );
        let mut q = item(Kind::Question);
        q.question_id = Some("q_1".into());
        let m = message(
            &q,
            &json!({"agent": {"harness": "codex"}, "url": "/inbox?q=q_1"}),
        )
        .unwrap();
        assert_eq!(m.title, "codex asked you a question");
        assert_eq!(m.tag, "clax-inbox-q_1");
        let m = message(&item(Kind::Approval), &json!({"url": "/inbox?approval=r1"})).unwrap();
        assert_eq!(
            (m.title.as_str(), m.url.as_str()),
            ("An agent needs your approval", "/inbox?approval=r1")
        );
    }

    #[test]
    fn only_unread_owner_items_that_wait_on_the_owner_push() {
        let v = json!({"url": "/a/x"});
        for k in [
            Kind::Version,
            Kind::Published,
            Kind::Finished,
            Kind::Ended,
            Kind::Push,
        ] {
            assert_eq!(message(&item(k), &v), None, "{k:?}");
        }
        let mut person = item(Kind::Question);
        person.recipient = Some("p_1".into());
        assert_eq!(message(&person, &v), None);
        let mut read = item(Kind::Question);
        read.read_at = Some("2026-10-27T00:00:01Z".into());
        assert_eq!(message(&read, &v), None);
        assert_eq!(
            message(
                &item(Kind::Connection),
                &json!({"connection": {"via": "invite"}})
            ),
            None
        );
        assert!(
            message(
                &item(Kind::Connection),
                &json!({"connection": {"via": "request"}})
            )
            .is_some()
        );
    }

    #[test]
    fn titles_are_one_safe_line_and_urls_stay_on_the_shell() {
        let v = json!({"agent": {"name": "evil\u{202e}\nname\twith   gaps"}, "url": "https://evil.example/"});
        let m = message(&item(Kind::Reply), &v).unwrap();
        assert_eq!(m.title, "evil name with gaps replied to you");
        assert_eq!(m.url, "/inbox");
        assert_eq!(
            message(&item(Kind::Reply), &json!({"url": "//evil.example"}))
                .unwrap()
                .url,
            "/inbox"
        );
        let long = "a".repeat(200);
        let t = one_line(&long);
        assert_eq!(t.chars().count(), TITLE_MAX);
        assert!(t.ends_with('…'));
    }
}
