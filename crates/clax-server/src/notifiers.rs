//! Delivery of a queue's notices ([`clax_store::queue_notifiers`]). A pass is
//! [`deliver_due`], and nothing in it is a host's own: the daemon runs it on a
//! timer ([`spawn_delivery`]), and a host with no timer of its own calls it and
//! wakes by [`clax_store::Store::notices_next_due`].
//!
//! What a message says is decided here and nowhere else. A stream's readers
//! are not known to be the queue's, so a stream gets fixed words, the report's
//! ID and a link to the report in Clax, which only the queue's people can open:
//!
//! - A fix to check goes to its reporter by direct message when the
//!   notifier's `people` maps their GitHub login, with the first line of what
//!   they filed, the preview link and where they answer, whatever the run. For
//!   anyone else, and when Zulip refuses that message, the stream is told: no
//!   reporter's name, no preview link, no pull request, no report text.
//! - A held fix goes to the stream alone, naming what a person must do: the
//!   pull request and, where its target says, what deploys its preview. From a
//!   run that may hold confidential material it names neither.
//! - A wait that ended with no preview goes to the stream alone, in place of a
//!   held fix's message not yet sent: a preview deployed now reaches nobody,
//!   and a member hands the fix over with a link of their own. It names what a
//!   held fix's message does, on the same terms.
//! - A rejected fix goes to the stream alone, with none of what the reopen said.
//!
//! No prose a model wrote is sent: not a fix's summary, not a note. A link can
//! be an agent's own, words and all, and Zulip's mention and link processors
//! run before its autolink, so what they are made of is percent-encoded out of
//! every link ([`link`]). What a person wrote is shown as code, so it mentions
//! nobody and links nowhere ([`as_code`]). What deploys a preview is the
//! target's own sentence, sent as it wrote it, markup and all.
//!
//! A link [`link`] refuses goes unsent, and the panel says so: a message that
//! lost it goes, and its words stand as that channel's failure.
//!
//! A failure is kept in fixed words, for the stream and direct messages apart,
//! and the notice tried again. Only an answer that says the site is unwell
//! ends a queue's turn in a pass ([`Failure::down`]); a refused message fails
//! that notice alone. What a server answered is never kept or logged, and a
//! transport's message, which may name the URL, is dropped: the key is in a
//! header no message carries.

use clax_core::model::AUTHOR_AGENT;
use clax_core::queue::Report;
use clax_core::report::{FixAttempt, FixOutcome};
use clax_http::state::AppState;
use clax_sample::form::{FormAnswer, post_form};
use clax_store::Store;
use clax_store::queue_notifiers::{DueNotice, FixLinks, NoticeChannel, NoticeEvent, OpenedZulip};
use clax_store::queues::Visibility;
use clax_store::secrets::Kek;
use std::collections::HashMap;
use std::time::Duration;

/// How often the daemon looks for what is due.
pub const CHECK_EVERY: Duration = Duration::from_secs(15);
/// The most notices one pass takes.
pub const BATCH: u32 = 10;
/// The longest one message may take, its answer included.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// The most of a report's first line, or a queue's name, a message shows.
const MAX_SHOWN_CHARS: usize = 120;

/// The panel's words for a key Zulip refused (401, 403).
pub const KEY_REFUSED: &str = "Zulip refused the bot's email or API key.";
/// For a message Zulip refused (any other 4xx, or a 200 saying `error`).
pub const MESSAGE_REFUSED: &str =
    "Zulip refused a message: check the stream's name and the people's addresses.";
/// For a 429.
pub const RATE_LIMITED: &str = "Zulip asked Clax to slow down.";
/// For no answer, a server's error, or an answer that is not Zulip's.
pub const UNREACHABLE: &str = "Zulip could not be reached.";
/// For a 3xx, which is never followed.
pub const REDIRECTED: &str =
    "The site answered with a redirect, which Clax does not follow: check its address.";
/// For a site this instance's rule no longer allows.
pub const SITE_REFUSED: &str = "This Clax may no longer contact the site: set a public address.";
/// For a key the store cannot give.
pub const NO_KEY: &str = "The bot's API key could not be read: set it again.";
/// For a fix whose link [`link`] refuses, whose reporter had a direct message
/// coming.
pub const FIX_LINK_UNUSABLE: &str =
    "A fix's link could not be put in a message, so the stream was told instead of its reporter.";
