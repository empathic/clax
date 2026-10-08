//! `clax toolpath`: export the recorded history as Toolpath documents, and
//! show the journal's state (spec 2026-10-06-toolpath-audit-design §8.1).

use crate::client::Client;
use clax_core::Home;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Write the recorded history as a Toolpath document (stdout, or -o).
    ///
    /// With no selector, the whole install: one path per artifact plus the
    /// install path. Selectors of one kind union (--artifact and --live
    /// together are one kind); kinds intersect. --by-session keeps that
    /// session's steps and the owner's, viewers' and anonymous steps on
    /// its artifacts. --since is inclusive and --until exclusive; a bare date
    /// is 00:00 UTC. Nothing is redacted unless --no-text, --no-names or
    /// --no-paths says so.
    Export(ExportArgs),
    /// Show the journal's directory, segment, cursor, lag and last error.
    Status,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ShapeArg {
    /// One path per artifact, plus the install path.
    Artifacts,
    /// One linear audit-trail path of every selected step.
    Journal,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum FormatArg {
    /// One Toolpath Graph document.
    Json,
    /// One path as Toolpath JSONL: needs --shape journal or one artifact.
    Jsonl,
}

#[derive(clap::Args)]
pub struct ExportArgs {
    /// An artifact, by ID or URL (repeatable).
    #[arg(long = "artifact", value_name = "ID|URL")]
    pub artifacts: Vec<String>,
    /// A live page, by its page URL (repeatable).
    #[arg(long = "live", value_name = "PAGE URL")]
    pub live: Vec<String>,
    /// A Clax session ID or harness session ID (repeatable): that session's
    /// steps, plus the owner's, viewers' and anonymous steps on the
    /// artifacts it touched.
    #[arg(long = "by-session", value_name = "SESSION")]
    pub by_sessions: Vec<String>,
    /// Keep events at or after this time (RFC 3339, or YYYY-MM-DD).
    #[arg(long, value_name = "TIME")]
    pub since: Option<String>,
    /// Keep events before this time (RFC 3339, or YYYY-MM-DD).
    #[arg(long, value_name = "TIME")]
    pub until: Option<String>,
    #[arg(long, value_enum, default_value = "artifacts")]
    pub shape: ShapeArg,
    #[arg(long, value_enum, default_value = "json")]
    pub format: FormatArg,
    /// Replace free text (comments, notes, labels, titles, questions and
    /// answers, messages) with its hash.
    #[arg(long)]
    pub no_text: bool,
    /// Leave out viewers' display names (public IDs stay).
    #[arg(long)]
    pub no_names: bool,
    /// Replace local paths and URLs that can carry a query with their hash.
    #[arg(long)]
    pub no_paths: bool,
    /// Indent the JSON.
    #[arg(long)]
    pub pretty: bool,
    /// Write to this file instead of stdout (mode 0600). It must not exist:
    /// it is created exclusively and removed if the export fails or is
    /// interrupted (SIGINT, SIGTERM). With --force, an existing file is
    /// replaced: the export is written to a temporary file beside it, then
    /// renamed over it.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Replace the -o file when it exists (through a temporary file and a
    /// rename, so it is the old file or the whole new one).
    #[arg(long)]
    pub force: bool,
}

pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Export(a) => export(cli, home, a),
        Cmd::Status => status(cli, home),
    }
}

