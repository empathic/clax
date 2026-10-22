//! The one-time import of the Clax app's JSON state files into the jobs'
//! tables: the files the app
//! kept before its queue ran on the core's jobs, read as the app wrote
//! them, become jobs, their steps, threads and questions, and the agent's
//! read consents, in one transaction with the import's `client_imports`
//! row. A key already imported changes nothing, so the app's caller may
//! retry after a crash between the import and its deletes.
//!
//! What each file becomes:
//!
//! | File | Becomes |
//! |---|---|
//! | `pending-make.json` | a `queued` `make` job per request, but for one whose request has only the plan of an app it made (the app moves a make's plan to the app's ID once it is made) |
//! | `plans.json` | the plan of the make job whose request it is (a plan not yet under an app's ID), else a `stopped` make job holding it while it has features to build |
//! | `unfinished.json` | the carry of a `stopped` change job per change with a thread still open (a `build:<app>` entry: of that app's make job) |
//! | `change-ledger.json` | a `change` job per record, with its `publish:<base>` step when version `base + 1` is the one the record made, and its `reply` step (then `done`) when its documents are written or it has none, and each of its addresses still open got an agent reply at or after that version; else `queued`, to run on: a landed version not done has a `turn:1` step before its publish step, whose one `finish` call replies on the addresses still owed the reply, so the queue posts it (documents still to write are not written) |
//! | `asked.json` | a `change` job per entry holding its question, `waiting` while it is open, `queued` once answered or skipped; none for a withdrawn question, an answer handed over that the entry does not hold, one a ledger record that landed without its reply already answered, a question an earlier entry names, or one not of the `ask` tool |
//! | `stopped.json` | a `cancelled` change job per thread, the thread kept `stopped` |
//! | `agent-read-consent.json` | `agent_read_consents` rows |
//!
//! An entry naming an artifact, thread or question the core no longer
//! holds is left out and said so in the summary's `left_out`; a file that
//! does not parse is named in `skipped`. Job IDs are `json-` and the first
//! 24 hex digits of the SHA-256 of the file's name, the entry's index and
//! its JSON, so the same files make the same jobs.
//!
//! The rows' JSON (the queue's codec reads them): `input` is `{request,
//! base, threads: {thread ID: comment ID}}`; `plan` is `{plan_rest: false,
//! data: <the plan as plans.json kept it>}`; `carry` is `{stops,
//! can_keep_going, data}`, `data` null or, of an unfinished.json entry,
//! `{cap, spent_usd, notes, draft: {edits, before, title?,
//! capabilities?}}`, the shape the queue's worker reads a stop's carry in
//! (see `carried`).

use super::Store;
use super::jobs::JobState;
use clax_core::audit::sha256_hex;
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The key of the import of the app's JSON state.
pub const APP_JSON_V1: &str = "app-json-v1";

/// The files [`APP_JSON_V1`] reads, by the names the app keeps them under
/// in its support directory.
pub const APP_JSON_FILES: [&str; 7] = [
    "pending-make.json",
    "plans.json",
    "unfinished.json",
    "change-ledger.json",
    "asked.json",
    "stopped.json",
    "agent-read-consent.json",
];

/// What a build or a change may cost, in US dollars (`buildCapUsd`,
/// `changeCapUsd` in the app), and less than what is left of it is too
/// little for another run (`minKeepGoingUsd`): whether a stopped job may
/// keep going.
const CAP_USD: f64 = 6.0;
const MIN_KEEP_GOING_USD: f64 = 0.05;

/// What an import did, as `client_imports.summary_json` keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportSummary {
    /// The key was imported before: this call changed nothing, and the
    /// rest is that import's summary.
    #[serde(default)]
    pub already: bool,
    pub jobs: usize,
    pub consents: usize,
    /// The files that did not parse.
    pub skipped: Vec<String>,
    /// The entries left out, each `<file>: <why>`.
    pub left_out: Vec<String>,
    pub done_at: String,
}

/// A job the import writes, with its rows.
#[derive(Debug)]
struct Planned {
    id: String,
    kind: &'static str,
    artifact_id: Option<String>,
    state: JobState,
    title: Option<String>,
    input: Value,
    plan: Option<Value>,
    carry: Option<Value>,
    /// Key, result and version, in commit order.
    steps: Vec<(String, Value, Option<u32>)>,
    /// Thread, comment, stopped.
    threads: Vec<(String, String, bool)>,
    questions: Vec<String>,
}

impl Store {
    /// Imports the app's JSON state `files` (file name to its text, of
    /// [`APP_JSON_FILES`]; a file the app does not hold is absent) under
    /// `key`, for `client`, in one transaction with the `client_imports`
    /// row (see the [module docs](self)). A key imported before answers
    /// its summary with `already` and changes nothing.
    ///
    /// # Errors
    /// `invalid_args` for a key other than [`APP_JSON_V1`] or a file not
    /// in [`APP_JSON_FILES`]; database errors.
    pub fn import_client_state(
        &self,
        key: &str,
        client: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<ImportSummary> {
        if key != APP_JSON_V1 {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("no import is named {key}"),
            ));
        }
        if let Some(name) = files.keys().find(|f| !APP_JSON_FILES.contains(&f.as_str())) {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("{name} is not one of the app's state files"),
            ));
        }
        let now = self.now();
        self.with_tx(|tx| {
            let done: Option<String> = tx
                .query_row(
                    "SELECT summary_json FROM client_imports WHERE key = ?1",
                    params![key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(done) = done {
                // Written by this module: a summary it cannot read is
                // answered empty, as already imported.
                let mut s: ImportSummary = serde_json::from_str(&done).unwrap_or_default();
                s.already = true;
                return Ok(s);
            }
            let mut imp = Import {
                tx,
                now: &now,
                jobs: Vec::new(),
                consents: 0,
                skipped: Vec::new(),
                left_out: Vec::new(),
                landed: Vec::new(),
            };
            for name in APP_JSON_FILES {
                if let Some(text) = files.get(name) {
                    imp.file(name, text)?;
                }
            }
            let summary = ImportSummary {
                already: false,
                jobs: imp.jobs.len(),
                consents: imp.consents,
                skipped: imp.skipped,
                left_out: imp.left_out,
                done_at: now.clone(),
            };
            let jobs = imp.jobs;
            for j in &jobs {
                insert(tx, client, j, &now)?;
            }
            tx.execute(
                "INSERT INTO client_imports (key, done_at, summary_json) VALUES (?1, ?2, ?3)",
                params![
                    key,
                    now,
                    serde_json::to_string(&summary).expect("a summary serialises")
                ],
            )?;
            Ok(summary)
        })
    }
}

/// Writes job `j` of `client` and its rows.
fn insert(tx: &Transaction<'_>, client: &str, j: &Planned, now: &str) -> Result<()> {
    let text = |v: &Option<Value>| v.as_ref().map(Value::to_string);
    tx.execute(
        "INSERT INTO jobs (id, client, kind, artifact_id, state, step, title, input_json,
            plan_json, carry_json, rev, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?11)",
        params![
            j.id,
            client,
            j.kind,
            j.artifact_id,
            j.state,
            j.steps.last().map(|s| &s.0),
            j.title,
            j.input.to_string(),
            text(&j.plan),
            text(&j.carry),
            now
        ],
    )?;
    for (key, result, n) in &j.steps {
        tx.execute(
            "INSERT INTO job_steps (job_id, key, result_json, version_n, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![j.id, key, result.to_string(), n, now],
        )?;
    }
    for (tid, cid, stopped) in &j.threads {
        tx.execute(
            "INSERT INTO job_threads (job_id, thread_id, comment_id, stopped) VALUES (?1, ?2, ?3, ?4)",
            params![j.id, tid, cid, stopped],
        )?;
    }
    for q in &j.questions {
        let (status, answers): (String, Option<String>) = tx.query_row(
            "SELECT status, answers_json FROM questions WHERE id = ?1",
            params![q],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (outcome, answers) = match (status.as_str(), answers) {
            ("answered", Some(a)) => (Some("answered"), Some(a)),
            ("declined", _) => (Some("declined"), None),
            ("withdrawn", _) => (Some("withdrawn"), None),
            _ => (None, None),
        };
        tx.execute(
            "INSERT INTO job_questions (job_id, question_id, outcome, answers_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![j.id, q, outcome, answers],
        )?;
    }
    Ok(())
}