/// For a held fix whose pull request [`link`] refuses.
pub const HELD_LINK_UNUSABLE: &str =
    "A held fix's pull request could not be put in a message, which went without it.";

const TEST_MESSAGE: &str = "This is a test message from Clax. Notices for this queue arrive here.";
const TEST_TOPIC: &str = "Clax test";

/// Who a message goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum To {
    /// A person, by Zulip email.
    Direct(String),
    /// The notifier's stream, under a topic.
    Stream { stream: String, topic: String },
}

/// A message, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub to: To,
    pub content: String,
}

impl Message {
    fn channel(&self) -> NoticeChannel {
        match self.to {
            To::Direct(_) => NoticeChannel::Direct,
            To::Stream { .. } => NoticeChannel::Stream,
        }
    }
}

/// What a notice sends: `first`, and `then` should Zulip refuse it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    first: Message,
    then: Option<Message>,
    /// What a link [`link`] refused cost, for the panel once a message went.
    lost_link: Option<(NoticeChannel, &'static str)>,
}

/// Why a message did not go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    /// Its fixed words.
    pub words: &'static str,
    /// How long the server asked to be left alone.
    pub wait_secs: Option<i64>,
    /// That the site is unwell, not this message: a 401, a 403, a 429, a 5xx,
    /// a 3xx or no answer. Nothing else is worth holding a queue's other
    /// notices back for, or falling back from.
    pub down: bool,
}

impl Failure {
    fn down(words: &'static str) -> Failure {
        Failure {
            words,
            wait_secs: None,
            down: true,
        }
    }

    fn refused(words: &'static str) -> Failure {
        Failure {
            words,
            wait_secs: None,
            down: false,
        }
    }
}

/// Whether `c` has no place in a line a person reads: a control character, a
/// line or paragraph separator, or one that reorders or hides the text around
/// it.
fn unprintable(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// `s` as inline code: no backtick, nothing [`unprintable`], at most
/// [`MAX_SHOWN_CHARS`]. `None` when nothing is left.
fn as_code(s: &str) -> Option<String> {
    let clean: String = s
        .chars()
        .filter(|c| *c != '`' && !unprintable(*c))
        .take(MAX_SHOWN_CHARS)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| format!("`{clean}`"))
}

/// `s`, a URL's ASCII, with ``* [ ] ( ) $ < > ` `` percent-encoded: with those
/// gone Zulip's mention, channel, link, maths and code syntax has nothing to
/// open with, and the URL parser leaves some of them as they came. All else
/// stays, as a site may tell `_ ~ : ; , @ !` from their encoded forms.
fn inert(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b"*[]()$<>`".contains(&b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(char::from(b));
        }
    }
    out
}

/// `u` as a link safe to put in a message, when it is `https:` or `http:`,
/// holds no user name or password, and its host is a plain name or an
/// address: a host cannot be encoded, so one with anything else is no link.
fn link(u: &str) -> Option<String> {
    let u = url::Url::parse(u).ok()?;
    if !matches!(u.scheme(), "https" | "http") || !u.username().is_empty() || u.password().is_some()
    {
        return None;
    }
    let host = match u.host()? {
        url::Host::Domain(d) => d
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
            .then(|| d.to_string())?,
        url::Host::Ipv4(a) => a.to_string(),
        url::Host::Ipv6(a) => format!("[{a}]"),
    };
    let port = u.port().map_or(String::new(), |p| format!(":{p}"));
    let rest = inert(&u[url::Position::BeforePath..]);
    Some(format!("{}://{host}{port}{rest}", u.scheme()))
}

/// The first line of what the report's filer wrote, as code.
fn title(report: &Report) -> Option<String> {
    let first = report
        .comments
        .first()
        .filter(|c| c.author_kind != AUTHOR_AGENT && !c.triage_only)?;
    as_code(first.body.lines().find(|l| !l.trim().is_empty())?)
}

