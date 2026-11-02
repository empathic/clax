//! A held browser confirmation ([`clax_http::confirm`]) offered as a chitin
//! item for the owner to approve or decline with a passkey, beside Clax's
//! own inbox card.
//!
//! While an agent is live, a decision made with the daemon token waits for
//! the owner's click in a browser of theirs on this machine. The fleet's
//! agents are always live, and the owner is often at another device, so
//! on a local daemon with `[sign]` (`confirm_to`, default `clax`,
//! [`clax_core::config::SignConfig`]) each confirmation is also queued with
//! the configured chitin CLI as an unsigned intent from the owner's title
//! (`chitin say --as <as> --queue <confirm_to> <text>`, as `CHITIN_TITLE=clax`,
//! with the four-field brief chitin requires). It appears in Basis's Needs
//! you, where the owner approves or declines it with a passkey
//! (chitin-approve, `docs/PROTOCOL.md` "Passkey approval" in
//! bdelanghe/chitin).
//!
//! - **Text and binding.** The item's text is the decision in words, the
//!   agents live, and a last line binding it:
//!   `clax confirmation <id> <kind> #<h>`, `h` being
//!   [`clax_http::confirm::binding`] of the confirmation's ID and its
//!   decision as recorded. The passkey signs that exact text (chitin binds
//!   the text into the WebAuthn challenge). Clax maps the item's ID to the
//!   confirmation it queued it for, and checks `h` against the confirmation
//!   again before answering: an approval of one item answers its own
//!   confirmation, never another.
//! - **The answer.** chitin records a passkey approval in the title's flow
//!   log (`<state_dir>/log/<as>.jsonl`) as `{side: "sent", op: "queue",
//!   from: <as>, to: <confirm_to>, outcome: "approved", ref: <item>, signer:
//!   "webauthn"}`, written only after the passkey's assertion verified;
//!   `outcome: "declined"` for a decline (by passkey, or `chitin decline` at
//!   the Mac). Clax tails that file (only while an item waits) and answers
//!   the confirmation through [`clax_http::confirm::answer`], the same
//!   transaction as a browser click, recording `via: "chitin"`, the item
//!   and the signer. An approval that is not a passkey's (no `signer:
//!   "webauthn"`, such as `chitin approve` with Touch ID at the Mac)
//!   answers nothing: the owner confirms that one in Clax. A decline only
//!   refuses, so any decline counts.
//! - **Once.** Whichever answers first (the browser, the passkey) answers
//!   it; the other finds it `confirmation_answered` and changes nothing. An
//!   item answered in the browser or expired stays in chitin's queue (chitin
//!   has no withdraw verb; Clax never declines in the owner's name): an
//!   approval of it later answers nothing.
//! - **Trust.** The flow log, like Clax's own database, is a file the
//!   owner's OS user can write: this adds no path an API caller holding the
//!   daemon token can use to approve. No route takes an approval.
//! - **Restart.** At start, items still waiting in `chitin pending` whose
//!   binding names a confirmation still open are tracked again. An item
//!   approved while Clax was down answers nothing (its confirmation expires;
//!   the agent asks again).
//! - **Off.** A hosted daemon, no `[sign]`, or `confirm = false` queues
//!   nothing. A chitin that fails is logged; the browser card still works.

use crate::routes::sign::{self, MAX_TEXT};
use clax_core::Event;
use clax_core::audit::{Actor, AuditCtx, Via};
use clax_core::config::SignConfig;
use clax_core::gitctx::GitField;
use clax_http::confirm;
use clax_http::state::AppState;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the flow log is read while an item waits.
pub const POLL: Duration = Duration::from_secs(1);
/// How long an item is tracked: past it, its confirmation has expired.
const TRACK_FOR: Duration =
    Duration::from_secs(2 * clax_store::confirmations::CONFIRMATION_TTL_SECS as u64);
/// The most a brief's `ask` holds (chitin's own limit).
const ASK_MAX: usize = 120;