/// The ID of the job entry `index` of file `file`, `entry`, becomes.
fn job_id(file: &str, index: usize, entry: &Value) -> String {
    let hex = sha256_hex(format!("{file}\n{index}\n{entry}").as_bytes());
    format!("json-{}", &hex[..24])
}

/// An import in progress, in its transaction.
struct Import<'a, 't> {
    tx: &'a Transaction<'t>,
    now: &'a str,
    jobs: Vec<Planned>,
    consents: usize,
    skipped: Vec<String>,
    left_out: Vec<String>,
    /// Each ledger record whose version landed and whose reply did not:
    /// its artifact, its addresses and the comments it answered.
    /// `change-ledger.json` is read before `asked.json`.
    landed: Vec<(String, BTreeSet<String>, BTreeSet<String>)>,
}

impl Import<'_, '_> {
    fn file(&mut self, name: &str, text: &str) -> Result<()> {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            self.skipped.push(name.to_string());
            return Ok(());
        };
        let parsed = match name {
            "pending-make.json" => self.pending_make(&v),
            "plans.json" => self.plans(&v)?,
            "unfinished.json" => self.unfinished(&v)?,
            "change-ledger.json" => self.ledger(&v)?,
            "asked.json" => self.asked(&v)?,
            "stopped.json" => self.stopped(&v)?,
            "agent-read-consent.json" => self.consents(&v)?,
            _ => false,
        };
        if !parsed {
            self.skipped.push(name.to_string());
        }
        Ok(())
    }

    fn leave_out(&mut self, file: &str, why: impl std::fmt::Display) {
        self.left_out.push(format!("{file}: {why}"));
    }

    /// The live, visible artifact `id`'s title and current version.
    fn artifact(&self, id: &str) -> Result<Option<(String, u32)>> {
        Ok(self
            .tx
            .query_row(
                "SELECT title, current_version FROM artifacts
                 WHERE id = ?1 AND deleted_at IS NULL AND hidden = 0",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Thread `tid`'s artifact, when the thread is there.
    fn thread_artifact(&self, tid: &str) -> Result<Option<String>> {
        Ok(self
            .tx
            .query_row(
                "SELECT artifact_id FROM threads WHERE id = ?1",
                params![tid],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Whether thread `tid` is there, on artifact `aid`, and open.
    fn open_thread(&self, aid: &str, tid: &str) -> Result<bool> {
        Ok(self.tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM threads
                 WHERE id = ?1 AND artifact_id = ?2 AND status = 'open')",
            params![tid, aid],
            |r| r.get(0),
        )?)
    }

    /// The newest of `comments` on thread `tid`, else the thread's newest
    /// viewer comment, else `""`.
    fn newest_comment(&self, tid: &str, comments: &[String]) -> Result<String> {
        let mut best: Option<(String, String)> = None;
        for c in comments {
            let at: Option<String> = self
                .tx
                .query_row(
                    "SELECT created_at FROM comments WHERE id = ?1 AND thread_id = ?2",
                    params![c, tid],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(at) = at
                && best.as_ref().is_none_or(|(b, _)| at >= *b)
            {
                best = Some((at, c.clone()));
            }
        }
        if let Some((_, c)) = best {
            return Ok(c);
        }
        Ok(self
            .tx
            .query_row(
                "SELECT id FROM comments WHERE thread_id = ?1 AND author_kind = 'viewer'
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                params![tid],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }

    /// The body of comment `id`, or `""`.
    fn comment_body(&self, id: &str) -> Result<String> {
        Ok(self
            .tx
            .query_row(
                "SELECT body FROM comments WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }

    /// A change job's rows for `threads` (each with the comments it
    /// answered) on artifact `aid`: the threads of `aid` that are there,
    /// each with its newest answered comment, and the request (the newest
    /// such comment's words).
    fn change_threads(
        &self,
        aid: &str,
        threads: &[(String, Vec<String>)],
    ) -> Result<(BTreeMap<String, String>, String)> {
        let mut out = BTreeMap::new();
        let mut newest: Option<(String, String)> = None;
        for (tid, comments) in threads {
            if self.thread_artifact(tid)?.as_deref() != Some(aid) {
                continue;
            }
            let c = self.newest_comment(tid, comments)?;
            if !c.is_empty() {
                let at: String = self.tx.query_row(
                    "SELECT created_at FROM comments WHERE id = ?1",
                    params![c],
                    |r| r.get(0),
                )?;
                if newest.as_ref().is_none_or(|(b, _)| at >= *b) {
                    newest = Some((at, c.clone()));
                }
            }
            out.insert(tid.clone(), c);
        }
        let request = match newest {
            Some((_, c)) => self.comment_body(&c)?,
            None => String::new(),
        };
        Ok((out, request))
    }

    fn pending_make(&mut self, v: &Value) -> bool {
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let requests = match (v.get("requests"), v.get("request")) {
            (Some(rs), _) if rs.is_array() => strings(rs),
            (_, Some(Value::String(r))) => vec![r.clone()],
            _ => return false,
        };
        for (i, r) in requests.iter().enumerate() {
            let r = r.trim();
            if r.is_empty() {
                continue;
            }
            self.jobs.push(Planned {
                id: job_id("pending-make.json", i, &json!(r)),
                kind: "make",
                artifact_id: None,
                state: JobState::Queued,
                title: None,
                input: json!({"request": r, "base": 0, "threads": {}}),
                plan: None,
                carry: None,
                steps: Vec::new(),
                threads: Vec::new(),
                questions: Vec::new(),
            });
        }
        true
    }

    fn plans(&mut self, v: &Value) -> Result<bool> {
        let Some(plans) = v.get("plans").and_then(Value::as_object) else {
            return Ok(false);
        };
        // The requests whose make made its app.
        let mut made: BTreeSet<String> = BTreeSet::new();
        for (i, (key, plan)) in plans.iter().enumerate() {
            let Some(request) = plan.get("request").and_then(Value::as_str) else {
                self.leave_out("plans.json", format!("the plan of {key} names no request"));
                continue;
            };
            let request = request.trim();
            let todo = plan
                .get("features")
                .and_then(Value::as_array)
                .is_some_and(|fs| {
                    fs.iter().any(|f| {
                        matches!(
                            f.get("status").and_then(Value::as_str).unwrap_or("todo"),
                            "todo" | "doing"
                        )
                    })
                });
            let held = json!({"plan_rest": false, "data": plan});
            let app = self.artifact(key)?;
            if app.is_some() {
                // The app moves a make's plan to the app's ID once the make
                // made it (`AgentLoop.newApp`): that make is over.
                made.insert(request.to_string());
            } else if let Some(j) = self.jobs.iter_mut().find(|j| {
                j.kind == "make" && j.plan.is_none() && j.input["request"].as_str() == Some(request)
            }) {
                // The pending make of the same request, not yet given a plan.
                j.plan = Some(held);
                continue;
            }
            if !todo {
                continue;
            }
            let (artifact_id, title, base) = match app {
                Some((title, n)) => (Some(key.clone()), Some(title), n),
                None => (None, None, 0),
            };
            let spent = plan.get("spentUsd").and_then(Value::as_f64).unwrap_or(0.0);
            self.jobs.push(Planned {
                id: job_id("plans.json", i, &json!([key, plan])),
                kind: "make",
                artifact_id,
                state: JobState::Stopped,
                title,
                input: json!({"request": request, "base": base, "threads": {}}),
                plan: Some(held),
                carry: Some(carry(1, spent, Value::Null)),
                steps: Vec::new(),
                threads: Vec::new(),
                questions: Vec::new(),
            });
        }
        // A pending make whose request has only its made app's plan ended
        // after it made the app, before the app forgot it: it is not made
        // again.
        let mut over = Vec::new();
        self.jobs.retain(|j| {
            let request = j.input["request"].as_str().unwrap_or_default();
            let ended = j.kind == "make"
                && j.artifact_id.is_none()
                && j.plan.is_none()
                && made.contains(request);
            if ended {
                over.push(request.to_string());
            }
            !ended
        });
        for request in over {
            self.leave_out(
                "pending-make.json",
                format!("the make of {request:?} made its app"),
            );
        }
        Ok(true)
    }

    fn unfinished(&mut self, v: &Value) -> Result<bool> {
        let Some(threads) = v.get("threads").and_then(Value::as_object) else {
            return Ok(false);
        };
        // A change answering several threads is kept under each: one job.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (i, (key, entry)) in threads.iter().enumerate() {
            let Some(UnfinishedEntry {
                artifact: aid,
                threads: tids,
                comments,
                spent_usd: spent,
                stops,
            }) = UnfinishedEntry::of(entry)
            else {
                self.leave_out(
                    "unfinished.json",
                    format!("the entry of {key} does not parse"),
                );
                continue;
            };
            if !seen.insert(entry.to_string()) {
                continue;
            }
            let held = carry(stops, spent, carried(entry, spent));
            if let Some(app) = key.strip_prefix("build:") {
                // A build's draft: its make job carries it.
                if let Some(j) = self
                    .jobs
                    .iter_mut()
                    .find(|j| j.kind == "make" && j.artifact_id.as_deref() == Some(app))
                {
                    j.carry = Some(held);
                    continue;
                }
                let Some((title, n)) = self.artifact(app)? else {
                    self.leave_out("unfinished.json", format!("the build of {app} is gone"));
                    continue;
                };
                self.jobs.push(Planned {
                    id: job_id("unfinished.json", i, &json!([key, entry])),
                    kind: "make",
                    artifact_id: Some(app.to_string()),
                    state: JobState::Stopped,
                    title: Some(title),
                    input: json!({"request": "", "base": n, "threads": {}}),
                    plan: None,
                    carry: Some(held),
                    steps: Vec::new(),
                    threads: Vec::new(),
                    questions: Vec::new(),
                });
                continue;
            }
            let Some((title, n)) = self.artifact(&aid)? else {
                self.leave_out(
                    "unfinished.json",
                    format!("the change of {aid} lost its app"),
                );
                continue;
            };
            // The app forgets a change none of whose threads is open
            // (`AgentWorker._pruneUnfinished`): it is not kept going.
            let mut open = false;
            for t in &tids {
                open |= self.open_thread(&aid, t)?;
            }
            if !open {
                self.leave_out(
                    "unfinished.json",
                    format!("the change of {key}: its threads are closed or gone"),
                );
                continue;
            }
            let asked: Vec<(String, Vec<String>)> =
                tids.iter().map(|t| (t.clone(), comments.clone())).collect();
            let (threads, request) = self.change_threads(&aid, &asked)?;
            self.jobs.push(Planned {
                id: job_id("unfinished.json", i, &json!([key, entry])),
                kind: "change",
                artifact_id: Some(aid),
                state: JobState::Stopped,
                title: Some(title),
                input: json!({"request": request, "base": n, "threads": threads}),
                plan: None,
                carry: Some(held),
                steps: Vec::new(),
                threads: threads.into_iter().map(|(t, c)| (t, c, false)).collect(),
                questions: Vec::new(),
            });
        }
        Ok(true)
    }

    fn ledger(&mut self, v: &Value) -> Result<bool> {
        let Some(records) = v.get("records").and_then(Value::as_array) else {
            return Ok(false);
        };
        for (i, r) in records.iter().enumerate() {
            let Some(rec) = LedgerRecord::of(r) else {
                self.leave_out("change-ledger.json", format!("record {i} does not parse"));
                continue;
            };
            let Some((title, _)) = self.artifact(&rec.artifact_id)? else {
                self.leave_out(
                    "change-ledger.json",
                    format!("the change of {} lost its app", rec.artifact_id),
                );
                continue;
            };
            let answered: Vec<(String, Vec<String>)> = rec.answered.clone().into_iter().collect();
            let (threads, request) = self.change_threads(&rec.artifact_id, &answered)?;
            let mut steps = Vec::new();
            let n = rec.base + 1;
            let made = self.made_by(&rec, n)?;
            let mut done = false;
            if let Some(at) = &made {
                let owed = self.owed(&rec, at)?;
                let replied = owed.is_empty();
                if !replied {
                    self.landed.push((
                        rec.artifact_id.clone(),
                        rec.addresses.iter().cloned().collect(),
                        rec.answered.values().flatten().cloned().collect(),
                    ));
                }
                // Its documents are done when written, or when it had none
                // to write (the app's `_settleDocs`).
                let docs_done = rec.docs_done || rec.docs == 0;
                done = docs_done && replied;
                if !done {
                    // The turn whose `finish` made the version: the queue
                    // reads a publish step as its finish call's first half,
                    // and answers the call with the reply it plans.
                    steps.push(("turn:1".to_string(), finish_turn(&rec, &owed), None));
                }
                steps.push((
                    format!("publish:{}", rec.base),
                    json!({"artifact_id": rec.artifact_id, "version": n, "docs": rec.docs}),
                    Some(n),
                ));
                if done {
                    steps.push((
                        "reply".to_string(),
                        json!({"comments": [], "imported": true}),
                        None,
                    ));
                }
            }
            let state = if done {
                JobState::Done
            } else {
                JobState::Queued
            };
            self.jobs.push(Planned {
                id: job_id("change-ledger.json", i, r),
                kind: "change",
                artifact_id: Some(rec.artifact_id.clone()),
                state,
                title: Some(title),
                input: json!({"request": request, "base": rec.base, "threads": threads}),
                plan: None,
                carry: None,
                steps,
                threads: threads.into_iter().map(|(t, c)| (t, c, false)).collect(),
                questions: Vec::new(),
            });
        }
        Ok(true)
    }

    /// When version `n` of the record's artifact is the one the record made
    /// (the app's `madeBy`: published from an app session, naming exactly
    /// the record's addresses, its note starting with the reply's first
    /// 40 characters), the time it was made.
    fn made_by(&self, rec: &LedgerRecord, n: u32) -> Result<Option<String>> {
        let v: Option<(String, Option<String>, Option<String>)> = self
            .tx
            .query_row(
                "SELECT v.created_at, v.note, s.harness FROM versions v
                 LEFT JOIN sessions s ON s.id = v.session_id
                 WHERE v.artifact_id = ?1 AND v.n = ?2",
                params![rec.artifact_id, n],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((at, note, harness)) = v else {
            return Ok(None);
        };
        if harness.as_deref() != Some("app") {
            return Ok(None);
        }
        let mut stmt = self.tx.prepare(
            "SELECT thread_id FROM version_threads WHERE artifact_id = ?1 AND version_n = ?2",
        )?;
        let named: BTreeSet<String> = stmt
            .query_map(params![rec.artifact_id, n], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let want: BTreeSet<String> = rec.addresses.iter().cloned().collect();
        if named != want || named.len() != rec.addresses.len() {
            return Ok(None);
        }
        let clean = |s: &str| {
            clax_core::working::clean_line(s, usize::MAX)
                .0
                .unwrap_or_default()
        };
        let head: String = clean(&rec.reply).chars().take(40).collect();
        if !clean(note.as_deref().unwrap_or_default()).starts_with(&head) {
            return Ok(None);
        }
        Ok(Some(at))
    }

    /// The threads still owed the record's reply: each of its addresses
    /// still open on its artifact with no agent comment at or after `at`.
    /// The app's recovery replies only on open threads
    /// (`AgentLoop.settle`), so a record whose addresses are all resolved
    /// or gone, or which addresses none, owes none: it replied.
    fn owed(&self, rec: &LedgerRecord, at: &str) -> Result<Vec<String>> {
        let mut owed = Vec::new();
        for tid in &rec.addresses {
            if !self.open_thread(&rec.artifact_id, tid)? {
                continue;
            }
            let found: bool = self.tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM comments
                     WHERE thread_id = ?1 AND author_kind = 'agent' AND created_at >= ?2)",
                params![tid, at],
                |r| r.get(0),
            )?;
            if !found {
                owed.push(tid.clone());
            }
        }
        Ok(owed)
    }

    fn asked(&mut self, v: &Value) -> Result<bool> {
        let Some(entries) = v.get("entries").and_then(Value::as_array) else {
            return Ok(false);
        };
        for (i, e) in entries.iter().enumerate() {
            let strings = |k: &str| -> Option<Vec<String>> {
                e.get(k)?
                    .as_array()?
                    .iter()
                    .map(|s| s.as_str().map(str::to_string))
                    .collect()
            };
            let (Some(q), Some(aid), Some(tids), Some(comments)) = (
                e.get("question_id").and_then(Value::as_str),
                e.get("artifact_id").and_then(Value::as_str),
                strings("thread_ids"),
                strings("comment_ids"),
            ) else {
                self.leave_out("asked.json", format!("entry {i} does not parse"));
                continue;
            };
            if self.jobs.iter().any(|j| j.questions.iter().any(|x| x == q)) {
                self.leave_out("asked.json", format!("the question {q} is another entry's"));
                continue;
            }
            let Some((title, n)) = self.artifact(aid)? else {
                self.leave_out("asked.json", format!("the question {q} lost its app"));
                continue;
            };
            // Only an `ask` question, which no job holds, is a job's.
            let status: Option<(String, bool)> = self
                .tx
                .query_row(
                    "SELECT status, taken_at IS NOT NULL FROM questions q WHERE id = ?1
                         AND q.source = 'ask'
                         AND NOT EXISTS (SELECT 1 FROM job_questions WHERE question_id = q.id)",
                    params![q],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((status, taken)) = status else {
                self.leave_out(
                    "asked.json",
                    format!("the question {q} is gone, not the ask tool's, or a job's"),
                );
                continue;
            };
            // What the app never runs again: a withdrawn question's change
            // (called off, or taken over by a later run); an answer handed
            // over that the entry does not hold (the run that asked took
            // it); and an answer whose run landed and did not reply, which
            // the ledger's change job finishes (`AgentLoop.settleAnswered`).
            let closed = !matches!(status.as_str(), "open" | "released");
            let why = if status == "withdrawn" {
                Some("was withdrawn")
            } else if closed && taken && e.get("answer").and_then(Value::as_str).is_none() {
                Some("was answered to the run that asked it")
            } else if closed
                && !comments.is_empty()
                && self.landed.iter().any(|(a, addresses, done)| {
                    a == aid
                        && tids.iter().any(|t| addresses.contains(t))
                        && comments.iter().all(|c| done.contains(c))
                })
            {
                Some("was answered to a change that landed")
            } else {
                None
            };
            if let Some(why) = why {
                self.leave_out("asked.json", format!("the question {q} {why}"));
                continue;
            }
            let asked: Vec<(String, Vec<String>)> =
                tids.iter().map(|t| (t.clone(), comments.clone())).collect();
            let (threads, request) = self.change_threads(aid, &asked)?;
            let state = if matches!(status.as_str(), "open" | "released") {
                JobState::Waiting
            } else {
                JobState::Queued
            };
            self.jobs.push(Planned {
                id: job_id("asked.json", i, e),
                kind: "change",
                artifact_id: Some(aid.to_string()),
                state,
                title: Some(title),
                input: json!({"request": request, "base": n, "threads": threads}),
                plan: None,
                carry: None,
                steps: Vec::new(),
                threads: threads.into_iter().map(|(t, c)| (t, c, false)).collect(),
                questions: vec![q.to_string()],
            });
        }
        Ok(true)
    }

    fn stopped(&mut self, v: &Value) -> Result<bool> {
        let Some(stops) = v.get("stops").and_then(Value::as_object) else {
            return Ok(false);
        };
        for (i, (tid, cid)) in stops.iter().enumerate() {
            let Some(cid) = cid.as_str() else {
                self.leave_out("stopped.json", format!("the stop of {tid} does not parse"));
                continue;
            };
            let Some(aid) = self.thread_artifact(tid)? else {
                self.leave_out("stopped.json", format!("the thread {tid} is gone"));
                continue;
            };
            let Some((title, n)) = self.artifact(&aid)? else {
                self.leave_out("stopped.json", format!("the thread {tid} lost its app"));
                continue;
            };
            self.jobs.push(Planned {
                id: job_id("stopped.json", i, &json!([tid, cid])),
                kind: "change",
                artifact_id: Some(aid),
                state: JobState::Cancelled,
                title: Some(title),
                input: json!({
                    "request": self.comment_body(cid)?,
                    "base": n,
                    "threads": BTreeMap::from([(tid.clone(), cid.to_string())]),
                }),
                plan: None,
                carry: None,
                steps: Vec::new(),
                threads: vec![(tid.clone(), cid.to_string(), true)],
                questions: Vec::new(),
            });
        }
        Ok(true)
    }

    fn consents(&mut self, v: &Value) -> Result<bool> {
        let Some(always) = v.get("always").and_then(Value::as_array) else {
            return Ok(false);
        };
        for pair in always {
            let (Some(r), Some(s)) = (
                pair.get(0).and_then(Value::as_str),
                pair.get(1).and_then(Value::as_str),
            ) else {
                self.leave_out("agent-read-consent.json", format!("{pair} is no pair"));
                continue;
            };
            if self.artifact(r)?.is_none() || self.artifact(s)?.is_none() {
                self.leave_out(
                    "agent-read-consent.json",
                    format!("{r} reading {s}: an app is gone"),
                );
                continue;
            }
            self.consents += self.tx.execute(
                "INSERT OR IGNORE INTO agent_read_consents (reader, source, created_at)
                 VALUES (?1, ?2, ?3)",
                params![r, s, self.now],
            )?;
        }
        Ok(true)
    }
}

/// A stopped job's carry: how many times it stopped, whether what it spent
/// leaves room for another run, and what it carries.
fn carry(stops: u64, spent: f64, data: Value) -> Value {
    json!({
        "stops": stops,
        "can_keep_going": CAP_USD - spent >= MIN_KEEP_GOING_USD,
        "data": data,
    })
}

/// Step `turn:1` of a ledger record's change whose version landed, as the
/// queue's runner writes a turn's notes: one `finish` call (ID
/// `imported`) planned on the record's base, whose reply posts the
/// record's reply on the threads still `owed` it and acknowledges the
/// comments it answered there. The queue reads the job's
/// `publish:<base>` step as the call's publish, and posts the reply.
fn finish_turn(rec: &LedgerRecord, owed: &[String]) -> Value {
    let ack: Vec<&String> = owed
        .iter()
        .filter_map(|t| rec.answered.get(t))
        .flatten()
        .collect();
    json!({
        "base": rec.base,
        "notes": {
            "calls": [{
                "id": "imported",
                "tool": "finish",
                "publish": {"base": rec.base, "data": null},
                "reply": {"threads": owed, "body": rec.reply, "resolve": rec.resolve, "ack": ack},
            }],
            "data": null,
        },
    })
}

/// The carry's `data` of `unfinished.json` entry `e`, in the shape the
/// queue's worker reads a stop's (`AgentTurns`' cap stop): the cap it
/// stopped at (`cap`), what its runs spent (`spent_usd`) and the agent's
/// notes, with its unfinished draft (`draft`): `edits`, path to the
/// draft's text or null for a file it deleted, over `before`, the
/// fingerprint of each such path as the app held it then, and the
/// draft's `title` and `capabilities` when it changed them. The worker
/// starts a Keep going run from the draft when the app still holds every
/// `before`.
fn carried(e: &Value, spent: f64) -> Value {
    let field = |k: &str| e.get(k).cloned().unwrap_or(Value::Null);
    let mut draft = json!({
        "edits": e.get("edits").cloned().unwrap_or_else(|| json!({})),
        "before": e.get("before").cloned().unwrap_or_else(|| json!({})),
    });
    for k in ["title", "capabilities"] {
        if !field(k).is_null() {
            draft[k] = field(k);
        }
    }
    json!({
        "cap": field("limit"),
        "spent_usd": spent,
        "notes": e.get("notes").cloned().unwrap_or_else(|| json!([])),
        "draft": draft,
    })
}

/// What the import reads of an `unfinished.json` entry
/// (`UnfinishedChange.fromJson`'s required fields).
struct UnfinishedEntry {
    artifact: String,
    threads: Vec<String>,
    comments: Vec<String>,
    spent_usd: f64,
    stops: u64,
}

impl UnfinishedEntry {
    fn of(e: &Value) -> Option<UnfinishedEntry> {
        let strings = |k: &str| -> Option<Vec<String>> {
            e.get(k)?
                .as_array()?
                .iter()
                .map(|s| s.as_str().map(str::to_string))
                .collect()
        };
        e.get("limit")?.as_str()?;
        e.get("at")?.as_str()?;
        Some(UnfinishedEntry {
            artifact: e.get("artifact")?.as_str()?.to_string(),
            threads: strings("threads")?,
            comments: strings("comments")?,
            spent_usd: e.get("spent_usd")?.as_f64()?,
            stops: e.get("stops").and_then(Value::as_u64).unwrap_or(1),
        })
    }
}

/// A `change-ledger.json` record (`ChangeRecord.fromJson`), the fields the
/// import reads.
#[derive(Clone, Debug)]
struct LedgerRecord {
    artifact_id: String,
    base: u32,
    addresses: Vec<String>,
    answered: BTreeMap<String, Vec<String>>,
    reply: String,
    resolve: bool,
    docs: usize,
    docs_done: bool,
}

impl LedgerRecord {
    fn of(r: &Value) -> Option<LedgerRecord> {
        let strings = |v: &Value| -> Option<Vec<String>> {
            v.as_array()?
                .iter()
                .map(|s| s.as_str().map(str::to_string))
                .collect()
        };
        Some(LedgerRecord {
            artifact_id: r.get("artifact_id")?.as_str()?.to_string(),
            base: u32::try_from(r.get("base")?.as_u64()?).ok()?,
            addresses: strings(r.get("addresses")?)?,
            answered: r
                .get("answered")?
                .as_object()?
                .iter()
                .map(|(k, v)| Some((k.clone(), strings(v)?)))
                .collect::<Option<_>>()?,
            reply: r.get("reply")?.as_str()?.to_string(),
            resolve: r.get("resolve").and_then(Value::as_bool).unwrap_or(false),
            docs: r.get("db")?.as_array()?.len(),
            docs_done: r.get("docs_done")?.as_bool()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::{Close, NewQuestion, Source};
    use crate::test_util::{DAEMON, artifact, session, store};
    use crate::threads::{NewComment, NewThread};
    use clax_core::ArtifactId;
    use clax_core::publish::validate;

    /// A thread on `aid` opened with `body`: its ID and its first comment's.
    fn thread_on(st: &Store, aid: &ArtifactId, body: &str) -> (String, String) {
        let t = st
            .create_thread(
                DAEMON,
                aid,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: crate::test_util::anchor(),
                    body: body.into(),
                    author_name: "Alex".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        let c: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT id FROM comments WHERE thread_id = ?1",
                    params![t.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        (t.id, c)
    }

    fn files(pairs: &[(&str, Value)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Every job of the app, open or not.
    fn jobs(st: &Store) -> Vec<crate::jobs::JobFull> {
        let ids: Vec<String> = st
            .with_read(|c| {
                let mut s = c.prepare("SELECT id FROM jobs ORDER BY id")?;
                Ok(s.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .unwrap();
        ids.iter().map(|id| st.job(id).unwrap().unwrap()).collect()
    }

    /// Each of the seven files, as the app writes them, becomes its rows
    /// (see the module docs' table), and a second import changes nothing.
    #[test]
    fn each_file_becomes_its_rows_once() {
        let (_d, st) = store();
        let app = session(&st, "app", "h1");
        let aid = artifact(&st, None);
        let other = artifact(&st, None);
        let (t1, c1) = thread_on(&st, &aid, "make it green");
        let (t2, c2) = thread_on(&st, &aid, "bigger title");
        // Version 2: the change the first ledger record made, and its reply.
        let reply = "Made it green.";
        let p = validate(
            serde_json::from_value(json!({
                "if_version": 1, "note": reply, "addresses": [t1],
                "files": {"index.html": {"content": "<p>green", "encoding": "utf8"}},
            }))
            .unwrap(),
        )
        .unwrap();
        st.publish_version(DAEMON, &aid, p, Some(&app)).unwrap();
        st.add_agent_reply(
            DAEMON,
            &t1,
            NewComment {
                author_kind: "agent",
                author_name: "Clax".into(),
                author_public_id: None,
                via_session_id: Some(app.clone()),
                body: reply.into(),
                via_page: false,
            },
            None,
        )
        .unwrap();
        let qs = serde_json::from_value(json!([{"question": "Which green?", "header": "Green",
            "options": [{"label": "Mint"}, {"label": "Forest"}]}]))
        .unwrap();
        let q = st
            .create_question(
                DAEMON,
                NewQuestion {
                    session_id: app.clone(),
                    artifact_id: Some(aid.as_str().into()),
                    source: Source::Ask,
                    tool_use_id: None,
                    questions: qs,
                    released: false,
                    thread_id: None,
                    in_terminal: false,
                },
            )
            .unwrap()
            .0
            .id;
        // The shapes ChangeRecord.toJson, Plan.toJson and
        // UnfinishedChange.toJson write.
        let record = |base: u32, t: &str, c: &str, docs_done: bool| {
            json!({
                "artifact_id": aid.as_str(), "base": base, "addresses": [t],
                "answered": {t: [c]}, "reply": reply, "resolve": true,
                "db": [{"op": "set", "collection": "notes", "doc_id": "n1", "data": {"t": 1}}],
                "pins": {"notes/n1": null}, "docs_done": docs_done,
            })
        };
        let plan = |request: &str, status: &str| {
            json!({
                "request": request,
                "features": [{"id": "f1", "title": "Totals", "detail": "", "status": status, "by": "agent"}],
                "updatedAt": "2026-10-18T10:00:00.000Z", "revision": 2, "spentUsd": 1.5, "stopped": false,
            })
        };
        let unfinished = |threads: Vec<&str>, comments: Vec<&str>| {
            json!({
                "artifact": aid.as_str(), "threads": threads, "comments": comments,
                "limit": "turns", "spent_usd": 5.98, "at": "2026-10-18T10:00:00.000Z",
                "stops": 2, "notes": ["Ran out of turns"], "edits": {"index.html": "<p>half"},
                "before": {"index.html": "5:abc"},
            })
        };
        let build_key = format!("build:{}", aid.as_str());
        let state = files(&[
            (
                "pending-make.json",
                json!({"requests": ["a budget", "a habit tracker"], "waiting": ["a habit tracker"]}),
            ),
            (
                "plans.json",
                json!({"plans": {
                    "provisional-1": plan("a budget", "todo"),
                    aid.as_str(): plan("a tracker", "doing"),
                    other.as_str(): plan("done already", "done"),
                }}),
            ),
            (
                "unfinished.json",
                json!({"version": 1, "threads": {
                    t2.as_str(): unfinished(vec![&t2], vec![&c2]),
                    build_key.as_str(): unfinished(vec![], vec![]),
                }}),
            ),
            (
                "change-ledger.json",
                json!({"records": [record(1, &t1, &c1, true), record(5, &t2, &c2, false), {
                    "artifact_id": "zzzzzzzzzzzz", "base": 1, "addresses": [], "answered": {},
                    "reply": "x", "resolve": false, "db": [], "pins": {}, "docs_done": false,
                }]}),
            ),
            (
                "asked.json",
                json!({"version": 1, "entries": [{
                    "question_id": q, "artifact_id": aid.as_str(),
                    "thread_ids": [t2], "comment_ids": [c2],
                }]}),
            ),
            (
                "stopped.json",
                json!({"version": 1, "stops": {t2.as_str(): c2}}),
            ),
            (
                "agent-read-consent.json",
                json!({"always": [[aid.as_str(), other.as_str()], [aid.as_str(), "zzzzzzzzzzzz"]]}),
            ),
        ]);
        let s = st.import_client_state(APP_JSON_V1, "app", &state).unwrap();
        assert!(!s.already);
        assert_eq!(s.skipped, Vec::<String>::new());
        assert_eq!(s.consents, 1);
        assert_eq!(s.left_out.len(), 2, "{:?}", s.left_out);
        assert_eq!(st.agent_read_consents().unwrap().len(), 1);
        let all = jobs(&st);
        assert_eq!(s.jobs, all.len());
        let by = |f: &dyn Fn(&crate::jobs::JobFull) -> bool| -> Vec<&crate::jobs::JobFull> {
            all.iter().filter(|j| f(j)).collect()
        };
        // The pending makes, queued; the budget's carries its plan.
        let makes = by(&|j| j.job.kind == "make" && j.job.state == JobState::Queued);
        assert_eq!(makes.len(), 2);
        let budget = makes
            .iter()
            .find(|j| j.job.input["request"] == "a budget")
            .unwrap();
        assert_eq!(
            budget.job.plan.as_ref().unwrap()["data"]["request"],
            "a budget"
        );
        // The app's plan still to build: a stopped make holding it, and the
        // build's draft as its carry.
        let build = by(&|j| j.job.kind == "make" && j.job.state == JobState::Stopped);
        assert_eq!(build.len(), 1, "a finished plan makes no job");
        assert_eq!(build[0].job.artifact_id.as_deref(), Some(aid.as_str()));
        let carry = build[0].job.carry.as_ref().unwrap();
        assert_eq!(
            (carry["stops"].as_u64(), carry["can_keep_going"].as_bool()),
            (Some(2), Some(false))
        );
        // The ledger: the change that landed and replied is done with both
        // steps; the one with no version runs again.
        let done = by(&|j| j.job.state == JobState::Done);
        assert_eq!(done.len(), 1);
        let keys: Vec<&str> = done[0].steps.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["publish:1", "reply"]);
        assert_eq!(done[0].steps[0].version_n, Some(2));
        assert_eq!(done[0].threads[0].comment_id, c1);
        assert_eq!(done[0].job.input["request"], "make it green");
        let again = by(&|j| j.job.kind == "change" && j.job.state == JobState::Queued);
        assert_eq!(again.len(), 1);
        assert!(again[0].steps.is_empty());
        assert_eq!(again[0].job.input["base"], 5);
        // The unfinished change, stopped with its carry.
        let stopped = by(&|j| j.job.kind == "change" && j.job.state == JobState::Stopped);
        assert_eq!(stopped.len(), 1);
        // Its carry in the shape the worker's Keep going reads: the cap it
        // stopped at, its spend and notes, and its draft's edits over what
        // the app held.
        assert_eq!(
            stopped[0].job.carry.as_ref().unwrap()["data"],
            json!({
                "cap": "turns", "spent_usd": 5.98, "notes": ["Ran out of turns"],
                "draft": {"edits": {"index.html": "<p>half"}, "before": {"index.html": "5:abc"}},
            })
        );
        assert_eq!(stopped[0].threads[0].thread_id, t2);
        // The open question: its job waits on it.
        let waiting = by(&|j| j.job.state == JobState::Waiting);
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].questions[0].question_id, q);
        // The stopped thread.
        let cancelled = by(&|j| j.job.state == JobState::Cancelled);
        assert_eq!(cancelled.len(), 1);
        assert!(cancelled[0].threads[0].stopped);
        assert_eq!(cancelled[0].threads[0].comment_id, c2);

        // Once: the same files again change nothing.
        let s2 = st.import_client_state(APP_JSON_V1, "app", &state).unwrap();
        assert!(s2.already);
        assert_eq!(s2.jobs, s.jobs);
        assert_eq!(jobs(&st).len(), all.len());
    }

    /// Publishes the next version of `aid` from session `app`, noted
    /// `reply` and addressing `threads`, as a change's publish does.
    fn publish_change(st: &Store, app: &str, aid: &ArtifactId, reply: &str, threads: &[&str]) {
        let n = st.get_artifact(aid).unwrap().unwrap().current_version;
        let p = validate(
            serde_json::from_value(json!({
                "if_version": n, "note": reply, "addresses": threads,
                "files": {"index.html": {"content": format!("<p>{reply}"), "encoding": "utf8"}},
            }))
            .unwrap(),
        )
        .unwrap();
        st.publish_version(DAEMON, aid, p, Some(app)).unwrap();
    }

    /// Posts `body` on thread `tid` as session `app`'s agent.
    fn agent_reply(st: &Store, app: &str, tid: &str, body: &str) {
        st.add_agent_reply(
            DAEMON,
            tid,
            NewComment {
                author_kind: "agent",
                author_name: "Clax".into(),
                author_public_id: None,
                via_session_id: Some(app.to_string()),
                body: body.into(),
                via_page: false,
            },
            None,
        )
        .unwrap();
    }

    /// A `ChangeRecord.toJson` record of a change on `aid` from `base`
    /// answering comment `c` of thread `t`, with one document write when
    /// `db`.
    fn change_record(
        aid: &ArtifactId,
        base: u32,
        (t, c): (&str, &str),
        reply: &str,
        db: bool,
        docs_done: bool,
    ) -> Value {
        let ops = if db {
            json!([{"op": "set", "collection": "notes", "doc_id": "n1", "data": {"t": 1}}])
        } else {
            json!([])
        };
        let pins = if db {
            json!({"notes/n1": null})
        } else {
            json!({})
        };
        json!({
            "artifact_id": aid.as_str(), "base": base, "addresses": [t],
            "answered": {t: [c]}, "reply": reply, "resolve": false,
            "db": ops, "pins": pins, "docs_done": docs_done,
        })
    }

    /// Session `app`'s open `ask` question about `aid`.
    fn ask(st: &Store, app: &str, aid: &ArtifactId) -> String {
        ask_from(st, app, aid, Source::Ask)
    }

    /// A question of `source` on `aid` from session `app`; its ID.
    fn ask_from(st: &Store, app: &str, aid: &ArtifactId, source: Source) -> String {
        let qs = serde_json::from_value(json!([{"question": "Which green?", "header": "Green",
            "options": [{"label": "Mint"}, {"label": "Forest"}]}]))
        .unwrap();
        st.create_question(
            DAEMON,
            NewQuestion {
                session_id: app.to_string(),
                artifact_id: Some(aid.as_str().into()),
                source,
                tool_use_id: None,
                questions: qs,
                released: false,
                thread_id: None,
                in_terminal: false,
            },
        )
        .unwrap()
        .0
        .id
    }

    /// The import's change jobs on `aid`, by the base of their input.
    fn changes_by_base(st: &Store) -> BTreeMap<u64, crate::jobs::JobFull> {
        jobs(st)
            .into_iter()
            .filter(|j| j.job.kind == "change")
            .map(|j| (j.job.input["base"].as_u64().unwrap(), j))
            .collect()
    }

    /// A ledger record is a finished change when its version landed, its
    /// documents are written or it has none, and no open thread it
    /// addresses still waits for its reply: `done`, never run again. One
    /// with documents still to write, or a reply still owed, keeps its
    /// publish step and runs on; one whose version never landed runs again.
    #[test]
    fn a_ledger_change_with_nothing_left_to_apply_is_done() {
        let (_d, st) = store();
        let app = session(&st, "app", "h1");
        let aid = artifact(&st, None);
        let (t1, c1) = thread_on(&st, &aid, "make it green");
        let (t2, c2) = thread_on(&st, &aid, "keep my notes");
        let (t3, c3) = thread_on(&st, &aid, "bigger title");
        let (t4, c4) = thread_on(&st, &aid, "a footer");
        let (t5, c5) = thread_on(&st, &aid, "a header");
        // Version 2: a change that wrote no documents, and replied.
        publish_change(&st, &app, &aid, "Made it green.", &[&t1]);
        agent_reply(&st, &app, &t1, "Made it green.");
        // Version 3: a change with a document write never written, replied.
        publish_change(&st, &app, &aid, "Kept your notes.", &[&t2]);
        agent_reply(&st, &app, &t2, "Kept your notes.");
        // Version 4: no documents, no reply, its thread resolved since.
        publish_change(&st, &app, &aid, "Added a footer.", &[&t4]);
        st.resolve_thread(DAEMON, &t4, "owner").unwrap();
        // Version 5: no documents, its reply still owed on an open thread.
        publish_change(&st, &app, &aid, "Added a header.", &[&t5]);
        let ledger = json!({"records": [
            change_record(&aid, 1, (&t1, &c1), "Made it green.", false, false),
            change_record(&aid, 2, (&t2, &c2), "Kept your notes.", true, false),
            change_record(&aid, 3, (&t4, &c4), "Added a footer.", false, false),
            change_record(&aid, 4, (&t5, &c5), "Added a header.", false, false),
            // Version 9 was never made: the change runs again.
            change_record(&aid, 8, (&t3, &c3), "Bigger title.", false, false),
        ]});
        let s = st
            .import_client_state(
                APP_JSON_V1,
                "app",
                &files(&[("change-ledger.json", ledger)]),
            )
            .unwrap();
        assert_eq!(s.jobs, 5);
        let by = changes_by_base(&st);
        let shape = |base: u64| -> (JobState, Vec<String>) {
            let j = &by[&base];
            (j.job.state, j.steps.iter().map(|s| s.key.clone()).collect())
        };
        let keys = |ks: &[&str]| ks.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(shape(1), (JobState::Done, keys(&["publish:1", "reply"])));
        assert_eq!(shape(2), (JobState::Queued, keys(&["turn:1", "publish:2"])));
        assert_eq!(shape(3), (JobState::Done, keys(&["publish:3", "reply"])));
        assert_eq!(shape(4), (JobState::Queued, keys(&["turn:1", "publish:4"])));
        assert_eq!(shape(8), (JobState::Queued, keys(&[])));
        assert_eq!(by[&1].steps[0].version_n, Some(2));
        // The turn the queue resumes from: its finish call, planned on the
        // record's base, replies on the threads still owed one.
        let finish = |base: u64| by[&base].steps[0].result["notes"]["calls"][0].clone();
        assert_eq!(
            by[&4].steps[0].result,
            json!({
                "base": 4,
                "notes": {"calls": [{
                    "id": "imported", "tool": "finish",
                    "publish": {"base": 4, "data": null},
                    "reply": {"threads": [t5], "body": "Added a header.", "resolve": false, "ack": [c5]},
                }], "data": null},
            })
        );
        // Replied already: the reply step owes no thread.
        assert_eq!(finish(2)["reply"]["threads"], json!([]));
        assert_eq!(finish(2)["reply"]["ack"], json!([]));
    }

    /// An `asked.json` entry the app would never run again makes no job: a
    /// withdrawn question's, an answer handed over that the entry does not
    /// hold, and an answer to comments a landed, unreplied ledger change
    /// already answered (its own job posts the reply). A skipped question
    /// not yet handed over, or whose entry holds its answer, runs; so does
    /// one whose landed change replied, as the app runs it.
    #[test]
    fn an_asked_entry_the_app_would_not_run_again_makes_no_job() {
        let (_d, st) = store();
        let app = session(&st, "app", "h1");
        let aid = artifact(&st, None);
        let threads: Vec<(String, String)> = (0..6)
            .map(|i| thread_on(&st, &aid, &format!("change {i}")))
            .collect();
        // Versions 2 and 3: changes answering threads 4 and 5, the second
        // replied.
        publish_change(&st, &app, &aid, "Changed four.", &[&threads[4].0]);
        publish_change(&st, &app, &aid, "Changed five.", &[&threads[5].0]);
        agent_reply(&st, &app, &threads[5].0, "Changed five.");
        let qs: Vec<String> = (0..6).map(|_| ask(&st, &app, &aid)).collect();
        st.close_question(DAEMON, &qs[0], Close::Withdraw).unwrap();
        for q in &qs[1..] {
            st.close_question(DAEMON, q, Close::Decline).unwrap();
        }
        st.take_question(&qs[1]).unwrap();
        st.take_question(&qs[3]).unwrap();
        let entry = |i: usize, answer: Option<&str>| {
            let mut e = json!({
                "question_id": qs[i], "artifact_id": aid.as_str(),
                "thread_ids": [threads[i].0], "comment_ids": [threads[i].1],
            });
            if let Some(a) = answer {
                e["answer"] = json!(a);
            }
            e
        };
        let state = files(&[
            (
                "change-ledger.json",
                json!({"records": [
                    change_record(&aid, 1, (&threads[4].0, &threads[4].1), "Changed four.", false, false),
                    change_record(&aid, 2, (&threads[5].0, &threads[5].1), "Changed five.", false, false),
                ]}),
            ),
            (
                "asked.json",
                json!({"version": 1, "entries": [
                    entry(0, None),
                    entry(1, None),
                    entry(2, None),
                    entry(3, Some("[clax] The person skipped your question")),
                    entry(4, None),
                    entry(5, None),
                ]}),
            ),
        ]);
        let s = st.import_client_state(APP_JSON_V1, "app", &state).unwrap();
        let held: BTreeSet<String> = jobs(&st)
            .iter()
            .flat_map(|j| j.questions.iter().map(|q| q.question_id.clone()))
            .collect();
        assert_eq!(
            held,
            BTreeSet::from([qs[2].clone(), qs[3].clone(), qs[5].clone()]),
            "{:?}",
            s.left_out
        );
        assert!(
            jobs(&st)
                .iter()
                .filter(|j| !j.questions.is_empty())
                .all(|j| j.job.state == JobState::Queued)
        );
        assert_eq!(s.left_out.len(), 3, "{:?}", s.left_out);
    }

    /// The import is total over `asked.json`: a question two entries name
    /// is held by the first entry's job, and the second is left out; a
    /// question of a hook (no job's) is left out. Neither refuses the
    /// import.
    #[test]
    fn an_asked_question_named_twice_or_of_a_hook_is_left_out() {
        let (_d, st) = store();
        let app = session(&st, "app", "h1");
        let aid = artifact(&st, None);
        let (t, c) = thread_on(&st, &aid, "make it green");
        let q = ask(&st, &app, &aid);
        let hook = ask_from(&st, &app, &aid, Source::Hook);
        let entry = |q: &str| {
            json!({
                "question_id": q, "artifact_id": aid.as_str(),
                "thread_ids": [t], "comment_ids": [c],
            })
        };
        let state = files(&[(
            "asked.json",
            json!({"version": 1, "entries": [entry(&q), entry(&q), entry(&hook)]}),
        )]);
        let s = st.import_client_state(APP_JSON_V1, "app", &state).unwrap();
        let all = jobs(&st);
        assert_eq!(s.jobs, 1, "{:?}", s.left_out);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].questions.len(), 1);
        assert_eq!(all[0].questions[0].question_id, q);
        assert_eq!(all[0].job.state, JobState::Waiting);
        assert_eq!(s.left_out.len(), 2, "{:?}", s.left_out);
        assert!(
            s.left_out
                .iter()
                .any(|l| l.contains(&q) && l.contains("another entry")),
            "{:?}",
            s.left_out
        );
        assert!(
            s.left_out.iter().any(|l| l.contains(&hook)),
            "{:?}",
            s.left_out
        );
    }

    /// A pending make whose request has only the plan of the app it made
    /// is over and makes no job; a pending make with its own plan still
    /// does, holding that plan.
    #[test]
    fn a_pending_make_that_made_its_app_makes_no_job() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let plan = |request: &str| {
            json!({
                "request": request,
                "features": [{"id": "f1", "title": "Totals", "detail": "", "status": "done", "by": "agent"}],
                "updatedAt": "2026-10-18T10:00:00.000Z", "revision": 2, "spentUsd": 1.5, "stopped": false,
            })
        };
        let state = files(&[
            (
                "pending-make.json",
                json!({"requests": ["a budget", "a habit tracker"]}),
            ),
            (
                "plans.json",
                json!({"plans": {
                    aid.as_str(): plan("a budget"),
                    "new:1": plan("a habit tracker"),
                }}),
            ),
        ]);
        let s = st.import_client_state(APP_JSON_V1, "app", &state).unwrap();
        assert_eq!(s.left_out.len(), 1, "{:?}", s.left_out);
        let all = jobs(&st);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].job.kind, "make");
        assert_eq!(all[0].job.input["request"], "a habit tracker");
        assert_eq!(
            all[0].job.plan.as_ref().unwrap()["data"]["request"],
            "a habit tracker"
        );
    }

    /// An unfinished change none of whose threads is open is over and
    /// makes no job; a change with one of its threads open is still
    /// stopped, to keep going.
    #[test]
    fn an_unfinished_change_with_no_thread_open_makes_no_job() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let (t1, c1) = thread_on(&st, &aid, "make it green");
        let (t2, c2) = thread_on(&st, &aid, "bigger title");
        let (t3, c3) = thread_on(&st, &aid, "a footer");
        st.resolve_thread(DAEMON, &t1, "owner").unwrap();
        st.resolve_thread(DAEMON, &t2, "owner").unwrap();
        let unfinished = |threads: Vec<&str>, comments: Vec<&str>| {
            json!({
                "artifact": aid.as_str(), "threads": threads, "comments": comments,
                "limit": "turns", "spent_usd": 1.0, "at": "2026-10-18T10:00:00.000Z",
                "stops": 1, "notes": [], "edits": {"index.html": "<p>half"},
                "before": {"index.html": "5:abc"},
            })
        };
        let state = files(&[(
            "unfinished.json",
            json!({"version": 1, "threads": {
                t1.as_str(): unfinished(vec![&t1], vec![&c1]),
                "01JA8Z3K6Q0V7N2M4P5R6S7T8W": unfinished(vec!["01JA8Z3K6Q0V7N2M4P5R6S7T8W"], vec![]),
                t2.as_str(): unfinished(vec![&t2, &t3], vec![&c2, &c3]),
                t3.as_str(): unfinished(vec![&t2, &t3], vec![&c2, &c3]),
            }}),
        )]);
        let s = st.import_client_state(APP_JSON_V1, "app", &state).unwrap();
        assert_eq!(s.left_out.len(), 2, "{:?}", s.left_out);
        let changes: Vec<(JobState, BTreeSet<String>)> = jobs(&st)
            .iter()
            .map(|j| {
                (
                    j.job.state,
                    j.threads.iter().map(|t| t.thread_id.clone()).collect(),
                )
            })
            .collect();
        assert_eq!(
            changes,
            [(JobState::Stopped, BTreeSet::from([t2.clone(), t3.clone()]))]
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_skipped_and_named() {
        let (_d, st) = store();
        let s = st
            .import_client_state(
                APP_JSON_V1,
                "app",
                &BTreeMap::from([
                    ("plans.json".to_string(), "{\"plans\": {".to_string()),
                    ("stopped.json".to_string(), "[1, 2]".to_string()),
                    (
                        "pending-make.json".to_string(),
                        json!({"request": "a budget"}).to_string(),
                    ),
                ]),
            )
            .unwrap();
        assert_eq!(s.skipped, ["plans.json", "stopped.json"]);
        assert_eq!(s.jobs, 1);
    }

    #[test]
    fn another_key_or_file_is_refused() {
        let (_d, st) = store();
        let none = BTreeMap::new();
        assert!(st.import_client_state("app-json-v2", "app", &none).is_err());
        let odd = BTreeMap::from([("settings.json".to_string(), "{}".to_string())]);
        assert!(st.import_client_state(APP_JSON_V1, "app", &odd).is_err());
        assert!(st.open_jobs("app").unwrap().is_empty());
    }
}