/// Where `report`'s reporter answers, from `base`: an overlay files a preview's
/// words into the preview's own instance, so not there. The shell of this
/// instance opens My reports at this address, and any other site's overlay is on
/// the page the report was filed from. `None` when no message can carry it.
fn my_reports(base: &str, report: &Report) -> Option<String> {
    let page = url::Url::parse(&report.artifact.url).ok()?;
    let own = url::Url::parse(base).is_ok_and(|b| b.origin() == page.origin());
    if own {
        return link(&format!("{}/#my-reports", base.trim_end_matches('/')));
    }
    link(page.as_str())
}

/// Where the queue's people read `report`, from `base`. Opening it asks for
/// membership of the queue, so a stream may have it.
fn open_report(base: &str, report: &Report) -> String {
    let url = format!(
        "{}/queues/{}/{}",
        base.trim_end_matches('/'),
        report.queue_id,
        report.id
    );
    link(&url).map_or("Open the report in Clax.".to_string(), |l| {
        format!("Open the report in Clax: {l}")
    })
}

/// `content` for the stream, under a topic of `report`'s ID.
fn on_stream(z: &OpenedZulip, report: &Report, content: String) -> Message {
    Message {
        to: To::Stream {
            stream: z.stream.clone(),
            topic: report.id.clone(),
        },
        content,
    }
}

/// What a fix with the link `url`, if it has a usable one, sends. A mapped
/// reporter with no usable link is told as anyone is, and the panel hears why.
fn fix_to_check(z: &OpenedZulip, report: &Report, base: &str, url: Option<&str>) -> Plan {
    let content = format!(
        "Report {} has something to check. {}",
        report.id,
        open_report(base, report)
    );
    let stream = on_stream(z, report, content);
    let email = report
        .reporter_profile
        .as_ref()
        .and_then(|p| p.github.as_deref())
        .and_then(|login| z.people.get(&login.to_ascii_lowercase()));
    match (email, url) {
        (Some(email), Some(url)) => {
            let what = title(report).map_or(String::new(), |t| format!(" {t}"));
            let reports = my_reports(base, report).map_or(String::new(), |l| format!(": {l}"));
            Plan {
                first: Message {
                    to: To::Direct(email.clone()),
                    content: format!(
                        "Your report{what} has a fix to check: {url}\n\nTry it, then say whether \
                         it works under My reports, where you filed it{reports}"
                    ),
                },
                then: Some(stream),
                lost_link: None,
            }
        }
        (email, _) => Plan {
            first: stream,
            then: None,
            lost_link: email.map(|_| (NoticeChannel::Direct, FIX_LINK_UNUSABLE)),
        },
    }
}

