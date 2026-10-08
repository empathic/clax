//! The journal's segment files (spec 2026-10-06-toolpath-audit-design §7):
//! one `clax-audit` path per `.path.jsonl` segment, appended as events are
//! recorded, rotated by UTC day and size, and recovered from the files
//! alone after a crash.
//!
//! **Files.** Segments live in `<dir>/YYYY/MM/` as
//! `clax-<install8>-<YYYYMMDD>-<nnn>.path.jsonl`: the UTC day of the
//! segment's first event and a count within that day. Directories are
//! made 0700 and files 0600; a directory is fsynced after it is made, and
//! a file's directory after the file is made.
//!
//! **Shape (§7.2).** A segment is `PathOpen`, then each step preceded by
//! the `ActorDef` of any actor it names that is new to the segment or
//! whose merged definition grew, and, once closed (at rotation or a
//! graceful shutdown), `Head` and `PathClose`. Each step's only parent is
//! the segment's previous step; a segment names the one before it in a
//! `continues` ref.
//!
//! **Rotation (§7.1).** A segment closes at the first event whose `at`
//! falls on a later UTC day than the segment's, or when the event's lines
//! and the closing lines would take it past the size cap (a segment always
//! takes at least one step). Both depend only on the rows, so a segment
//! rewritten from the table splits where the first writing did.
//! Retention, when set, removes whole closed segments whose day is more
//! than that many days before the clock's day, each time a segment opens;
//! it never touches a `.damaged` file or the table.
//!
//! **Writing (§7.3, §7.4).** [`SegmentWriter::append_batch`] renders rows
//! into whole lines and appends them with one write per segment touched,
//! [`SegmentWriter::sync_if_due`] coalesces `sync_data` to one a second,
//! and rotation syncs a segment before it leaves it.
//!
//! **Recovery (§7.5).** There is no cursor file: the newest segment is
//! the cursor. [`SegmentWriter::recover`] truncates a partial last line,
//! renames a segment holding a line Clax did not write to `<name>.damaged`
//! (never deleting or linking it), removes an empty one, and reads the
//! cursor, the open segment's size, its last step and its actor
//! definitions back from the file. Rendering is a pure function of the
//! row, so lines lost to a crash come back byte-identical, and no step is
//! written twice. A failed write puts the writer back through recovery
//! before its next write, so an error mid-batch leaves the same state a
//! crash would.
//!
//! All file access goes through [`JournalFs`], which has no link
//! operation; [`StdFs`] is the real one, [`MemFs`] an in-memory one for
//! tests that injects failures.

use super::{KIND_URI, Obj, Redaction, RenderEnv, chain_step, clax_uri, step_id};
use crate::store::audit::AuditRow;
use crate::working::Clock;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The most rows the appender reads and writes as one batch (§7.3).
pub const BATCH_ROWS: u32 = 512;

/// Rendered bytes held before they are written, within a batch: bounds the
/// memory a batch of large events takes.
const FLUSH_BYTES: usize = 4 << 20;

/// The size of a recovery read.
const READ_CHUNK: usize = 64 << 10;

/// The longest line recovery reads into memory; a longer one is not a line
/// Clax wrote (§7.5).
const LINE_CAP: usize = 16 << 20;

/// The longest written bytes stay unsynced while events keep arriving
/// (§7.4).
const SYNC_MAX_DEFER_S: i64 = 5;

/// The closing lines' size: `Head` names a twelve-digit step ID.
const CLOSE_BYTES: u64 =
    (r#"{"Head":{"step_id":"e000000000000"}}"#.len() + 1 + r#"{"PathClose":{}}"#.len() + 1) as u64;

/// The file operations the journal uses. Every path is absolute. There is
/// no link operation: a segment is only ever created, appended to,
/// truncated, renamed or removed.
pub trait JournalFs: Send {
    /// Makes `dir` and any missing parents, each mode 0700, fsyncing each
    /// new directory's parent.
    fn create_dir_all(&mut self, dir: &Path) -> io::Result<()>;
    /// The names of `dir`'s entries; empty when `dir` does not exist.
    fn list(&mut self, dir: &Path) -> io::Result<Vec<String>>;
    /// The length of the file at `path`; `None` when there is none.
    fn len(&mut self, path: &Path) -> io::Result<Option<u64>>;
    /// Reads from `path` at `offset` into `buf`, returning the bytes read
    /// (0 at the end).
    fn read_at(&mut self, path: &Path, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    /// Creates an empty file at `path`, mode 0600, failing when one exists,
    /// and fsyncs its directory.
    fn create(&mut self, path: &Path) -> io::Result<()>;
    /// Appends all of `data` to `path`. On an error, some prefix of `data`
    /// may have been written.
    fn append(&mut self, path: &Path, data: &[u8]) -> io::Result<()>;
    /// Flushes `path`'s data to the disk.
    fn sync_data(&mut self, path: &Path) -> io::Result<()>;
    /// Truncates `path` to `len` bytes.
    fn set_len(&mut self, path: &Path, len: u64) -> io::Result<()>;
    /// Renames `from` to `to`, failing when `to` exists, and fsyncs the
    /// directory.
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    /// Removes the file at `path`, and fsyncs its directory.
    fn remove(&mut self, path: &Path) -> io::Result<()>;
}

/// What a segment writer is told: the install, the build writing (named
/// in each `PathOpen`), the size cap, retention and the redaction the
/// journal renders under (§7.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentConfig {
    pub install: String,
    pub clax_version: String,
    pub clax_commit: String,
    /// The size a segment stays within, in bytes (`[toolpath]
    /// segment_max_mb`).
    pub max_bytes: u64,
    /// Days a closed segment is kept; 0 keeps every segment (`[toolpath]
    /// journal_retain_days`).
    pub retain_days: u32,
    pub redaction: Redaction,
}

impl SegmentConfig {
    /// The defaults of §7.1 for `install`, as built by `clax_version` at
    /// `clax_commit`: 64 MiB segments kept forever, rendering text.
    pub fn new(
        install: impl Into<String>,
        clax_version: impl Into<String>,
        clax_commit: impl Into<String>,
    ) -> SegmentConfig {
        SegmentConfig {
            install: install.into(),
            clax_version: clax_version.into(),
            clax_commit: clax_commit.into(),
            max_bytes: 64 << 20,
            retain_days: 0,
            redaction: Redaction::NONE,
        }
    }
}

/// A segment's place in the sequence: its UTC day (`YYYYMMDD`) and its
/// count within the day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SegId {
    day: u32,
    nnn: u32,
}