/// Why the owner is asked (the brief's `why`).
pub const WHY: &str = "An agent of yours is live, and this was asked with Clax's daemon token, \
which your agents can read: Clax holds the decision until you answer.";
/// What approving does (the brief's `effect`).
pub const EFFECT: &str = "Approve: Clax lets this one decision through, once. Decline: Clax \
refuses it. Either way its card in Clax's inbox is answered too.";
/// How it is undone (the brief's `undo_how`).
pub const UNDO_HOW: &str = "A share, a site or a connection can be closed again in Clax; a push \
or an answer to an agent cannot be taken back.";

/// An item queued for a confirmation: the confirmation and its binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub confirmation: String,
    pub binding: String,
}

/// The binding line ending an item's text.
pub fn binding_line(id: &str, kind: &str, h: &str) -> String {
    format!("clax confirmation {id} {kind} #{h}")
}

/// The confirmation ID and binding an item's text ends with
/// ([`binding_line`]), if it is one of Clax's.
pub fn parse_binding(text: &str) -> Option<(String, String)> {
    let last = text.trim_end().lines().last()?;
    let mut w = last.strip_prefix("clax confirmation ")?.split(' ');
    let (id, _kind, h) = (w.next()?, w.next()?, w.next()?.strip_prefix('#')?);
    let ok_id =
        id.len() == 35 && id.starts_with("cf_") && id[3..].bytes().all(|b| b.is_ascii_hexdigit());
    let ok_h = h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit());
    (w.next().is_none() && ok_id && ok_h).then(|| (id.to_string(), h.to_string()))
}

/// `s` on one line: control characters as spaces, runs of spaces as one.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `s` cut to at most `n` characters, `…` marking a cut.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// The item's text for confirmation view `v` ([`confirm::view`]) and its
/// binding `h`: the decision in words, the agents live, the binding line;
/// at most [`MAX_TEXT`] characters, the words cut first and the binding
/// line never.
pub fn text(v: &Value, h: &str) -> String {
    let words = one_line(v["words"].as_str().unwrap_or("make a decision"));
    let names: Vec<String> = v["agents"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    let n = x["name"]
                        .as_str()
                        .or(x["harness"].as_str())
                        .unwrap_or("an agent");
                    one_line(n)
                })
                .collect()
        })
        .unwrap_or_default();
    let agents = match names.len() {
        0 => "Asked with Clax's daemon token while an agent of yours was live.".to_string(),
        1 => format!(
            "Asked with Clax's daemon token while this agent was live: {}.",
            cut(&names[0], 80)
        ),
        n => format!(
            "Asked with Clax's daemon token while these {n} agents were live: {}.",
            cut(&names.join(", "), 200)
        ),
    };
    let bind = binding_line(
        v["id"].as_str().unwrap_or_default(),
        v["decision"]["kind"].as_str().unwrap_or("decision"),
        h,
    );
    let head = "Confirm in Clax: ";
    let room = MAX_TEXT
        .saturating_sub(head.len() + agents.chars().count() + bind.chars().count() + 2)
        .max(16);
    format!("{head}{}\n{agents}\n{bind}", cut(&words, room))
}

/// `chitin say --queue`'s arguments for confirmation view `v` with binding
/// `h`, from `c.as_title` to `to`: the brief when this chitin takes one
/// (`brief`, [`takes_brief`]), then the recipient and the text ([`text`]).
/// A chitin without the brief reads `--ask` as the recipient, so it is
/// never sent one; a chitin with it refuses an item without.
pub fn queue_args(c: &SignConfig, to: &str, v: &Value, h: &str, brief: bool) -> Vec<String> {
    let words = one_line(v["words"].as_str().unwrap_or("make a decision"));
    let ask = cut(&format!("Confirm in Clax: {words}"), ASK_MAX);
    let head = ["say", "--as", &c.as_title, "--queue"];
    let brief_args = [
        "--ask",
        &ask,
        "--why",
        WHY,
        "--effect",
        EFFECT,
        "--undo",
        "partly",
        "--undo-how",
        UNDO_HOW,
    ];
    let brief_args: &[&str] = if brief { &brief_args } else { &[] };
    head.iter()
        .chain(brief_args)
        .chain([&to])
        .map(|s| s.to_string())
        .chain([text(v, h)])
        .collect()
}