/// What `links`, if the message may have them, say of a held fix's pull
/// request: the words, and what a link [`link`] refused cost.
fn pull_request(links: Option<&FixLinks>) -> (String, Option<(NoticeChannel, &'static str)>) {
    let Some(url) = links.and_then(|l| l.url.as_deref()) else {
        return (String::new(), None);
    };
    link(url).map_or(
        (
            String::new(),
            Some((NoticeChannel::Stream, HELD_LINK_UNUSABLE)),
        ),
        |pull| (format!("\n\nPull request: {pull}"), None),
    )
}

/// The message telling the queue's people that a fix waits for one of them.
fn fix_held(
    z: &OpenedZulip,
    report: &Report,
    base: &str,
    queue: &str,
    links: Option<&FixLinks>,
    confidential: bool,
) -> Plan {
    let name = as_code(queue).unwrap_or_default();
    let mut content = format!(
        "A fix for report {} in {name} is held for a person.",
        report.id
    );
    let links = links.filter(|_| !confidential);
    let (pull, lost_link) = pull_request(links);
    content.push_str(&pull);
    if let Some(step) = links.and_then(|l| l.preview_step.as_deref()) {
        content.push_str(&format!(
            "\n{step}\nNothing tells its reporter until the preview answers."
        ));
    }
    content.push_str(&format!(
        "\n\n{} It says what the fix changed, and what to decide.",
        open_report(base, report)
    ));
    Plan {
        first: on_stream(z, report, content),
        then: None,
        lost_link,
    }
}

/// The message telling the queue's people that the wait for a fix's preview
/// ended with none seen, which does not say none is up. It may be the first
/// they hear of the fix, so it holds what [`fix_held`] would, and from a
/// confidential run as little.
fn fix_wait_ended(
    z: &OpenedZulip,
    report: &Report,
    base: &str,
    queue: &str,
    links: Option<&FixLinks>,
    confidential: bool,
) -> Plan {
    let name = as_code(queue).unwrap_or_default();
    let mut content = format!(
        "ClaxBot stopped watching for a preview of a fix for report {} in {name}, without seeing \
         one up.",
        report.id
    );
    let links = links.filter(|_| !confidential);
    let (pull, lost_link) = pull_request(links);
    content.push_str(&pull);
    if let Some(step) = links.and_then(|l| l.preview_step.as_deref()) {
        content.push_str(&format!("\n{step}"));
    }
    content.push_str(&format!(
        "\n\nA preview that is up or deployed from now on reaches nobody: nothing watches for it, \
         so its reporter is not told. To tell them, a member of the queue hands the fix over, \
         proposing one of their own with the preview's link. {}",
        open_report(base, report)
    ));
    Plan {
        first: on_stream(z, report, content),
        then: None,
        lost_link,
    }
}

/// The message telling the queue's people that a fix was rejected. What the
/// reopen said stays on the report.
fn fix_rejected(z: &OpenedZulip, report: &Report, base: &str, queue: &str) -> Plan {
    let name = as_code(queue).unwrap_or_default();
    let content = format!(
        "A fix for report {} in {name} was rejected. {}",
        report.id,
        open_report(base, report)
    );
    Plan {
        first: on_stream(z, report, content),
        then: None,
        lost_link: None,
    }
}

/// What `n` calls for now, or `None` when what it tells is no longer so: the
/// report is gone, the fix moved on, its link changed, or the wait ended for a
/// held fix's message, which speaks of it as going on, or was never over after
/// all for the message that says it is. That a fix was rejected stays so.
fn plan_of(
    st: &Store,
    z: &OpenedZulip,
    base: &str,
    n: &DueNotice,
) -> clax_core::Result<Option<Plan>> {
    let Some(report) = st.report(&n.queue_id, &n.thread_id, Visibility::All)? else {
        return Ok(None);
    };
    let fix: Option<&FixAttempt> = report.fixes.iter().find(|f| f.n == n.n);
    if n.event == NoticeEvent::FixRejected {
        let queue = st.queue(&n.queue_id)?.map(|q| q.name).unwrap_or_default();
        return Ok(fix.map(|_| fix_rejected(z, &report, base, &queue)));
    }
    let Some(fix) = fix.filter(|f| f.outcome == FixOutcome::Pending) else {
        return Ok(None);
    };
    let missed = fix.members.as_ref().is_some_and(|m| m.preview_missed);
    let ended = n.event == NoticeEvent::FixWaitEnded;
    Ok(match n.event {
        NoticeEvent::FixToCheck => {
            let current = fix.fix.verify_url.as_deref().filter(|_| !fix.held);
            (current == Some(n.link.as_str())).then(|| {
                let url = current.and_then(link);
                fix_to_check(z, &report, base, url.as_deref())
            })
        }
        NoticeEvent::FixHeld | NoticeEvent::FixWaitEnded if fix.held && missed == ended => {
            let queue = st.queue(&n.queue_id)?.map(|q| q.name).unwrap_or_default();
            let links = st.fix_links(&n.thread_id, n.n)?;
            let confidential = st.fix_is_confidential(&n.thread_id, n.n)?;
            let message = if ended { fix_wait_ended } else { fix_held };
            Some(message(
                z,
                &report,
                base,
                &queue,
                links.as_ref(),
                confidential,
            ))
        }
        NoticeEvent::FixHeld | NoticeEvent::FixWaitEnded | NoticeEvent::FixRejected => None,
    })
}

/// What an answer means: `Ok` for Zulip's success alone.
fn judge(a: &FormAnswer) -> Result<(), Failure> {
    match a.status {
        200 => {
            let said: Option<serde_json::Value> = serde_json::from_slice(&a.body).ok();
            match said.as_ref().map(|v| &v["result"]) {
                Some(r) if r == "success" => Ok(()),
                Some(r) if r == "error" => Err(Failure::refused(MESSAGE_REFUSED)),
                _ => Err(Failure::refused(UNREACHABLE)),
            }
        }
        0 | 300..=399 => Err(Failure::down(REDIRECTED)),
        401 | 403 => Err(Failure::down(KEY_REFUSED)),
        429 => Err(Failure {
            wait_secs: a.retry_after.as_deref().and_then(|s| s.trim().parse().ok()),
            ..Failure::down(RATE_LIMITED)
        }),
        400..=499 => Err(Failure::refused(MESSAGE_REFUSED)),
        500..=599 => Err(Failure::down(UNREACHABLE)),
        _ => Err(Failure::refused(UNREACHABLE)),
    }
}

/// `m` as Zulip's form. `type=direct` needs Zulip 7.0 (feature level 174); an
/// older server refuses it, and the message falls back.
fn form_of(m: &Message) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    match &m.to {
        To::Direct(email) => {
            form.append_pair("type", "direct");
            form.append_pair("to", &serde_json::json!([email]).to_string());
        }
        To::Stream { stream, topic } => {
            form.append_pair("type", "stream");
            form.append_pair("to", stream);
            form.append_pair("topic", topic);
        }
    }
    form.append_pair("content", &m.content);
    form.finish()
}