/// The export's query parameters (spec §8.3).
fn query(a: &ExportArgs) -> anyhow::Result<Vec<(&'static str, String)>> {
    let mut q = Vec::new();
    for t in &a.artifacts {
        let (id, _) = clax_mcp::tools::artifact_ref(t)
            .map_err(|_| anyhow::anyhow!("'{t}' is not an artifact ID or URL"))?;
        q.push(("artifact", id));
    }
    q.extend(a.live.iter().map(|u| ("live", u.clone())));
    q.extend(a.by_sessions.iter().map(|s| ("by_session", s.clone())));
    q.extend(a.since.iter().map(|s| ("since", s.clone())));
    q.extend(a.until.iter().map(|s| ("until", s.clone())));
    q.push((
        "shape",
        match a.shape {
            ShapeArg::Artifacts => "artifacts",
            ShapeArg::Journal => "journal",
        }
        .into(),
    ));
    q.push((
        "format",
        match a.format {
            FormatArg::Json => "json",
            FormatArg::Jsonl => "jsonl",
        }
        .into(),
    ));
    for (k, on) in [
        ("no_text", a.no_text),
        ("no_names", a.no_names),
        ("no_paths", a.no_paths),
        ("pretty", a.pretty),
    ] {
        q.push((k, on.to_string()));
    }
    Ok(q)
}

/// Refuses to replace `dest` without `force`.
fn may_write(dest: &Path, force: bool) -> anyhow::Result<()> {
    if dest.is_dir() {
        anyhow::bail!("{} is a directory", dest.display());
    }
    if !force && dest.symlink_metadata().is_ok() {
        anyhow::bail!("{} exists; pass --force to replace it", dest.display());
    }
    Ok(())
}

/// What a whole export ends with: a JSON document its closing brace (and
/// a newline when indented), a JSONL path its `PathClose` line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Json,
    Jsonl,
}

const PATH_CLOSE: &[u8] = b"{\"PathClose\":{}}\n";

/// A reader that follows what it passes on, so the end of an export can
/// be checked after it is copied: the last bytes, for JSONL, and, for JSON,
/// the nesting depth outside strings, so a stream cut short just after an
/// inner `}` is not taken for a whole document.
struct Tail<'a> {
    inner: &'a mut dyn Read,
    last: Vec<u8>,
    depth: u64,
    in_string: bool,
    escaped: bool,
    /// The top-level value has closed.
    closed: bool,
    /// Something other than whitespace came after it, or a closing bracket
    /// came with nothing open.
    junk: bool,
}

impl Tail<'_> {
    fn new(inner: &mut dyn Read) -> Tail<'_> {
        Tail {
            inner,
            last: Vec::new(),
            depth: 0,
            in_string: false,
            escaped: false,
            closed: false,
            junk: false,
        }
    }

    fn follow(&mut self, b: u8) {
        if self.in_string {
            match (self.escaped, b) {
                (true, _) => self.escaped = false,
                (false, b'\\') => self.escaped = true,
                (false, b'"') => self.in_string = false,
                _ => {}
            }
            return;
        }
        if self.closed {
            self.junk |= !b.is_ascii_whitespace();
            return;
        }
        match b {
            b'"' => self.in_string = true,
            b'{' | b'[' => self.depth += 1,
            b'}' | b']' => match self.depth {
                0 => self.junk = true,
                1 => {
                    self.depth = 0;
                    self.closed = true;
                }
                _ => self.depth -= 1,
            },
            _ => {}
        }
    }

    /// Fails unless what was read ends as a whole export does: the
    /// daemon aborts an export cut short, and this also catches one that
    /// ended without the abort reaching the client.
    fn check(&self, ending: Ending) -> anyhow::Result<()> {
        let whole = match ending {
            Ending::Jsonl => self.last.ends_with(PATH_CLOSE),
            Ending::Json => self.closed && !self.junk && !self.in_string,
        };
        if whole {
            Ok(())
        } else {
            anyhow::bail!("the export ended early")
        }
    }
}

impl Read for Tail<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        for &b in &buf[..n] {
            self.follow(b);
        }
        self.last.extend_from_slice(&buf[..n]);
        let keep = PATH_CLOSE.len();
        if self.last.len() > keep {
            self.last.drain(..self.last.len() - keep);
        }
        Ok(n)
    }
}

/// Copies `body` to `out`, failing unless it ends as a whole export does.
fn copy_whole(body: &mut dyn Read, out: &mut dyn Write, ending: Ending) -> anyhow::Result<u64> {
    let mut tail = Tail::new(body);
    let n = std::io::copy(&mut tail, out)?;
    tail.check(ending)?;
    Ok(n)
}