/// Whether chitin's usage (`out`, what it prints run with no arguments)
/// names the queue's brief (`--queue --ask`).
pub fn usage_takes_brief(out: &str) -> bool {
    out.contains("--queue --ask")
}

/// Whether the configured chitin takes a queued item's brief: its usage,
/// read once when the task starts.
async fn takes_brief(c: &SignConfig) -> bool {
    let mut cmd = tokio::process::Command::new(&c.chitin);
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    if let Some(dir) = &c.state_dir {
        cmd.env("CHITIN_STATE_DIR", dir);
    }
    match tokio::time::timeout(Duration::from_secs(20), cmd.output()).await {
        Ok(Ok(o)) => {
            usage_takes_brief(&String::from_utf8_lossy(&o.stderr))
                || usage_takes_brief(&String::from_utf8_lossy(&o.stdout))
        }
        _ => false,
    }
}

/// What a flow-log line says of an item: approved with a passkey, or
/// declined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Decline,
}

/// The item a flow-log line `l` answers, with the verdict and its signer,
/// for items queued as `as_title` to `to` (see the module): a passkey
/// approval (`signer: "webauthn"`) or any decline. Anything else: `None`.
pub fn verdict(l: &Value, as_title: &str, to: &str) -> Option<(String, Verdict, String)> {
    if l["side"] != "sent" || l["op"] != "queue" || l["from"] != as_title {
        return None;
    }
    let item = l["ref"].as_str()?.to_string();
    let signer = l["signer"].as_str().unwrap_or("mac").to_string();
    match l["outcome"].as_str()? {
        "approved" if signer == "webauthn" && l["to"] == to => {
            Some((item, Verdict::Approve, signer))
        }
        "declined" => Some((item, Verdict::Decline, signer)),
        _ => None,
    }
}

/// The items still waiting in `chitin pending --json` output `items` that
/// Clax queued to `to` for a confirmation `open` says is still open with
/// that binding: what is tracked again after a restart.
pub fn recover(
    items: &[Value],
    to: &str,
    open: impl Fn(&str, &str) -> bool,
) -> HashMap<String, Held> {
    items
        .iter()
        .filter(|i| ours_waiting(i, to))
        .filter_map(|i| {
            let (cid, h) = parse_binding(i["text"].as_str()?)?;
            let item = i["id"].as_str()?.to_string();
            open(&cid, &h).then_some((
                item,
                Held {
                    confirmation: cid,
                    binding: h,
                },
            ))
        })
        .collect()
}

/// The flow log of chitin title `as_title` under `state_dir` (chitin's
/// default, `$XDG_STATE_HOME/chitin` or `~/.local/state/chitin`, for none).
pub fn flow_log(state_dir: Option<&str>, as_title: &str) -> Option<PathBuf> {
    let dir = match state_dir {
        Some(d) => PathBuf::from(d),
        None => match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x).join("chitin"),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".local/state/chitin"),
        },
    };
    Some(dir.join("log").join(format!("{as_title}.jsonl")))
}