/// Sends `m` as `z`'s bot. Natively a site its owner did not set is reached
/// at public addresses alone.
///
/// # Errors
/// [`Failure`], in fixed words.
pub async fn send(z: &OpenedZulip, m: &Message) -> Result<(), Failure> {
    use base64::Engine as _;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!(
        "{}:{}",
        z.bot_email,
        z.api_key.expose()
    ));
    let headers = [("authorization", format!("Basic {basic}"))];
    let url = format!("{}/api/v1/messages", z.site);
    let post = post_form(&url, &headers, form_of(m), !z.local_site);
    match clax_rt::time::timeout(SEND_TIMEOUT, post).await {
        Ok(Ok(answer)) => judge(&answer),
        Ok(Err(_)) | Err(_) => Err(Failure::down(UNREACHABLE)),
    }
}

/// How each message of a plan went, in the order sent.
type Told = Vec<(NoticeChannel, Result<(), Failure>)>;

/// Sends `p`, falling back when Zulip refused its first message and is
/// otherwise well: from a site that is down the notice waits instead.
async fn tell(z: &OpenedZulip, p: &Plan) -> Told {
    let first = send(z, &p.first).await;
    let mut told = vec![(p.first.channel(), first)];
    if let (Err(f), Some(then)) = (first, &p.then)
        && !f.down
    {
        told.push((then.channel(), send(z, then).await));
    }
    told
}