/// A segment file found in the journal directory.
#[derive(Debug, Clone)]
struct Found {
    id: SegId,
    name: String,
    path: PathBuf,
    damaged: bool,
}

/// The open segment, as written so far.
#[derive(Debug, Clone)]
struct Open {
    id: SegId,
    name: String,
    path: PathBuf,
    /// Bytes in the file, including those of this batch not yet written.
    size: u64,
    /// The `continues` ref of its `PathOpen`, written with its first step.
    continues: Option<String>,
    last_seq: Option<i64>,
    actors: BTreeMap<String, Value>,
    /// The redaction its lines are rendered under, as its `PathOpen` says.
    redaction: Redaction,
    /// The size cap it was opened under, as its `PathOpen` says.
    max_bytes: u64,
}

/// The journal's segment writer: the only handle on the open segment.
pub struct SegmentWriter {
    dir: PathBuf,
    cfg: SegmentConfig,
    env: RenderEnv,
    clock: Arc<dyn Clock>,
    fs: Box<dyn JournalFs>,
    open: Option<Open>,
    /// The last `seq` journalled.
    cursor: i64,
    /// The newest segment before the open one (or the newest of all, when
    /// none is open): the file name a new segment `continues`.
    prev: Option<(SegId, String)>,
    /// The file has not been read back since a write failed, or ever.
    needs_recovery: bool,
    /// When bytes were first written since the last `sync_data`.
    unsynced_since: Option<DateTime<Utc>>,
    /// Why retention could not remove a segment, until a removal succeeds.
    warning: Option<String>,
    /// When this writer first read the journal back (the restart): events
    /// recorded before it may be lines a crash lost, so they continue the
    /// open segment under the options it was written with.
    recovered_at: Option<DateTime<Utc>>,
}

impl SegmentWriter {
    /// A writer for the journal in `dir` that has not read it yet: the
    /// first [`append_batch`](Self::append_batch) recovers first, and
    /// [`cursor`](Self::cursor) is `None` until then.
    pub fn new(
        dir: impl Into<PathBuf>,
        cfg: SegmentConfig,
        clock: Arc<dyn Clock>,
        fs: Box<dyn JournalFs>,
    ) -> SegmentWriter {
        SegmentWriter {
            dir: dir.into(),
            env: RenderEnv::journal(cfg.install.clone()),
            cfg,
            clock,
            fs,
            open: None,
            cursor: 0,
            prev: None,
            needs_recovery: true,
            unsynced_since: None,
            warning: None,
            recovered_at: None,
        }
    }

    /// A writer for the journal in `dir`, recovered (§7.5).
    ///
    /// # Errors
    /// The file error recovery met.
    pub fn open_or_recover(
        dir: impl Into<PathBuf>,
        cfg: SegmentConfig,
        clock: Arc<dyn Clock>,
        fs: Box<dyn JournalFs>,
    ) -> io::Result<SegmentWriter> {
        let mut w = SegmentWriter::new(dir, cfg, clock, fs);
        w.recover()?;
        Ok(w)
    }

    /// The last `seq` journalled: every event up to it is in the journal.
    /// `None` until the journal has been read.
    pub fn cursor(&self) -> Option<i64> {
        (!self.needs_recovery).then_some(self.cursor)
    }

    /// The open segment's file name, else the newest one's.
    pub fn segment(&self) -> Option<&str> {
        self.open
            .as_ref()
            .map(|o| o.name.as_str())
            .or(self.prev.as_ref().map(|(_, n)| n.as_str()))
    }

    /// Whether bytes have been written since the last `sync_data`.
    pub fn unsynced(&self) -> bool {
        self.unsynced_since.is_some()
    }