/// The lines appended to a file since it was last read.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    /// From the file's end now: what is already there is not news.
    pub fn at_end(path: &Path) -> Tail {
        let offset = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Tail {
            path: path.to_path_buf(),
            offset,
            partial: Vec::new(),
        }
    }

    /// Forgets what was appended since the last read.
    pub fn skip(&mut self) {
        *self = Tail::at_end(&self.path);
    }

    /// The JSON lines appended since the last read (a file that shrank is
    /// read from its start; an unfinished last line waits for the next).
    pub fn read(&mut self) -> Vec<Value> {
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        let Ok(n) = f.take(len - self.offset).read_to_end(&mut buf) else {
            return Vec::new();
        };
        self.offset += n as u64;
        self.partial.extend_from_slice(&buf);
        let Some(end) = self.partial.iter().rposition(|b| *b == b'\n') else {
            return Vec::new();
        };
        let done: Vec<u8> = self.partial.drain(..=end).collect();
        String::from_utf8_lossy(&done)
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

/// The most of Clax's items that may wait in chitin's queue at once.
/// chitin holds at most 50 items for every queuer together and refuses
/// more, and an item of Clax's waits a day while its confirmation expires
/// in ten minutes: past this, a confirmation is not queued (the browser
/// card still asks), so Clax never fills the fleet's signing queue.
pub const MAX_WAITING: usize = 5;

/// How many items in `chitin pending --json` output `items` Clax queued to
/// `to` that still wait (neither declined nor expired).
pub fn waiting(items: &[Value], to: &str) -> usize {
    items.iter().filter(|i| ours_waiting(i, to)).count()
}

/// Whether chitin pending item `i` is one Clax queued to `to` that still
/// waits.
fn ours_waiting(i: &Value, to: &str) -> bool {
    i["to"] == to && i["queued_by"] == "clax" && i["declined"] != true && i["expired"] != true
}

/// Queues confirmation view `v` with chitin, unless [`MAX_WAITING`] of
/// Clax's items already wait: the item's ID and binding.
async fn queue(
    c: &SignConfig,
    to: &str,
    v: &Value,
    brief: bool,
) -> Result<(String, String), String> {
    let out = sign::chitin(c, &["pending", "--as", &c.as_title, "--json"])
        .await
        .map_err(|e| e.message)?;
    let items: Vec<Value> =
        serde_json::from_str(&out).map_err(|e| format!("chitin pending answered no list: {e}"))?;
    let n = waiting(&items, to);
    if n >= MAX_WAITING {
        return Err(format!(
            "{n} of Clax's items already wait in chitin's queue: not queued, confirm in Clax"
        ));
    }
    let (id, decision) = (v["id"].as_str().unwrap_or_default(), &v["decision"]);
    let h = confirm::binding(id, decision);
    let args = queue_args(c, to, v, &h, brief);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = sign::chitin(c, &refs).await.map_err(|e| e.message)?;
    let item = sign::queued_id(&out)
        .ok_or_else(|| format!("chitin say --queue named no item: {}", out.trim()))?;
    Ok((item, h))
}

/// Answers the confirmation `h` was queued for, as `verdict` from item
/// `item` signed by `signer`, if its binding still holds; publishes the
/// answer.
async fn answer(s: &AppState, h: Held, item: String, verdict: Verdict, signer: String) {
    let st = s.clone();
    let approve = verdict == Verdict::Approve;
    let cid = h.confirmation.clone();
    let r = s
        .store_call(move |db| {
            let Some(c) = db.confirmation(&h.confirmation)? else {
                return Ok(None);
            };
            if confirm::binding(&c.id, &c.decision) != h.binding {
                return Err(clax_core::CoreError::invalid(
                    "binding_mismatch",
                    "the item's binding is not this confirmation's",
                ));
            }
            let ctx = AuditCtx {
                actor: Actor::Owner {
                    public_id: db.owner_public_id()?.unwrap_or_else(|| "owner".into()),
                },
                via: Via::Daemon,
                git: GitField::Absent,
                call: None,
            };
            let how = [
                ("via", json!("chitin")),
                ("chitin_item", json!(item)),
                ("signer", json!(signer)),
            ];
            confirm::answer(&st, db, &ctx, &c.id, approve, &how).map(Some)
        })
        .await;
    match r {
        Ok(Some(v)) => {
            tracing::info!(confirmation = %cid, approve, "a confirmation answered through chitin");
            s.events.publish(Event::Confirmation { confirmation: v });
        }
        Ok(None) => {}
        // Answered in the browser first, expired, or not this one's.
        Err(e) => {
            tracing::info!(confirmation = %cid, code = e.code, "a chitin answer changed nothing")
        }
    }
}

/// Starts the queue-and-answer task on a local daemon with `[sign]` and
/// `confirm_to` (see the module); returns its handle, or none.
pub fn listen(s: &AppState) -> Vec<clax_rt::JoinHandle<()>> {
    if s.hosted.is_some() || s.embed.is_some() {
        return Vec::new();
    }
    let Some(c) = s.sign.clone() else {
        return Vec::new();
    };
    let Some(to) = c.confirm_to.clone() else {
        return Vec::new();
    };
    let Some(log) = flow_log(c.state_dir.as_deref(), &c.as_title) else {
        return Vec::new();
    };
    tracing::info!(to = %to, log = %log.display(), "offering held confirmations to chitin");
    // Subscribed now: a confirmation opened while the task starts is heard.
    let rx = s.events.subscribe();
    vec![clax_rt::spawn(run(s.clone(), c, to, log, rx))]
}

async fn run(
    s: AppState,
    c: Arc<SignConfig>,
    to: String,
    log: PathBuf,
    mut rx: tokio::sync::broadcast::Receiver<clax_core::Stamped>,
) {
    let mut tail = Tail::at_end(&log);
    let brief = takes_brief(&c).await;
    let (done_tx, mut done) = tokio::sync::mpsc::unbounded_channel();
    // Item ID → what it was queued for, and when.
    let mut held: HashMap<String, (Held, Instant)> = HashMap::new();
    // Confirmations queued, or being queued: each is queued once.
    let mut asked: HashSet<String> = HashSet::new();
    if let Ok(out) = sign::chitin(&c, &["pending", "--as", &c.as_title, "--json"]).await
        && let Ok(items) = serde_json::from_str::<Vec<Value>>(&out)
    {
        let open: HashMap<String, String> = s
            .store_call(|db| {
                Ok(db
                    .open_confirmations()?
                    .into_iter()
                    .map(|x| (confirm::binding(&x.id, &x.decision), x.id))
                    .collect())
            })
            .await
            .unwrap_or_default();
        for (item, h) in recover(&items, &to, |cid, b| open.get(b).is_some_and(|x| x == cid)) {
            asked.insert(h.confirmation.clone());
            held.insert(item, (h, Instant::now()));
        }
    }
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(e) => {
                    let Event::Confirmation { confirmation: v } = &e.event else { continue };
                    let Some(id) = v["id"].as_str() else { continue };
                    if v["status"] == "open" {
                        if asked.insert(id.to_string()) {
                            let (c, to, v, tx) = (c.clone(), to.clone(), v.clone(), done_tx.clone());
                            let id = id.to_string();
                            clax_rt::spawn(async move {
                                let _ = tx.send((id, queue(&c, &to, &v, brief).await));
                            });
                        }
                    } else {
                        held.retain(|_, (h, _)| h.confirmation != id);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "the chitin confirmations task lagged; events dropped");
                }
                Err(_) => return,
            },
            Some((cid, r)) = done.recv() => match r {
                Ok((item, binding)) => {
                    tracing::info!(confirmation = %cid, item = %item, "a held confirmation queued for the owner's passkey");
                    held.insert(item, (Held { confirmation: cid, binding }, Instant::now()));
                }
                Err(e) => tracing::warn!(confirmation = %cid, error = %e, "could not queue a held confirmation with chitin"),
            },
            _ = tick.tick() => {
                held.retain(|_, (_, at)| at.elapsed() < TRACK_FOR);
                if held.is_empty() {
                    tail.skip();
                    continue;
                }
                for l in tail.read() {
                    let Some((item, v, signer)) = verdict(&l, &c.as_title, &to) else { continue };
                    if let Some((h, _)) = held.remove(&item) {
                        answer(&s, h, item, v, signer).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CF: &str = "cf_0123456789abcdef0123456789abcdef";

    fn view(words: &str, agents: Value) -> Value {
        json!({"id": CF, "words": words, "agents": agents,
            "decision": {"kind": "push", "artifact_id": "a1"}})
    }

    #[test]
    fn the_text_ends_with_its_binding_and_fits_chitins_limit() {
        let h = "0011223344556677";
        let t = text(
            &view(
                "push “Notes” to https://clax.dev",
                json!([{"name": "goal-x", "harness": "claude"}]),
            ),
            h,
        );
        assert_eq!(
            t,
            format!(
                "Confirm in Clax: push “Notes” to https://clax.dev\nAsked with Clax's daemon token while this agent was live: goal-x.\nclax confirmation {CF} push #{h}"
            )
        );
        assert_eq!(parse_binding(&t), Some((CF.into(), h.into())));
        // User-controlled words: cut to fit, newlines and escapes flattened, the binding kept.
        let long = format!(
            "answer an agent's question “{}\n\u{1b}[2J”",
            "x".repeat(2000)
        );
        let t = text(&view(&long, json!([])), h);
        assert!(t.chars().count() <= MAX_TEXT, "{}", t.chars().count());
        assert_eq!(t.lines().count(), 3);
        assert!(!t.chars().any(|c| c.is_control() && c != '\n'));
        assert_eq!(parse_binding(&t), Some((CF.into(), h.into())));
    }

    #[test]
    fn only_a_well_formed_last_line_binds() {
        for t in [
            "clax confirmation cf_0123 push #0011223344556677",
            &format!("clax confirmation {CF} push #00112233"),
            &format!("clax confirmation {CF} push 0011223344556677"),
            &format!("clax confirmation {CF} push #0011223344556677 extra"),
            &format!("clax confirmation {CF} push #0011223344556677\nmore"),
        ] {
            assert_eq!(parse_binding(t), None, "{t}");
        }
    }

    #[test]
    fn the_queue_carries_the_brief_chitin_requires() {
        let c = SignConfig {
            chitin: "chitin".into(),
            as_title: "bobby".into(),
            state_dir: None,
            confirm_to: Some("clax".into()),
        };
        let v = view("x".repeat(300).as_str(), json!([]));
        // A chitin without the brief gets none: it would read --ask as the recipient.
        let old = queue_args(&c, "clax", &v, "0011223344556677", false);
        assert_eq!(&old[..5], ["say", "--as", "bobby", "--queue", "clax"]);
        assert_eq!(old.len(), 6);
        // Usage as each chitin prints it (deployed 6h32ad23, 2026-11-02; and
        // approve-into-fold 710bddd).
        assert!(!usage_takes_brief(
            "| say --as <title> --queue [--conversation <id>] [--in-reply-to <id>] <to> <text> |"
        ));
        assert!(usage_takes_brief(
            "say --as <title> --queue --ask <line> --why <text> --effect <text> --undo yes|no|partly [--undo-how <text>]"
        ));
        let a = queue_args(&c, "clax", &v, "0011223344556677", true);
        assert_eq!(&a[..5], ["say", "--as", "bobby", "--queue", "--ask"]);
        assert!(a[5].chars().count() <= ASK_MAX && a[5].ends_with('…'));
        assert_eq!(
            &a[6..14],
            [
                "--why",
                WHY,
                "--effect",
                EFFECT,
                "--undo",
                "partly",
                "--undo-how",
                UNDO_HOW
            ]
        );
        assert_eq!(a[14], "clax");
        assert!(a[15].starts_with("Confirm in Clax: "));
        assert!(WHY.len() <= 300 && EFFECT.len() <= 300 && UNDO_HOW.len() <= 200);
    }

    #[test]
    fn a_passkey_approval_or_any_decline_answers_and_nothing_else() {
        let line = |o: &str, extra: Value| {
            let mut l = json!({"ts": "t", "side": "sent", "op": "queue", "from": "bobby",
                "to": "clax", "outcome": o, "ref": "0123456789ab"});
            l.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            l
        };
        let item = || "0123456789ab".to_string();
        assert_eq!(
            verdict(
                &line("approved", json!({"signer": "webauthn"})),
                "bobby",
                "clax"
            ),
            Some((item(), Verdict::Approve, "webauthn".into()))
        );
        assert_eq!(
            verdict(
                &line("declined", json!({"signer": "webauthn"})),
                "bobby",
                "clax"
            ),
            Some((item(), Verdict::Decline, "webauthn".into()))
        );
        // `chitin decline` at the Mac: to "-", no signer.
        assert_eq!(
            verdict(&line("declined", json!({"to": "-"})), "bobby", "clax"),
            Some((item(), Verdict::Decline, "mac".into()))
        );
        for l in [
            line("approved", json!({})), // Touch ID at the Mac
            line("approved", json!({"signer": "se"})),
            line("approved", json!({"signer": "webauthn", "to": "elsewhere"})),
            line("approved", json!({"signer": "webauthn", "from": "alex"})),
            line(
                "approved",
                json!({"signer": "webauthn", "side": "received"}),
            ),
            line("queued", json!({})),
            line("approve-refused", json!({"signer": "webauthn"})),
            line("approve-undelivered", json!({"signer": "webauthn"})),
            line("decline-refused", json!({"signer": "webauthn"})),
            json!({"side": "sent", "op": "say", "from": "bobby", "to": "clax", "outcome": "approved", "ref": "x", "signer": "webauthn"}),
        ] {
            assert_eq!(verdict(&l, "bobby", "clax"), None, "{l}");
        }
    }

    #[test]
    fn a_restart_tracks_again_only_clax_items_for_open_confirmations_with_their_binding() {
        let h = "0011223344556677";
        let item = |id: &str, extra: Value| {
            let mut i = json!({"id": id, "as": "bobby", "to": "clax", "queued_by": "clax",
                "text": format!("Confirm in Clax: x\nAsked.\n{}", binding_line(CF, "push", h)),
                "declined": false, "expired": false});
            i.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            i
        };
        let items = [
            item("aaaaaaaaaaaa", json!({})),
            item("bbbbbbbbbbbb", json!({"declined": true})),
            item("cccccccccccc", json!({"expired": true})),
            item("dddddddddddd", json!({"to": "lander"})),
            item("eeeeeeeeeeee", json!({"queued_by": "personal-super-human"})),
            item("ffffffffffff", json!({"text": "no binding"})),
        ];
        let got = recover(&items, "clax", |cid, b| cid == CF && b == h);
        assert_eq!(
            got,
            HashMap::from([(
                "aaaaaaaaaaaa".to_string(),
                Held {
                    confirmation: CF.into(),
                    binding: h.into()
                }
            )])
        );
        assert!(recover(&items, "clax", |_, b| b != h).is_empty());
    }

    #[test]
    fn the_tail_reads_only_whole_lines_appended_since() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bobby.jsonl");
        std::fs::write(&p, "{\"old\":1}\n").unwrap();
        let mut t = Tail::at_end(&p);
        assert!(t.read().is_empty());
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "{{\"a\":1}}\nnot json\n{{\"b\":").unwrap();
        assert_eq!(t.read(), vec![json!({"a": 1})]);
        writeln!(f, "2}}").unwrap();
        assert_eq!(t.read(), vec![json!({"b": 2})]);
        // A file that shrank (rotated) is read from its start.
        std::fs::write(&p, "{\"c\":3}\n").unwrap();
        assert_eq!(t.read(), vec![json!({"c": 3})]);
        // Skipped lines are not news.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"{\"d\":4}\n")
            .unwrap();
        t.skip();
        assert!(t.read().is_empty());
        // No file yet: nothing, and the file once it exists.
        let mut none = Tail::at_end(&dir.path().join("later.jsonl"));
        assert!(none.read().is_empty());
        std::fs::write(dir.path().join("later.jsonl"), "{\"e\":5}\n").unwrap();
        assert_eq!(none.read(), vec![json!({"e": 5})]);
    }

    #[test]
    fn only_claxs_waiting_items_count_toward_the_cap() {
        let i = |extra: Value| {
            let mut v =
                json!({"to": "clax", "queued_by": "clax", "declined": false, "expired": false});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let items = [
            i(json!({})),
            i(json!({})),
            i(json!({"declined": true})),
            i(json!({"expired": true})),
            i(json!({"queued_by": "personal-super-human"})),
            i(json!({"to": "lander"})),
        ];
        assert_eq!(waiting(&items, "clax"), 2);
    }

    #[test]
    fn the_flow_log_is_the_titles_under_the_state_dir() {
        assert_eq!(
            flow_log(Some("/s/chitin"), "bobby"),
            Some(PathBuf::from("/s/chitin/log/bobby.jsonl"))
        );
    }
}