/// The file a signal handler removes if the export is interrupted: the
/// partial file being written.
static UNFINISHED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Installs the SIGINT and SIGTERM handlers (once): on either, the
/// partial file [`UNFINISHED`] names is removed and the process exits 130
/// or 143. They are installed before this returns, so a signal that comes
/// after it never takes the default action.
fn watch_interrupts() {
    use tokio::signal::unix::{SignalKind, signal};
    static HANDLER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    HANDLER.get_or_init(|| {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let signals = {
            let _in_rt = rt.enter();
            (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
            )
        };
        let (Ok(mut int), Ok(mut term)) = signals else {
            return;
        };
        std::thread::spawn(move || {
            let code = rt.block_on(async {
                tokio::select! {
                    _ = int.recv() => 130,
                    _ = term.recv() => 143,
                }
            });
            remove_unfinished();
            std::process::exit(code);
        });
    });
}

/// Creates the partial file `path` with `open`, marked for removal on an
/// interrupt. The mark's lock is held across the creation, so an interrupt
/// meanwhile waits and then removes the file made, never one that was
/// already there.
fn create_marked(
    path: &Path,
    open: impl FnOnce() -> std::io::Result<std::fs::File>,
) -> std::io::Result<std::fs::File> {
    watch_interrupts();
    let mut mark = UNFINISHED.lock().unwrap_or_else(|e| e.into_inner());
    let file = open()?;
    *mark = Some(path.to_path_buf());
    Ok(file)
}

/// Marks `path` as the partial file to remove on SIGINT or SIGTERM (none
/// with `None`).
fn unfinished(path: Option<&Path>) {
    *UNFINISHED.lock().unwrap_or_else(|e| e.into_inner()) = path.map(Path::to_path_buf);
}

/// Removes the partial file [`unfinished`] names, if any.
fn remove_unfinished() {
    if let Some(p) = UNFINISHED.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = std::fs::remove_file(p);
    }
}

fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Writes `body` to `dest`, which must not exist: `dest` is created
/// exclusively (mode 0600), so a file that appeared since the caller's
/// check is refused, never replaced. The export streams into it and is
/// synced; on any failure, an export that ends early included, `dest` is
/// removed. While it is written, the partial file is visible under `dest`;
/// SIGINT and SIGTERM remove it. Returns the bytes written.
pub fn create_exclusive(dest: &Path, body: &mut dyn Read, ending: Ending) -> anyhow::Result<u64> {
    let mut file = create_marked(dest, || open_private(dest)).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => {
            anyhow::anyhow!("{} exists; pass --force to replace it", dest.display())
        }
        _ => anyhow::anyhow!("creating {}: {e}", dest.display()),
    })?;
    let done = copy_whole(body, &mut file, ending).and_then(|n| {
        file.sync_all()?;
        Ok(n)
    });
    if done.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    unfinished(None);
    done
}

/// Writes `body` over `dest`: into `<dest>.tmp-<ULID>` beside it (created
/// new, mode 0600), synced, then renamed over `dest`, so `dest` is the old
/// file or the whole new one. On any failure the temporary file is removed
/// and `dest` is untouched; SIGINT and SIGTERM remove it too. Returns the
/// bytes written.
pub fn replace(dest: &Path, body: &mut dyn Read, ending: Ending) -> anyhow::Result<u64> {
    let name = dest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{} names no file", dest.display()))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".tmp-{}", clax_core::new_ulid()));
    let tmp = dest.with_file_name(tmp_name);
    let mut file = create_marked(&tmp, || open_private(&tmp))
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", tmp.display()))?;
    let done = copy_whole(body, &mut file, ending).and_then(|n| {
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, dest)?;
        Ok(n)
    });
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    unfinished(None);
    done
}