/// Why a notifier cannot be used: its `rev` if it has one, and the panel's
/// words.
type Blocked = (Option<String>, &'static str);

/// Queue `qid`'s notifier with its key, or why not.
fn opened(st: &Store, kek: &Kek, qid: &str) -> clax_core::Result<Result<OpenedZulip, Blocked>> {
    let words = match st.open_zulip_notifier(kek, qid) {
        Ok(Some(z)) => return Ok(Ok(z)),
        Ok(None) => NO_KEY,
        Err(clax_core::CoreError::Invalid {
            code: "invalid_site",
            ..
        }) => SITE_REFUSED,
        Err(clax_core::CoreError::Invalid { .. }) => NO_KEY,
        Err(e) => return Err(e),
    };
    Ok(Err((st.zulip_notifier(qid)?.map(|z| z.rev), words)))
}

/// What to do with a taken notice.
enum Step {
    Send(Box<OpenedZulip>, Plan),
    Drop,
    Blocked(Blocked),
}

fn step_of(st: &Store, kek: &Kek, base: &str, n: &DueNotice) -> clax_core::Result<Step> {
    let z = match opened(st, kek, &n.queue_id)? {
        Ok(z) => z,
        Err(blocked) => return Ok(Step::Blocked(blocked)),
    };
    Ok(match plan_of(st, &z, base, n)? {
        Some(p) => Step::Send(Box::new(z), p),
        None => Step::Drop,
    })
}

/// Where this instance's own pages are.
fn base_of(s: &AppState) -> String {
    s.hosted
        .as_ref()
        .map_or_else(|| s.browser_base.clone(), |h| h.origin.clone())
}

/// One pass: takes up to [`BATCH`] notices due now, a turn for each queue
/// before any second, and sends each. A queue whose site is down
/// ([`Failure::down`]) is tried no more this pass, so a site that hangs costs
/// one [`SEND_TIMEOUT`]. Answers how many went. A store's error ends the pass;
/// what it had taken comes due again by itself. A host with no KEK has nothing
/// to send with, and takes nothing.
pub async fn deliver_due(s: &AppState) -> usize {
    let Some(kek) = s.bot.kek.clone() else {
        return 0;
    };
    let due = match s.store.call(|st| st.take_due_notices(BATCH)).await {
        Ok(due) => due,
        Err(e) => {
            tracing::warn!(error = %e, "taking due notices failed");
            return 0;
        }
    };
    let base = base_of(s);
    let mut sent = 0;
    let mut down: HashMap<String, Option<i64>> = HashMap::new();
    for n in due {
        if let Some(wait) = down.get(&n.queue_id).copied() {
            let id = n.id.clone();
            let _ = s.store.call(move |st| st.retry_notice(&id, wait)).await;
            continue;
        }
        let (k, b, notice) = (kek.clone(), base.clone(), n.clone());
        let step = s.store.call(move |st| step_of(st, &k, &b, &notice)).await;
        let (id, qid) = (n.id.clone(), n.queue_id.clone());
        let done = match step {
            Ok(Step::Drop) => s.store.call(move |st| st.drop_notice(&id)).await,
            Ok(Step::Blocked((rev, words))) => {
                down.insert(n.queue_id.clone(), None);
                s.store
                    .call(move |st| {
                        if let Some(rev) = rev {
                            st.note_notifier_failure(&qid, &rev, NoticeChannel::Stream, words)?;
                        }
                        st.retry_notice(&id, None)
                    })
                    .await
            }
            Ok(Step::Send(z, plan)) => {
                let told = clax_rt::sendable(async { tell(&z, &plan).await }).await;
                let went = told.iter().any(|(_, r)| r.is_ok());
                sent += usize::from(went);
                let mut failed = told.iter().filter_map(|(_, r)| r.err());
                if let Some(f) = failed.clone().find(|f| f.down) {
                    down.insert(n.queue_id.clone(), f.wait_secs);
                }
                let wait = failed.next_back().and_then(|f| f.wait_secs);
                let rev = z.rev.clone();
                let lost_link = plan.lost_link;
                s.store
                    .call(move |st| {
                        for (channel, result) in told {
                            match result {
                                Ok(()) => st.note_notifier_ok(&qid, &rev, channel)?,
                                Err(f) => {
                                    st.note_notifier_failure(&qid, &rev, channel, f.words)?;
                                }
                            }
                        }
                        if went {
                            if let Some((channel, words)) = lost_link {
                                st.note_notifier_failure(&qid, &rev, channel, words)?;
                            }
                            st.notice_sent(&id)
                        } else {
                            st.retry_notice(&id, wait)
                        }
                    })
                    .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = done {
            tracing::warn!(error = %e, "a notice's delivery could not be recorded");
            return sent;
        }
    }
    sent
}

/// Sends the test message of queue `qid`'s notifier to its stream, by its
/// saved settings, and notes how it went, for its panel. It tries no direct
/// message.
///
/// # Errors
/// [`Failure`], in fixed words. A store's error is [`NO_KEY`]'s.
pub async fn send_test(s: &AppState, kek: Kek, qid: String) -> Result<(), Failure> {
    let q = qid.clone();
    let z = match s.store.call(move |st| opened(st, &kek, &q)).await {
        Ok(Ok(z)) => z,
        Ok(Err((rev, words))) => {
            let noted = s
                .store
                .call(move |st| match rev {
                    Some(rev) => st.note_notifier_failure(&qid, &rev, NoticeChannel::Stream, words),
                    None => Ok(()),
                })
                .await;
            if let Err(e) = noted {
                tracing::warn!(error = %e, "a test message's outcome could not be recorded");
            }
            return Err(Failure::down(words));
        }
        Err(_) => return Err(Failure::down(NO_KEY)),
    };
    let m = Message {
        to: To::Stream {
            stream: z.stream.clone(),
            topic: TEST_TOPIC.to_string(),
        },
        content: TEST_MESSAGE.to_string(),
    };
    let told = clax_rt::sendable(async { send(&z, &m).await }).await;
    let (rev, outcome) = (z.rev.clone(), told);
    let noted = s
        .store
        .call(move |st| match outcome {
            Ok(()) => st.note_notifier_ok(&qid, &rev, NoticeChannel::Stream),
            Err(f) => st.note_notifier_failure(&qid, &rev, NoticeChannel::Stream, f.words),
        })
        .await;
    if let Err(e) = noted {
        tracing::warn!(error = %e, "a test message's outcome could not be recorded");
    }
    told
}

/// Delivers on the daemon: a pass every `every`, until the task is ended.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_delivery(s: AppState, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let sent = deliver_due(&s).await;
            if sent > 0 {
                tracing::info!(sent, "sent queue notices");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_person_wrote_is_inert_as_code() {
        assert_eq!(as_code("@**all**").as_deref(), Some("`@**all**`"));
        assert_eq!(as_code("a`b\nc").as_deref(), Some("`abc`"));
        assert_eq!(as_code(" ` "), None);
        assert_eq!(
            as_code(&"x".repeat(500)).unwrap().chars().count(),
            MAX_SHOWN_CHARS + 2
        );
    }

    #[test]
    fn code_holds_nothing_that_reorders_or_breaks_a_line() {
        let s = "a\u{202E}b\u{2066}c\u{2028}d\u{2029}e\u{200F}f\u{061C}g";
        assert_eq!(as_code(s).as_deref(), Some("`abcdefg`"));
    }

    #[test]
    fn only_a_web_link_is_a_link() {
        assert_eq!(
            link("https://a.dev/x y").as_deref(),
            Some("https://a.dev/x%20y")
        );
        assert_eq!(link("javascript:alert(1)"), None);
        assert_eq!(link("nonsense"), None);
    }

    #[test]
    fn a_link_keeps_its_shape() {
        for u in [
            "https://a.dev/",
            "https://pr-7.a.dev:8443/x/y.html?a=1&b=2#top",
            "http://127.0.0.1:7480/queues/q/t",
            "http://[::1]:7480/queues/q/t",
            "https://pr_7.a.dev/x",
            "https://a.dev/a_b~c:d;e,f@g!h?q=a_b~c:d;e,f@g!h#a_b~c:d;e,f@g!h",
        ] {
            assert_eq!(link(u).as_deref(), Some(u));
        }
    }

    /// Zulip's mention, channel, link, image, maths and code syntax, wherever
    /// a URL can hold it.
    #[test]
    fn a_link_holds_no_markup() {
        for (bad, want) in [
            ("@**all**", "@%2A%2Aall%2A%2A"),
            ("@*g*", "@%2Ag%2A"),
            ("[x](y)", "%5Bx%5D%28y%29"),
            ("![x](y)", "!%5Bx%5D%28y%29"),
            ("#**s**", "#%2A%2As%2A%2A"),
            ("$$x$$", "%24%24x%24%24"),
            ("<x>", "%3Cx%3E"),
            ("`x`", "%60x%60"),
        ] {
            for (at, sep) in [("path", "/"), ("query", "/?q="), ("fragment", "/#")] {
                let got = link(&format!("https://a.dev{sep}{bad}")).unwrap();
                let tail = want.trim_start_matches('#');
                assert!(got.ends_with(tail), "{at}: {bad}: {got}");
            }
        }
    }

    #[test]
    fn a_host_or_user_that_cannot_be_encoded_is_no_link() {
        assert_eq!(link("https://**all**.a.dev/"), None);
        assert_eq!(link("https://user@a.dev/"), None);
        assert_eq!(link("https://u:p@a.dev/"), None);
    }

    #[test]
    fn a_retry_after_that_is_no_number_is_no_wait() {
        let a = FormAnswer {
            status: 429,
            retry_after: Some("soon".into()),
            body: vec![],
        };
        assert_eq!(judge(&a).unwrap_err().wait_secs, None);
    }
}