    /// Why retention last failed to remove a segment, until one succeeds.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    fn install8(&self) -> &str {
        self.cfg.install.get(..8).unwrap_or(&self.cfg.install)
    }

    fn file_name(&self, id: SegId) -> String {
        format!(
            "clax-{}-{:08}-{:03}.path.jsonl",
            self.install8(),
            id.day,
            id.nnn
        )
    }

    fn month_dir(&self, day: u32) -> PathBuf {
        self.dir
            .join(format!("{:04}", day / 10000))
            .join(format!("{:02}", day / 100 % 100))
    }

    /// `name` as one of this install's segments (or a `.damaged` one).
    fn parse_name(&self, name: &str) -> Option<(SegId, bool)> {
        let (stem, damaged) = match name.find(".path.jsonl") {
            Some(i) if name[i..] == *".path.jsonl" => (&name[..i], false),
            Some(i) if name[i..].starts_with(".path.jsonl.damaged") => (&name[..i], true),
            _ => return None,
        };
        let rest = stem.strip_prefix("clax-")?.strip_prefix(self.install8())?;
        let rest = rest.strip_prefix('-')?;
        let (day, nnn) = rest.split_once('-')?;
        let digits = |s: &str, n: usize| s.len() >= n && s.bytes().all(|b| b.is_ascii_digit());
        if !(day.len() == 8 && digits(day, 8) && digits(nnn, 3)) {
            return None;
        }
        Some((
            SegId {
                day: day.parse().ok()?,
                nnn: nnn.parse().ok()?,
            },
            damaged,
        ))
    }

    /// Every segment of this install in the directory, oldest first.
    fn segments(&mut self) -> io::Result<Vec<Found>> {
        let mut out = Vec::new();
        let num = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
        let dir = self.dir.clone();
        for y in self.fs.list(&dir)? {
            if !num(&y, 4) {
                continue;
            }
            for m in self.fs.list(&dir.join(&y))? {
                if !num(&m, 2) {
                    continue;
                }
                let month = dir.join(&y).join(&m);
                for name in self.fs.list(&month)? {
                    if let Some((id, damaged)) = self.parse_name(&name) {
                        out.push(Found {
                            id,
                            path: month.join(&name),
                            name,
                            damaged,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| (a.id, a.damaged, &a.name).cmp(&(b.id, b.damaged, &b.name)));
        Ok(out)
    }

    /// Reads the journal back (§7.5): the newest segment's partial last
    /// line is truncated, an empty segment removed, a segment holding a
    /// line Clax did not write renamed `.damaged`, and the cursor, the open
    /// segment and its actor definitions read from what remains.
    ///
    /// # Errors
    /// The file error met; the writer stays unrecovered.
    pub fn recover(&mut self) -> io::Result<()> {
        // The restart's time, taken once and before the scan: an event
        // recorded while the scan runs, or after a later read-back (a failed
        // write), was still recorded under the new settings.
        if self.recovered_at.is_none() {
            self.recovered_at = Some(self.clock.now());
        }
        self.needs_recovery = true;
        self.open = None;
        self.prev = None;
        self.cursor = 0;
        let mut segs: Vec<Found> = self
            .segments()?
            .into_iter()
            .filter(|f| !f.damaged)
            .collect();
        while let Some(f) = segs.pop() {
            match self.scan(&f)? {
                Scan::Empty => {
                    tracing::warn!(segment = %f.name, "removing an empty journal segment");
                    self.fs.remove(&f.path)?;
                }
                Scan::Damaged { max_seq } => {
                    let to = self.damaged_name(&f)?;
                    tracing::error!(segment = %f.name, renamed = %to.display(),
                        "a journal segment holds a line Clax did not write; renamed it, and the journal goes on in a new segment");
                    self.fs.rename(&f.path, &to)?;
                    if let Some(seq) = max_seq {
                        let name = to
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.cursor = seq;
                        self.prev = Some((f.id, name));
                        break;
                    }
                }
                Scan::Ok {
                    size,
                    first_seq,
                    last_seq,
                    actors,
                    state,
                    redaction,
                    max_bytes,
                } => {
                    self.cursor = last_seq.unwrap_or(first_seq - 1);
                    match state {
                        End::Closed => self.prev = Some((f.id, f.name)),
                        End::Headed => {
                            self.fs.append(&f.path, b"{\"PathClose\":{}}\n")?;
                            self.fs.sync_data(&f.path)?;
                            self.prev = Some((f.id, f.name));
                        }
                        End::Open => {
                            self.prev = segs.last().map(|p| (p.id, p.name.clone()));
                            self.open = Some(Open {
                                id: f.id,
                                name: f.name,
                                path: f.path,
                                size,
                                continues: None,
                                last_seq,
                                actors,
                                redaction,
                                max_bytes: max_bytes.unwrap_or(self.cfg.max_bytes),
                            });
                        }
                    }
                    break;
                }
            }
        }
        self.needs_recovery = false;
        Ok(())
    }

    /// A free `.damaged` name beside `f`.
    fn damaged_name(&mut self, f: &Found) -> io::Result<PathBuf> {
        let base = f.path.with_file_name(format!("{}.damaged", f.name));
        let mut to = base.clone();
        let mut n = 1;
        while self.fs.len(&to)?.is_some() {
            n += 1;
            to = f.path.with_file_name(format!("{}.damaged.{n}", f.name));
        }
        Ok(to)
    }

    /// Reads segment `f` line by line, first truncating a partial last line.
    fn scan(&mut self, f: &Found) -> io::Result<Scan> {
        let len = self.fs.len(&f.path)?.unwrap_or(0);
        let mut off = 0u64;
        let mut buf = vec![0u8; READ_CHUNK];
        let mut line: Vec<u8> = Vec::new();
        let mut v = Validator::default();
        let mut over = false;
        let mut complete = 0u64;
        let mut pending = 0u64;
        while off < len {
            let n = self.fs.read_at(&f.path, off, &mut buf)?;
            if n == 0 {
                break;
            }
            off += n as u64;
            let mut rest = &buf[..n];
            while let Some(i) = rest.iter().position(|&b| b == b'\n') {
                let len = pending + i as u64;
                if over || len > LINE_CAP as u64 {
                    v.too_long();
                } else {
                    line.extend_from_slice(&rest[..i]);
                    v.line(&line);
                }
                complete += len + 1;
                line.clear();
                pending = 0;
                over = false;
                rest = &rest[i + 1..];
            }
            pending += rest.len() as u64;
            if over || pending > LINE_CAP as u64 {
                // Measured, not kept: a line this long is not one Clax wrote.
                over = true;
                line.clear();
            } else {
                line.extend_from_slice(rest);
            }
        }
        if complete < off {
            tracing::warn!(segment = %f.name, dropped = off - complete,
                "truncating a partial line from the end of a journal segment");
            self.fs.set_len(&f.path, complete)?;
            self.fs.sync_data(&f.path)?;
        }
        if complete == 0 {
            return Ok(Scan::Empty);
        }
        Ok(v.finish(complete))
    }

    /// Closes the open segment when `row` belongs in a later one by day: it
    /// falls on a later UTC day than the segment's (§7.1). Returns whether
    /// it closed one.
    ///
    /// # Errors
    /// The file error met; the writer reads the journal back before its
    /// next write.
    pub fn maybe_rotate(&mut self, row: &AuditRow) -> io::Result<bool> {
        let day = self.day_of(row);
        match &self.open {
            Some(o) if o.last_seq.is_some() && day > o.id.day => {
                self.close_open(&mut Vec::new())?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// The UTC day `row` was recorded on, as `YYYYMMDD`; the open
    /// segment's (else the clock's) when its time does not parse.
    fn day_of(&self, row: &AuditRow) -> u32 {
        let fallback = || {
            self.open
                .as_ref()
                .map(|o| o.id.day)
                .unwrap_or_else(|| day_number(self.clock.now().date_naive()))
        };
        DateTime::parse_from_rfc3339(&row.at)
            .map(|t| day_number(t.with_timezone(&Utc).date_naive()))
            .unwrap_or_else(|_| fallback())
    }

    /// Appends `rows` (in `seq` order; those at or below the cursor are
    /// skipped), rotating where §7.1 says. Recovers first when the journal
    /// has not been read, or a write failed since it was.
    ///
    /// # Errors
    /// The file error met. What was written before it stays; anything
    /// after it is read back from the file before the next write, and
    /// written again by the next call with the same rows.
    pub fn append_batch(&mut self, rows: &[AuditRow]) -> io::Result<()> {
        let r = self.append_inner(rows);
        if r.is_err() {
            self.needs_recovery = true;
        }
        r
    }

    fn append_inner(&mut self, rows: &[AuditRow]) -> io::Result<()> {
        if self.needs_recovery {
            self.recover()?;
        }
        if let Some(o) = &self.open
            && self.fs.len(&o.path)?.is_none()
        {
            tracing::warn!(segment = %o.name, "the open journal segment is gone; the journal goes on in a new segment");
            let o = self.open.take().expect("open");
            self.prev = Some((o.id, o.name));
        }
        let cutoff = self.retention_cutoff();
        let mut buf: Vec<u8> = Vec::new();
        for row in rows {
            if row.seq <= self.cursor {
                continue;
            }
            let day = self.day_of(row);
            // An event already past retention is never written (§7.1).
            if cutoff.is_some_and(|c| day < c) {
                self.cursor = row.seq;
                continue;
            }
            // A later day begins a new segment, and so do settings other
            // than the open segment was written under, from the first event
            // recorded since this writer first read the journal back: an
            // earlier one may be a line a crash lost, which comes back as it
            // was written.
            let settings_changed = |o: &Open| {
                (o.redaction != self.cfg.redaction || o.max_bytes != self.cfg.max_bytes)
                    && DateTime::parse_from_rfc3339(&row.at)
                        .map_or(true, |t| self.recovered_at.is_none_or(|r| t >= r))
            };
            if matches!(&self.open, Some(o) if o.last_seq.is_some()
                && (day > o.id.day || settings_changed(o)))
            {
                self.close_open(&mut buf)?;
            }
            loop {
                if self.open.is_none() {
                    self.start_segment(day)?;
                }
                let o = self.open.as_ref().expect("a segment is open");
                let (step, grew) = chain_step(
                    row,
                    o.last_seq,
                    &self.env,
                    &o.redaction,
                    &o.actors,
                    "the journal",
                );
                let mut lines = Vec::new();
                if o.size == 0 {
                    push_line(&mut lines, &self.path_open(o, row.seq));
                }
                for (actor, definition) in &grew {
                    push_line(
                        &mut lines,
                        &json!({"ActorDef": {"actor": actor, "definition": definition}}),
                    );
                }
                push_line(&mut lines, &json!({"Step": step}));
                let n = lines.len() as u64;
                if o.last_seq.is_some() && o.size + n + CLOSE_BYTES > o.max_bytes {
                    self.close_open(&mut buf)?;
                    continue;
                }
                let o = self.open.as_mut().expect("a segment is open");
                o.size += n;
                o.last_seq = Some(row.seq);
                o.actors.extend(grew);
                buf.extend_from_slice(&lines);
                self.cursor = row.seq;
                break;
            }
            if buf.len() >= FLUSH_BYTES {
                self.flush(&mut buf)?;
            }
        }
        self.flush(&mut buf)
    }

    /// The first UTC day (`YYYYMMDD`) inside `journal_retain_days` of the
    /// clock's day; `None` when every segment is kept.
    fn retention_cutoff(&self) -> Option<u32> {
        if self.cfg.retain_days == 0 {
            return None;
        }
        let today = self.clock.now().date_naive();
        today
            .checked_sub_days(chrono::Days::new(self.cfg.retain_days.into()))
            .map(day_number)
    }

    /// The open segment's `PathOpen`, its first step `first_seq`. It
    /// records the options the segment is rendered under (its redaction
    /// and size cap), which recovery reads back (§7.5).
    fn path_open(&self, o: &Open, first_seq: i64) -> Value {
        let install = &self.cfg.install;
        let uri = clax_uri(install, Obj::Install);
        let day = o.id.day;
        let date = format!("{:04}-{:02}-{:02}", day / 10000, day / 100 % 100, day % 100);
        let segment = format!("{:08}-{:03}", day, o.id.nnn);
        let clax = json!({
            "projection": "journal",
            "install": install,
            "segment": segment,
            "first_seq": first_seq,
            "clax_version": self.cfg.clax_version,
            "clax_commit": self.cfg.clax_commit,
            "redaction": o.redaction.names(),
            "segment_max_bytes": o.max_bytes,
        });
        let mut meta = json!({
            "title": format!("Clax audit trail {date} #{}", o.id.nnn),
            "kind": KIND_URI,
            "source": uri,
            "clax": clax,
        });
        if let Some(prev) = &o.continues {
            meta["refs"] = json!([{"rel": "continues", "href": prev}]);
        }
        json!({"PathOpen": {
            "version": "1",
            "id": format!("clax-journal-{}-{segment}", self.install8()),
            "base": {"uri": uri},
            "graph_ref": format!("toolpath://clax/{install}"),
            "meta": meta,
        }})
    }

    /// Writes `buf` to the open segment.
    fn flush(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let path = &self.open.as_ref().expect("bytes belong to a segment").path;
        self.fs.append(path, buf)?;
        buf.clear();
        if self.unsynced_since.is_none() {
            self.unsynced_since = Some(self.clock.now());
        }
        Ok(())
    }

    /// Writes `buf`, then the open segment's `Head` and `PathClose`, syncs
    /// it (recovery reads only the newest segment, so an older one must be
    /// on the disk), and leaves no segment open.
    fn close_open(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        let Some(o) = self.open.as_ref() else {
            return Ok(());
        };
        if let Some(last) = o.last_seq {
            push_line(buf, &json!({"Head": {"step_id": step_id(last)}}));
            push_line(buf, &json!({"PathClose": {}}));
        }
        self.flush(buf)?;
        let o = self.open.take().expect("open");
        self.fs.sync_data(&o.path)?;
        self.unsynced_since = None;
        self.prev = Some((o.id, o.name));
        Ok(())
    }

    /// Creates the segment a row of `day` opens, under the configured
    /// options, and applies retention.
    fn start_segment(&mut self, day: u32) -> io::Result<()> {
        let month = self.month_dir(day);
        self.fs.create_dir_all(&month)?;
        let mut nnn = 1;
        for name in self.fs.list(&month)? {
            if let Some((id, _)) = self.parse_name(&name)
                && id.day == day
            {
                nnn = nnn.max(id.nnn + 1);
            }
        }
        if let Some((p, _)) = &self.prev
            && p.day == day
        {
            nnn = nnn.max(p.nnn + 1);
        }
        let id = SegId { day, nnn };
        let name = self.file_name(id);
        let path = month.join(&name);
        self.fs.create(&path)?;
        self.open = Some(Open {
            id,
            name,
            path,
            size: 0,
            continues: self.prev.as_ref().map(|(_, n)| n.clone()),
            last_seq: None,
            actors: BTreeMap::new(),
            redaction: self.cfg.redaction,
            max_bytes: self.cfg.max_bytes,
        });
        self.retain(id);
        Ok(())
    }

    /// Removes the closed segments wholly before the retention cutoff,
    /// except the one just opened. Every event in a segment falls on its
    /// day or earlier (a later day begins a new segment), so a segment
    /// whose day is before the cutoff holds no event inside retention.
    /// Best effort: a segment that cannot be listed or removed is logged,
    /// kept in [`warning`](Self::warning), and tried again at the next
    /// open; the journal goes on.
    fn retain(&mut self, keep: SegId) {
        let Some(cutoff) = self.retention_cutoff() else {
            return;
        };
        let segs = match self.segments() {
            Ok(s) => s,
            Err(e) => {
                self.retention_failed(format!("listing journal segments: {e}"));
                return;
            }
        };
        let mut failed = false;
        for f in segs {
            if !f.damaged && f.id != keep && f.id.day < cutoff {
                tracing::info!(segment = %f.name, "removing a journal segment past journal_retain_days");
                if let Err(e) = self.fs.remove(&f.path) {
                    failed = true;
                    self.retention_failed(format!("removing {}: {e}", f.name));
                }
            }
        }
        if !failed {
            self.warning = None;
        }
    }

    fn retention_failed(&mut self, why: String) {
        tracing::warn!(error = %why, "journal retention could not remove a segment; the journal goes on");
        self.warning = Some(format!("journal_retain_days: {why}"));
    }

    /// Syncs the open segment when its bytes have been unsynced for
    /// [`SYNC_MAX_DEFER_S`] seconds or more (§7.4): a steady stream of
    /// events is synced that often, and a quiet moment syncs sooner
    /// ([`sync`](Self::sync)). Returns whether it synced.
    ///
    /// # Errors
    /// The file error met; the writer reads its segment back before its
    /// next write.
    pub fn sync_if_due(&mut self) -> io::Result<bool> {
        let Some(since) = self.unsynced_since else {
            return Ok(false);
        };
        if self.clock.now() - since < chrono::Duration::seconds(SYNC_MAX_DEFER_S) {
            return Ok(false);
        }
        self.sync()?;
        Ok(true)
    }

    /// Syncs the open segment's unsynced bytes now.
    ///
    /// # Errors
    /// The file error met. After a failed sync the kernel may have dropped
    /// the bytes, so the writer reads its segment back before its next
    /// write.
    pub fn sync(&mut self) -> io::Result<()> {
        if self.unsynced_since.is_none() {
            return Ok(());
        }
        if let Some(o) = &self.open
            && let Err(e) = self.fs.sync_data(&o.path)
        {
            self.needs_recovery = true;
            return Err(e);
        }
        self.unsynced_since = None;
        Ok(())
    }

    /// Closes the open segment for a graceful shutdown: its `Head` and
    /// `PathClose`, synced. The next event opens a new segment.
    ///
    /// # Errors
    /// The file error met.
    pub fn close(&mut self) -> io::Result<()> {
        if self.needs_recovery {
            return Ok(());
        }
        let r = self.close_open(&mut Vec::new());
        if r.is_err() {
            self.needs_recovery = true;
        }
        r
    }
}

/// `day` as `YYYYMMDD`.
fn day_number(day: NaiveDate) -> u32 {
    day.year().max(0) as u32 * 10000 + day.month() * 100 + day.day()
}

fn push_line(buf: &mut Vec<u8>, v: &Value) {
    serde_json::to_writer(&mut *buf, v).expect("a value serialises");
    buf.push(b'\n');
}

/// How a segment's last complete line leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Open,
    /// `Head` without `PathClose`: an interrupted close.
    Headed,
    Closed,
}

/// What reading a segment found.
enum Scan {
    Empty,
    /// A line Clax did not write; the highest `seq` of the steps that
    /// parse.
    Damaged {
        max_seq: Option<i64>,
    },
    Ok {
        size: u64,
        first_seq: i64,
        last_seq: Option<i64>,
        actors: BTreeMap<String, Value>,
        state: End,
        redaction: Redaction,
        max_bytes: Option<u64>,
    },
}

/// Checks a segment's lines against the shape this module writes.
#[derive(Default)]
struct Validator {
    lines: u64,
    first_seq: Option<i64>,
    last_seq: Option<i64>,
    max_seq: Option<i64>,
    actors: BTreeMap<String, Value>,
    redaction: Redaction,
    max_bytes: Option<u64>,
    head: bool,
    closed: bool,
    damaged: bool,
}

impl Validator {
    /// A line longer than [`LINE_CAP`]: not one Clax wrote.
    fn too_long(&mut self) {
        self.lines += 1;
        self.damaged = true;
    }

    fn line(&mut self, bytes: &[u8]) {
        self.lines += 1;
        let v: Option<Value> = serde_json::from_slice(bytes).ok();
        let one = v
            .as_ref()
            .and_then(Value::as_object)
            .filter(|o| o.len() == 1)
            .and_then(|o| o.iter().next());
        let Some((variant, body)) = one else {
            self.damaged = true;
            return;
        };
        if self.closed || (self.head && variant != "PathClose") {
            self.damaged = true;
        }
        match (self.lines, variant.as_str()) {
            (1, "PathOpen") => {
                let clax = body.pointer("/meta/clax");
                match clax
                    .and_then(|c| c.get("first_seq"))
                    .and_then(Value::as_i64)
                {
                    Some(s) => self.first_seq = Some(s),
                    None => self.damaged = true,
                }
                match clax.and_then(|c| c.get("redaction")).map(redaction_of) {
                    Some(Some(r)) => self.redaction = r,
                    Some(None) => self.damaged = true,
                    None => {}
                }
                self.max_bytes = clax
                    .and_then(|c| c.get("segment_max_bytes"))
                    .and_then(Value::as_u64);
            }
            (1, _) | (_, "PathOpen") => self.damaged = true,
            (_, "Step") => match body.pointer("/meta/clax/seq").and_then(Value::as_i64) {
                Some(s) => {
                    if self.last_seq.is_some_and(|l| s <= l) {
                        self.damaged = true;
                    }
                    self.last_seq = Some(s);
                    self.max_seq = Some(self.max_seq.map_or(s, |m| m.max(s)));
                }
                None => self.damaged = true,
            },
            (_, "ActorDef") => {
                match (
                    body.get("actor").and_then(Value::as_str),
                    body.get("definition"),
                ) {
                    (Some(a), Some(d)) => {
                        self.actors.insert(a.to_string(), d.clone());
                    }
                    _ => self.damaged = true,
                }
            }
            (_, "Head") => self.head = true,
            (_, "PathClose") => self.closed = true,
            _ => self.damaged = true,
        }
    }

    fn finish(self, size: u64) -> Scan {
        match (self.damaged, self.first_seq) {
            (false, Some(first_seq)) => Scan::Ok {
                size,
                first_seq,
                last_seq: self.last_seq,
                actors: self.actors,
                redaction: self.redaction,
                max_bytes: self.max_bytes,
                state: if self.closed {
                    End::Closed
                } else if self.head {
                    End::Headed
                } else {
                    End::Open
                },
            },
            _ => Scan::Damaged {
                max_seq: self.max_seq,
            },
        }
    }
}

/// The redaction a `PathOpen`'s `meta.clax.redaction` names (the CLI
/// option names); `None` when it names something else.
fn redaction_of(v: &Value) -> Option<Redaction> {
    let mut r = Redaction::NONE;
    for name in v.as_array()? {
        match name.as_str()? {
            "no-text" => r.no_text = true,
            "no-names" => r.no_names = true,
            "no-paths" => r.no_paths = true,
            _ => return None,
        }
    }
    Some(r)
}

/// The real file system. Holds the segment last appended to open, so an
/// append is one `write_all`, and the file being read back open, so a
/// recovery opens it once.
///
/// Syncs are a plain `fsync(2)` (through `nix`), for files and directories
/// alike: the durability the store's own commits rely on. Rust's
/// `File::sync_data` is `F_FULLFSYNC` on Apple systems, which flushes the
/// whole drive cache and would make the journal, rebuilt from the table on
/// recovery, stricter than the table itself.
///
/// [`JournalFs::rename`] checks for its target and then renames, which
/// relies on the journal having one writer.
#[derive(Default)]
pub struct StdFs {
    open: Option<(PathBuf, std::fs::File)>,
    reading: Option<(PathBuf, std::fs::File)>,
}

impl StdFs {
    pub fn new() -> StdFs {
        StdFs::default()
    }

    fn file(&mut self, path: &Path) -> io::Result<&mut std::fs::File> {
        if self.open.as_ref().is_none_or(|(p, _)| p != path) {
            let f = std::fs::OpenOptions::new().append(true).open(path)?;
            self.open = Some((path.to_path_buf(), f));
        }
        Ok(&mut self.open.as_mut().expect("just opened").1)
    }

    fn forget(&mut self, path: &Path) {
        if self.open.as_ref().is_some_and(|(p, _)| p == path) {
            self.open = None;
        }
        if self.reading.as_ref().is_some_and(|(p, _)| p == path) {
            self.reading = None;
        }
    }
}

/// `fsync(2)` on `f`.
fn fsync(f: &std::fs::File) -> io::Result<()> {
    nix::unistd::fsync(f).map_err(io::Error::from)
}

/// fsyncs directory `dir`.
fn sync_dir(dir: &Path) -> io::Result<()> {
    fsync(&std::fs::File::open(dir)?)
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a path with no parent"))
}

impl JournalFs for StdFs {
    fn create_dir_all(&mut self, dir: &Path) -> io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        if dir.is_dir() {
            return Ok(());
        }
        if let Some(p) = dir.parent() {
            self.create_dir_all(p)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => return Ok(()),
            Err(e) => return Err(e),
        }
        sync_dir(dir)?;
        sync_dir(parent(dir)?)
    }

    fn list(&mut self, dir: &Path) -> io::Result<Vec<String>> {
        match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn len(&mut self, path: &Path) -> io::Result<Option<u64>> {
        match std::fs::symlink_metadata(path) {
            Ok(m) => Ok(Some(m.len())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn read_at(&mut self, path: &Path, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        if self.reading.as_ref().is_none_or(|(p, _)| p != path) {
            self.reading = Some((path.to_path_buf(), std::fs::File::open(path)?));
        }
        let f = &self.reading.as_ref().expect("just opened").1;
        f.read_at(buf, offset)
    }

    fn create(&mut self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .append(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        sync_dir(parent(path)?)?;
        self.open = Some((path.to_path_buf(), f));
        Ok(())
    }

    fn append(&mut self, path: &Path, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let r = self.file(path)?.write_all(data);
        if r.is_err() {
            self.forget(path);
        }
        r
    }

    fn sync_data(&mut self, path: &Path) -> io::Result<()> {
        let f = self.file(path)?;
        fsync(f)
    }

    fn set_len(&mut self, path: &Path, len: u64) -> io::Result<()> {
        self.forget(path);
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(len)
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        if self.len(to)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the rename's target exists",
            ));
        }
        self.forget(from);
        std::fs::rename(from, to)?;
        sync_dir(parent(to)?)
    }

    fn remove(&mut self, path: &Path) -> io::Result<()> {
        self.forget(path);
        std::fs::remove_file(path)?;
        sync_dir(parent(path)?)
    }
}

/// An in-memory [`JournalFs`] for tests. Clones share one file system, so
/// a test keeps a clone to read the files and inject failures while a
/// writer holds another.
#[derive(Clone, Default)]
pub struct MemFs(Arc<Mutex<MemState>>);

/// [`MemFs`]'s contents and fault plan.
#[derive(Default)]
pub struct MemState {
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    /// Directories, with the mode each was made with.
    pub dirs: BTreeMap<PathBuf, u32>,
    /// The mode each file was created with.
    pub modes: BTreeMap<PathBuf, u32>,
    /// `sync_data` calls, by path.
    pub syncs: BTreeMap<PathBuf, u32>,
    /// `append` calls, failed ones included.
    pub appends: u32,
    /// Files created, by path, in order.
    pub created: Vec<PathBuf>,
    /// An operation that panics when called, to test a dying writer.
    pub panic_on: Option<&'static str>,
    /// Directory fsyncs, by directory.
    pub dir_syncs: BTreeMap<PathBuf, u32>,
    /// The next this many operations named in `fail_ops` fail with
    /// `fail_with`.
    pub fail_count: u32,
    pub fail_ops: Vec<&'static str>,
    pub fail_with: Option<io::ErrorKind>,
    /// The raw OS error the failures carry (`ENOSPC`, `EIO`), when set.
    pub fail_errno: Option<i32>,
    /// The next failing `append` writes this many bytes first.
    pub partial: Option<usize>,
}

impl MemFs {
    pub fn new() -> MemFs {
        MemFs::default()
    }

    /// The shared state.
    pub fn state(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Makes the next `count` calls of the operations `ops` (`append`,
    /// `create`, `sync_data`, `create_dir_all`, ...) fail with OS error
    /// `errno`.
    pub fn fail(&self, ops: &[&'static str], count: u32, errno: i32) {
        let mut s = self.state();
        s.fail_ops = ops.to_vec();
        s.fail_count = count;
        s.fail_errno = Some(errno);
        s.fail_with = None;
    }

    /// The file at `path`, as text.
    pub fn text(&self, path: &Path) -> Option<String> {
        self.state()
            .files
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// Every file's path, in order.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.state().files.keys().cloned().collect()
    }

    fn check(&self, op: &'static str) -> io::Result<()> {
        let mut s = self.state();
        if s.panic_on == Some(op) {
            drop(s);
            panic!("injected panic in {op}");
        }
        if s.fail_count > 0 && s.fail_ops.contains(&op) {
            s.fail_count -= 1;
            return Err(match (s.fail_errno, s.fail_with) {
                (Some(n), _) => io::Error::from_raw_os_error(n),
                (None, Some(k)) => io::Error::from(k),
                (None, None) => io::Error::other("injected failure"),
            });
        }
        Ok(())
    }
}

impl JournalFs for MemFs {
    fn create_dir_all(&mut self, dir: &Path) -> io::Result<()> {
        self.check("create_dir_all")?;
        let mut s = self.state();
        let mut made = Vec::new();
        let mut d = Some(dir);
        while let Some(x) = d {
            if x.as_os_str().is_empty() || x == Path::new("/") || s.dirs.contains_key(x) {
                break;
            }
            made.push(x.to_path_buf());
            d = x.parent();
        }
        for x in made.into_iter().rev() {
            s.dirs.insert(x.clone(), 0o700);
            if let Some(p) = x.parent() {
                *s.dir_syncs.entry(p.to_path_buf()).or_default() += 1;
            }
            *s.dir_syncs.entry(x).or_default() += 1;
        }
        Ok(())
    }

    fn list(&mut self, dir: &Path) -> io::Result<Vec<String>> {
        self.check("list")?;
        let s = self.state();
        let mut names: Vec<String> = s
            .files
            .keys()
            .chain(s.dirs.keys())
            .filter(|p| p.parent() == Some(dir))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn len(&mut self, path: &Path) -> io::Result<Option<u64>> {
        self.check("len")?;
        Ok(self.state().files.get(path).map(|f| f.len() as u64))
    }

    fn read_at(&mut self, path: &Path, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.check("read_at")?;
        let s = self.state();
        let f = s.files.get(path).ok_or(io::ErrorKind::NotFound)?;
        let from = (offset as usize).min(f.len());
        let n = buf.len().min(f.len() - from);
        buf[..n].copy_from_slice(&f[from..from + n]);
        Ok(n)
    }

    fn create(&mut self, path: &Path) -> io::Result<()> {
        self.check("create")?;
        let mut s = self.state();
        let dir = parent(path)?.to_path_buf();
        if !s.dirs.contains_key(&dir) {
            return Err(io::ErrorKind::NotFound.into());
        }
        if s.files.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        s.files.insert(path.to_path_buf(), Vec::new());
        s.modes.insert(path.to_path_buf(), 0o600);
        s.created.push(path.to_path_buf());
        *s.dir_syncs.entry(dir).or_default() += 1;
        Ok(())
    }

    fn append(&mut self, path: &Path, data: &[u8]) -> io::Result<()> {
        let failed = self.check("append");
        let mut s = self.state();
        s.appends += 1;
        let partial = if failed.is_err() {
            s.partial.take()
        } else {
            None
        };
        let f = s.files.get_mut(path).ok_or(io::ErrorKind::NotFound)?;
        match failed {
            Ok(()) => {
                f.extend_from_slice(data);
                Ok(())
            }
            Err(e) => {
                if let Some(n) = partial {
                    f.extend_from_slice(&data[..n.min(data.len())]);
                }
                Err(e)
            }
        }
    }

    fn sync_data(&mut self, path: &Path) -> io::Result<()> {
        self.check("sync_data")?;
        let mut s = self.state();
        if !s.files.contains_key(path) {
            return Err(io::ErrorKind::NotFound.into());
        }
        *s.syncs.entry(path.to_path_buf()).or_default() += 1;
        Ok(())
    }

    fn set_len(&mut self, path: &Path, len: u64) -> io::Result<()> {
        self.check("set_len")?;
        let mut s = self.state();
        let f = s.files.get_mut(path).ok_or(io::ErrorKind::NotFound)?;
        f.resize(len as usize, 0);
        Ok(())
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.check("rename")?;
        let mut s = self.state();
        if s.files.contains_key(to) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let f = s.files.remove(from).ok_or(io::ErrorKind::NotFound)?;
        s.files.insert(to.to_path_buf(), f);
        if let Some(m) = s.modes.remove(from) {
            s.modes.insert(to.to_path_buf(), m);
        }
        *s.dir_syncs.entry(parent(to)?.to_path_buf()).or_default() += 1;
        Ok(())
    }

    fn remove(&mut self, path: &Path) -> io::Result<()> {
        self.check("remove")?;
        let mut s = self.state();
        s.files.remove(path).ok_or(io::ErrorKind::NotFound)?;
        s.modes.remove(path);
        *s.dir_syncs.entry(parent(path)?.to_path_buf()).or_default() += 1;
        Ok(())
    }
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;