/// Writes `body` to `dest`: [`replace`] with `force`, else
/// [`create_exclusive`] (spec §8.1, ruling T10-3).
pub fn write_output(
    dest: &Path,
    force: bool,
    body: &mut dyn Read,
    ending: Ending,
) -> anyhow::Result<u64> {
    may_write(dest, force)?;
    if force {
        replace(dest, body, ending)
    } else {
        create_exclusive(dest, body, ending)
    }
}

fn export(cli: &crate::Cli, home: &Home, a: &ExportArgs) -> anyhow::Result<()> {
    let q = query(a)?;
    if let Some(dest) = &a.output {
        may_write(dest, a.force)?;
    }
    let ending = match a.format {
        FormatArg::Json => Ending::Json,
        FormatArg::Jsonl => Ending::Jsonl,
    };
    let c = Client::connect(home, cli.port_for(home)?)?;
    let mut res = c.get_stream("/api/toolpath/export", &q)?;
    match &a.output {
        Some(dest) => {
            let n = write_output(dest, a.force, &mut res, ending)
                .map_err(|e| anyhow::anyhow!("exporting to {}: {e:#}", dest.display()))?;
            if cli.json {
                println!("{}", json!({"file": dest, "bytes": n}));
            } else {
                eprintln!("wrote {} ({n} bytes)", dest.display());
            }
        }
        None => {
            let mut out = std::io::stdout().lock();
            copy_whole(&mut res, &mut out, ending)?;
            out.flush()?;
        }
    }
    Ok(())
}

fn status(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    let v = c.get("/api/toolpath/status")?;
    super::print(cli, v, status_text);
    Ok(())
}

/// `clax toolpath status`'s text: one `name  value` line per field, `-`
/// for a value there is none of.
fn status_text(v: &Value) -> String {
    let or_dash = |v: &Value| match v {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    [
        (
            "journal",
            if v["journal"] == true { "on" } else { "off" }.to_string(),
        ),
        ("directory", or_dash(&v["dir"])),
        ("segment", or_dash(&v["segment"])),
        (
            "cursor",
            format!(
                "{} of newest event {}",
                or_dash(&v["cursor"]),
                or_dash(&v["newest_seq"])
            ),
        ),
        (
            "lag",
            match v["lag_ms"].as_i64() {
                Some(ms) => format!("{ms} ms"),
                None => "-".into(),
            },
        ),
        ("last error", or_dash(&v["last_error"])),
    ]
    .iter()
    .map(|(k, v)| format!("{k:<11} {v}"))
    .collect::<Vec<_>>()
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body that yields `ok` bytes, then fails, as a dropped stream does.
    struct Broken(usize);

    impl Read for Broken {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::other("the stream ended early"));
            }
            let n = self.0.min(buf.len());
            buf[..n].fill(b'x');
            self.0 -= n;
            Ok(n)
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The tests that write exports share the one partial-file mark.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const DOC: &[u8] = b"{\"graph\":{}}\n";
    const NEW: &[u8] = b"{\"graph\":{\"id\":\"new\"}}";

    #[test]
    fn cli_export_writes_atomically() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("page.path.json");
        let read = |p: &Path| std::fs::read(p).unwrap();
        // A new file, private, with nothing left beside it.
        let n = write_output(&dest, false, &mut &DOC[..], Ending::Json).unwrap();
        assert_eq!(n, DOC.len() as u64);
        assert_eq!(read(&dest), DOC);
        assert_eq!(
            std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(entries(dir.path()), ["page.path.json"]);
        // An existing file is kept unless told, also when it appears after
        // the check: the file is created exclusively.
        let e = write_output(&dest, false, &mut &NEW[..], Ending::Json).unwrap_err();
        assert!(e.to_string().contains("--force"), "{e}");
        let e = create_exclusive(&dest, &mut &NEW[..], Ending::Json).unwrap_err();
        assert!(e.to_string().contains("--force"), "{e}");
        assert_eq!(read(&dest), DOC);
        // So is a dangling symlink, which is never followed.
        let link = dir.path().join("link.path.json");
        std::os::unix::fs::symlink(dir.path().join("nowhere"), &link).unwrap();
        assert!(write_output(&link, false, &mut &NEW[..], Ending::Json).is_err());
        assert!(create_exclusive(&link, &mut &NEW[..], Ending::Json).is_err());
        assert!(!dir.path().join("nowhere").exists());
        std::fs::remove_file(&link).unwrap();
        // Without --force, a stream that breaks partway, or ends early,
        // leaves no file at all.
        let fresh = dir.path().join("fresh.path.json");
        let e = write_output(&fresh, false, &mut Broken(100_000), Ending::Json).unwrap_err();
        assert!(e.to_string().contains("ended early"), "{e}");
        let e = write_output(&fresh, false, &mut &b"{\"graph\":{"[..], Ending::Json).unwrap_err();
        assert!(e.to_string().contains("ended early"), "{e}");
        let e = write_output(&fresh, false, &mut &DOC[..], Ending::Jsonl).unwrap_err();
        assert!(e.to_string().contains("ended early"), "{e}");
        // A stream cut short just after an inner `}`, or inside a string
        // that holds one, is not a whole document; braces in strings do not
        // count.
        for cut in [
            &b"{\"graph\":{\"id\":\"x\"}"[..],
            b"{\"graph\":{},\"t\":\"}",
            b"{\"t\":\"a\\\"}",
            b"{}}",
            b"{} {}",
        ] {
            let e = write_output(&fresh, false, &mut &cut[..], Ending::Json).unwrap_err();
            assert!(
                e.to_string().contains("ended early"),
                "{}",
                String::from_utf8_lossy(cut)
            );
        }
        let quoted = b"{\"t\":\"a}\\\"{\",\"u\":[1,{}]}\n";
        write_output(&fresh, false, &mut &quoted[..], Ending::Json).unwrap();
        std::fs::remove_file(&fresh).unwrap();
        assert!(e.to_string().contains("ended early"), "{e}");
        assert_eq!(entries(dir.path()), ["page.path.json"]);
        // With --force, they leave the old file whole and no temporary file.
        assert!(write_output(&dest, true, &mut Broken(100_000), Ending::Json).is_err());
        assert!(write_output(&dest, true, &mut &b"{\"gr"[..], Ending::Json).is_err());
        assert_eq!(read(&dest), DOC);
        assert_eq!(entries(dir.path()), ["page.path.json"]);
        // --force replaces it.
        write_output(&dest, true, &mut &NEW[..], Ending::Json).unwrap();
        assert_eq!(read(&dest), NEW);
        assert_eq!(entries(dir.path()), ["page.path.json"]);
        // A JSONL path ends with PathClose.
        let jsonl = b"{\"PathOpen\":{}}\n{\"PathClose\":{}}\n";
        let path = dir.path().join("one.path.jsonl");
        write_output(&path, false, &mut &jsonl[..], Ending::Jsonl).unwrap();
        // A directory is never a destination.
        assert!(write_output(dir.path(), true, &mut &DOC[..], Ending::Json).is_err());
    }

    #[test]
    fn an_interrupted_export_removes_its_partial_file() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("page.path.json");
        std::fs::write(&partial, b"{\"gra").unwrap();
        unfinished(Some(&partial));
        remove_unfinished();
        assert!(!partial.exists());
        // Once the export ends, nothing is left marked.
        std::fs::write(&partial, DOC).unwrap();
        unfinished(Some(&partial));
        unfinished(None);
        remove_unfinished();
        assert!(partial.exists());
    }

    #[test]
    fn status_text_is_one_line_per_field() {
        let v = json!({"journal": false, "dir": "/h/toolpath/journal", "segment": null,
                       "cursor": null, "newest_seq": 12, "lag_ms": null, "last_error": null});
        assert_eq!(
            status_text(&v),
            "journal     off\n\
             directory   /h/toolpath/journal\n\
             segment     -\n\
             cursor      - of newest event 12\n\
             lag         -\n\
             last error  -"
        );
    }
}
