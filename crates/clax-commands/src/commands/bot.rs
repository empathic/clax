//! `clax bot surface`: what the author of a ClaxBot surface checks a
//! definition with (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
//! "Tooling for hosts"). `check` reads a surface file in either notation
//! and prints what a model would read of it, or what is wrong and where;
//! `convert` writes the definition in the other notation.
//!
//! Both read one file and write text. Neither reads the Clax home,
//! starts or reaches a daemon, or uses the network.

use clax_surface::{Author, Definition, Limits};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Check a ClaxBot surface definition, or write it in the other
    /// notation.
    ///
    /// A surface is what a host gives ClaxBot's runs: operations, their
    /// types and request kinds. It is written as a `.d.ts` file, for a
    /// person, or as a JSON manifest, for a program; both say the same.
    #[command(subcommand)]
    Surface(SurfaceCmd),
}

#[derive(clap::Subcommand)]
pub enum SurfaceCmd {
    /// Read a surface file and print what a model would read of it.
    ///
    /// The file is a `.d.ts` file or a `.json` manifest, by its name, and
    /// is read as a host's: it may not take one of Clax's own names or
    /// charge. A file that is not a surface definition is refused with
    /// what is wrong and where, a mistake a line on stderr, as
    /// `error: <file>:<line>: …` or, for a value of a manifest,
    /// `error: <file>: <JSON Pointer>: …`, and the command exits with 1.
    /// A manifest's mistakes are said in its order, the first of each
    /// operation, interface and kind, 64 at the most; a last line then
    /// says how many entries were not looked at. A `.d.ts` file's first
    /// mistake is said.
    ///
    /// A file that reads may still get notes on stderr (`note: <file>:
    /// …`), which change nothing of the exit status, 0: for each allow
    /// pattern of a kind that matches no operation of the surface or of
    /// Clax's own. It will match only another surface's, or it was
    /// written without the surface's name.
    ///
    /// With --json, stdout has one object: `{"name", "notation",
    /// "operations": [{"name", "access", "waits", "once", "throws"}],
    /// "kinds": [{"name", "allow": [pattern] or null}], "notes": [text],
    /// "text"}`, or, for a file that is refused, `{"file", "errors":
    /// [{"line" or "pointer", "message"}]}` (a mistake about the file as
    /// a whole has neither; the last has `"stopped": true` and
    /// `"unread": <entries>` when the reader stopped there) and nothing
    /// on stderr, with the same exit status.
    ///
    /// Reads no Clax home and starts no daemon.
    Check(CheckArgs),
    /// Write a surface definition in the other notation: a `.d.ts` file
    /// as a JSON manifest, a manifest as a `.d.ts` file.
    ///
    /// The definition is checked as `check` checks it, and written as
    /// Clax writes one: the same text for the same definition, whichever
    /// notation it was read from. A file's `//` comments and its layout
    /// are not part of its definition and are not written. Nothing is
    /// written when the file is refused, when what would be written is
    /// more than a reader of it takes, or over the file being read.
    /// Refusals are said as `check` says them, with --json too.
    ///
    /// Reads no Clax home and starts no daemon.
    Convert(ConvertArgs),
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// The surface file: `<name>.d.ts`, or a manifest, `<name>.json`.
    pub file: PathBuf,
    /// Print the definition as a `.d.ts` file instead, as Clax writes
    /// one. To type-check that file (`tsc --noEmit`), declare beside it
    /// the core's types it may name, which no file of Clax's declares
    /// yet: `type Int = number; type Instruction = string;` and
    /// `Content` and `Image` as your host is given them.
    #[arg(long, conflicts_with = "ops")]
    pub dts: bool,
    /// Print the operations as a table instead: each one's name as a
    /// script calls it, its access, whether it waits, whether it runs
    /// once, and its error codes; then the kinds, each with the
    /// patterns it allows.
    #[arg(long)]
    pub ops: bool,
    /// Read the file as Clax's own surface, which may be named `clax`
    /// and may charge (`@spend`).
    #[arg(long)]
    pub own: bool,
}

#[derive(clap::Args)]
pub struct ConvertArgs {
    /// The surface file: `<name>.d.ts`, or a manifest, `<name>.json`.
    pub file: PathBuf,
    /// The notation to write. Default: the one the file is not in.
    #[arg(long, value_enum)]
    pub to: Option<Notation>,
    /// Write to this file instead of standard output. It is not the
    /// file being read.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Replace the output file when it exists.
    #[arg(long, requires = "output")]
    pub force: bool,
    /// Read the file as Clax's own surface, which may be named `clax`
    /// and may charge (`@spend`).
    #[arg(long)]
    pub own: bool,
}

/// One of the two notations of a surface definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Notation {
    /// A `.d.ts` file.
    Dts,
    /// A JSON manifest.
    Json,
}

/// One thing wrong with a file: where, when it is at a line of the file
/// or at a value of a manifest, and what.
#[derive(Debug, PartialEq, Eq)]
struct Mistake {
    line: Option<usize>,
    pointer: Option<String>,
    message: String,
    /// For the reader's last word, that it stopped: how many entries it
    /// did not look at. It is no mistake of the file's.
    unread: Option<usize>,
}

/// Why a command did nothing: the file it is about, and what is wrong
/// with it, one mistake at least.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    file: PathBuf,
    mistakes: Vec<Mistake>,
}

impl Refusal {
    /// A refusal about `file` as a whole: `message` says all of it,
    /// the file's name too.
    fn of(file: &Path, message: impl Into<String>) -> Refusal {
        Refusal {
            file: file.to_path_buf(),
            mistakes: vec![Mistake {
                line: None,
                pointer: None,
                message: message.into(),
                unread: None,
            }],
        }
    }

    /// The mistakes a reader of `file` found.
    fn read(file: &Path, errors: clax_surface::Errors) -> Refusal {
        let unread = Some(errors.unread()).filter(|n| *n > 0);
        let mut mistakes: Vec<Mistake> = errors
            .into_vec()
            .into_iter()
            .map(|e| Mistake {
                line: e.line(),
                // A manifest as a whole is at the empty pointer.
                pointer: e.pointer().filter(|p| !p.is_empty()).map(str::to_string),
                message: e.message,
                unread: None,
            })
            .collect();
        // A reader that stopped says so last.
        if let Some(last) = mistakes.last_mut() {
            last.unread = unread;
        }
        Refusal {
            file: file.to_path_buf(),
            mistakes,
        }
    }

    /// The refusal as lines for stderr: `error: <file>:<line>: …` for a
    /// mistake at a line, `error: <file>: <pointer>: …` for one at a
    /// value of a manifest, `error: <file>: …` for the reader's word
    /// that it stopped, else `error: …`.
    fn lines(&self) -> String {
        let file = self.file.display();
        let mut out = String::new();
        for m in &self.mistakes {
            let _ = match (m.line, &m.pointer, m.unread) {
                (Some(line), ..) => writeln!(out, "error: {file}:{line}: {}", m.message),
                (None, Some(pointer), _) => {
                    writeln!(out, "error: {file}: {pointer}: {}", m.message)
                }
                (None, None, Some(_)) => writeln!(out, "error: {file}: {}", m.message),
                (None, None, None) => writeln!(out, "error: {}", m.message),
            };
        }
        out
    }

    /// The refusal as `--json` gives it.
    fn json(&self) -> serde_json::Value {
        let errors: Vec<serde_json::Value> = self
            .mistakes
            .iter()
            .map(|m| {
                let mut error = serde_json::json!({"message": m.message});
                if let Some(line) = m.line {
                    error["line"] = line.into();
                }
                if let Some(pointer) = &m.pointer {
                    error["pointer"] = pointer.as_str().into();
                }
                if let Some(unread) = m.unread {
                    error["stopped"] = true.into();
                    error["unread"] = unread.into();
                }
                error
            })
            .collect();
        serde_json::json!({"file": self.file, "errors": errors})
    }
}

impl Notation {
    /// The notation of the file at `path`, which its name says.
    fn of(path: &Path) -> Result<Notation, Refusal> {
        let name = path.file_name().map(|n| n.to_string_lossy());
        match name.as_deref() {
            Some(n) if n.ends_with(".d.ts") => Ok(Notation::Dts),
            Some(n) if n.ends_with(".json") => Ok(Notation::Json),
            _ => Err(Refusal::of(
                path,
                format!(
                    "{}: a surface file is named `<name>.d.ts`, or `<name>.json` for a manifest",
                    path.display()
                ),
            )),
        }
    }

    fn other(self) -> Notation {
        match self {
            Notation::Dts => Notation::Json,
            Notation::Json => Notation::Dts,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Notation::Dts => "dts",
            Notation::Json => "json",
        }
    }

    /// The most bytes a file in this notation may be.
    fn bytes(self, limits: &Limits) -> usize {
        match self {
            Notation::Dts => limits.file_bytes,
            Notation::Json => limits.manifest_bytes,
        }
    }
}

/// Reads the surface file at `path` as a host's, or as Clax's own, and
/// says which notation it is in.
///
/// # Errors
/// That the file cannot be read, is not named as a surface file, is
/// larger than a file of its notation may be (before it is read), or is
/// not a surface definition, with each mistake a reader found.
fn read(path: &Path, own: bool) -> Result<(Definition, Notation), Refusal> {
    let notation = Notation::of(path)?;
    let limits = Limits::default();
    let unread = |e: std::io::Error| Refusal::of(path, format!("{}: {e}", path.display()));
    let size = std::fs::metadata(path).map_err(unread)?.len();
    if size > notation.bytes(&limits) as u64 {
        return Err(Refusal::of(
            path,
            format!(
                "{}: the file is {size} bytes long, and the limit is {}",
                path.display(),
                notation.bytes(&limits)
            ),
        ));
    }
    let text = std::fs::read_to_string(path).map_err(unread)?;
    let reserved;
    let author = if own {
        Author::Clax
    } else {
        reserved = crate::reserved_names();
        Author::Host {
            reserved: &reserved,
        }
    };
    let definition = match notation {
        Notation::Dts => clax_surface::parse(&text, author, &limits).map_err(Into::into),
        Notation::Json => clax_surface::parse_manifest(&text, author, &limits),
    }
    .map_err(|errors| Refusal::read(path, errors))?;
    Ok((definition, notation))
}

/// The definition as text in `notation`, when a reader would take it.
fn written(def: &Definition, notation: Notation) -> Result<String, String> {
    let limits = Limits::default();
    match notation {
        Notation::Dts => clax_surface::print_within(def, &limits),
        Notation::Json => clax_surface::print_manifest_within(def, &limits),
    }
}

/// The operations as a table: a heading, then a row for each, its
/// columns as wide as their longest entry.
fn table(def: &Definition) -> String {
    let yes = |flag: bool| if flag { "yes" } else { "-" };
    let mut rows = vec![[
        "OPERATION".to_string(),
        "ACCESS".into(),
        "WAITS".into(),
        "ONCE".into(),
        "ERRORS".into(),
    ]];
    rows.extend(def.operations().map(|o| {
        [
            o.name.to_string(),
            o.op.access.tag().into(),
            yes(o.op.waits).into(),
            yes(o.op.once).into(),
            if o.op.throws.is_empty() {
                "-".into()
            } else {
                o.op.throws.join(", ")
            },
        ]
    }));
    let width = |k: usize| rows.iter().map(|r| r[k].chars().count()).max().unwrap_or(0);
    let widths = [width(0), width(1), width(2), width(3)];
    let mut out = String::new();
    for row in &rows {
        for (cell, width) in row.iter().zip(widths) {
            let _ = write!(out, "{cell:width$}  ");
        }
        let _ = writeln!(out, "{}", row[4]);
    }
    out
}

/// The definition's operations as `check --json` gives them.
fn operations_json(def: &Definition) -> Vec<serde_json::Value> {
    def.operations()
        .map(|o| {
            serde_json::json!({
                "name": o.name,
                "access": o.op.access.tag(),
                "waits": o.op.waits,
                "once": o.op.once,
                "throws": o.op.throws,
            })
        })
        .collect()
}

/// The kinds as a table: each one's name and what it allows.
fn kinds_table(def: &Definition) -> String {
    let width = def
        .kinds()
        .iter()
        .map(|k| k.name.chars().count())
        .max()
        .unwrap_or(0)
        .max("KIND".len());
    let mut out = format!("{:width$}  ALLOWS\n", "KIND");
    for k in def.kinds() {
        let allows = match &k.allow {
            Some(patterns) if patterns.is_empty() => "(no operation)".to_string(),
            Some(patterns) => patterns
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            None => "(every operation)".to_string(),
        };
        let _ = writeln!(out, "{:width$}  {allows}", k.name);
    }
    out
}

/// What an author should look at again though the definition reads:
/// each allow pattern of a kind that names no operation of the surface
/// itself, nor of `clax`, Clax's own, when it is known. A kind may
/// allow another surface's operations, which this file does not hold,
/// so such a pattern is no mistake; one written without the surface's
/// name is, and looks the same.
fn notes(def: &Definition, clax: Option<&Definition>) -> Vec<String> {
    let mut known = vec![def];
    known.extend(clax);
    let mut out = Vec::new();
    for kind in def.kinds() {
        for pattern in kind.allow.iter().flatten() {
            if pattern.names_any(&known) {
                continue;
            }
            let of = match clax {
                Some(_) => format!("`{}` or of Clax's own surface", def.name()),
                None => format!("`{}`", def.name()),
            };
            let mut note = format!(
                "kind `{}` allows `{pattern}`, which matches no operation of {of}; it will match only another surface's",
                kind.name
            );
            // The slip this is for: the surface's name left out.
            let with_surface = format!("{}.{pattern}", def.name());
            if clax_surface::Pattern::parse(&with_surface).is_ok_and(|p| p.names_any(&[def])) {
                let _ = write!(
                    note,
                    " (an allow pattern names the surface too: `{with_surface}`)"
                );
            }
            out.push(note);
        }
    }
    out
}

fn check(cli: &crate::Cli, a: &CheckArgs) -> Result<(), Refusal> {
    let (def, notation) = read(&a.file, a.own)?;
    let text = if a.dts {
        clax_surface::print(&def)
    } else if a.ops {
        let mut tables = table(&def);
        if !def.kinds().is_empty() {
            tables.push('\n');
            tables.push_str(&kinds_table(&def));
        }
        tables
    } else {
        clax_surface::declaration(&[&def], &|_| true)
            .map_err(|e| Refusal::of(&a.file, format!("{}: {e}", a.file.display())))?
    };
    // Clax's own surface is beside a host's; its own file stands alone.
    let clax = crate::bot_surface().filter(|_| !a.own);
    let notes = notes(&def, clax);
    let kinds: Vec<serde_json::Value> = def
        .kinds()
        .iter()
        .map(|k| {
            let allow = k
                .allow
                .as_ref()
                .map(|patterns| patterns.iter().map(ToString::to_string).collect::<Vec<_>>());
            serde_json::json!({"name": k.name, "allow": allow})
        })
        .collect();
    super::print(
        cli,
        serde_json::json!({
            "name": def.name(),
            "notation": notation.name(),
            "operations": operations_json(&def),
            "kinds": kinds,
            "notes": notes,
            "text": text,
        }),
        |_| text.trim_end_matches('\n').to_string(),
    );
    if !cli.json {
        for note in &notes {
            eprintln!("note: {}: {note}", a.file.display());
        }
    }
    Ok(())
}

/// Refuses to write `dest` when it is `source`, the file being read, or
/// when it exists and `force` is not given.
fn may_write(dest: &Path, source: &Path, force: bool) -> Result<(), Refusal> {
    if dest.is_dir() {
        return Err(Refusal::of(
            dest,
            format!("{} is a directory", dest.display()),
        ));
    }
    let same = match (std::fs::canonicalize(dest), std::fs::canonicalize(source)) {
        (Ok(dest), Ok(source)) => dest == source,
        _ => false,
    };
    if same {
        return Err(Refusal::of(
            dest,
            format!(
                "{} is the file being read: written again, a surface file loses its `//` comments and its layout, which are not part of a definition; name another file",
                dest.display()
            ),
        ));
    }
    if !force && dest.symlink_metadata().is_ok() {
        return Err(Refusal::of(
            dest,
            format!("{} exists; pass --force to replace it", dest.display()),
        ));
    }
    Ok(())
}

/// Writes `text` to `dest` whole or not at all: to a new file beside it,
/// which then takes its name.
fn write_whole(dest: &Path, text: &str) -> Result<(), Refusal> {
    let Some(name) = dest.file_name() else {
        return Err(Refusal::of(
            dest,
            format!("{} names no file", dest.display()),
        ));
    };
    let mut name = name.to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let tmp = dest.with_file_name(name);
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, dest)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Refusal::of(dest, format!("{}: {e}", dest.display()))
    })
}

fn convert(cli: &crate::Cli, a: &ConvertArgs) -> Result<(), Refusal> {
    if let Some(dest) = &a.output {
        may_write(dest, &a.file, a.force)?;
    }
    let (def, from) = read(&a.file, a.own)?;
    let to = a.to.unwrap_or(from.other());
    let text = written(&def, to).map_err(|e| {
        Refusal::of(
            &a.file,
            format!("{}: {e}; nothing was written", a.file.display()),
        )
    })?;
    match &a.output {
        Some(dest) => {
            write_whole(dest, &text)?;
            super::print(
                cli,
                serde_json::json!({"name": def.name(), "notation": to.name(), "wrote": dest}),
                |_| format!("wrote {}", dest.display()),
            );
        }
        None => super::print(
            cli,
            serde_json::json!({"name": def.name(), "notation": to.name(), "text": text}),
            |_| text.trim_end_matches('\n').to_string(),
        ),
    }
    Ok(())
}

/// Runs the command. One that is refused says why itself, on stderr or,
/// with `--json`, as an object on stdout, and ends the process with 1.
pub fn run(cli: &crate::Cli, cmd: &Cmd) -> anyhow::Result<()> {
    let done = match cmd {
        Cmd::Surface(SurfaceCmd::Check(a)) => check(cli, a),
        Cmd::Surface(SurfaceCmd::Convert(a)) => convert(cli, a),
    };
    if let Err(refusal) = done {
        if cli.json {
            println!("{}", refusal.json());
        } else {
            eprint!("{}", refusal.lines());
        }
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_surface::Reserved;

    const DEMO: &str = "declare const demo: {\n  /** Reads. @read */\n  get(key: string): Promise<string | null>;\n  jobs: {\n    /** Starts one. @effect @waits @once @throws busy @throws not_found */\n    start(name: string): Promise<null>;\n  };\n};\n";

    fn demo() -> Definition {
        let reserved = Reserved::of(&[]);
        let author = Author::Host {
            reserved: &reserved,
        };
        clax_surface::parse(DEMO, author, &Limits::default()).unwrap()
    }

    #[test]
    fn a_files_name_says_its_notation() {
        for (name, notation) in [
            ("shell.d.ts", Notation::Dts),
            ("dir.json/shell.d.ts", Notation::Dts),
            ("shell.json", Notation::Json),
            ("/tmp/a.b.json", Notation::Json),
        ] {
            assert_eq!(Notation::of(Path::new(name)).unwrap(), notation, "{name}");
        }
        for name in ["shell.ts", "shell", "shell.d.ts.bak", "shell.JSON"] {
            let e = Notation::of(Path::new(name)).unwrap_err();
            assert_eq!(
                e.lines(),
                format!(
                    "error: {name}: a surface file is named `<name>.d.ts`, or `<name>.json` for a manifest\n"
                )
            );
        }
        assert_eq!(Notation::Dts.other(), Notation::Json);
        assert_eq!(Notation::Json.other(), Notation::Dts);
    }

    #[test]
    fn the_table_lists_each_operation_as_it_was_understood() {
        assert_eq!(
            table(&demo()),
            "OPERATION        ACCESS  WAITS  ONCE  ERRORS\n\
             demo.get         read    -      -     -\n\
             demo.jobs.start  effect  yes    yes   busy, not_found\n"
        );
    }

    #[test]
    fn each_notation_is_written_when_a_reader_would_take_it() {
        let def = demo();
        assert_eq!(written(&def, Notation::Dts).unwrap(), DEMO);
        let manifest = written(&def, Notation::Json).unwrap();
        assert!(manifest.starts_with("{\n  \"clax_surface\": 1,\n  \"name\": \"demo\","));
        let reserved = Reserved::of(&[]);
        let author = Author::Host {
            reserved: &reserved,
        };
        assert_eq!(
            clax_surface::parse_manifest(&manifest, author, &Limits::default()).unwrap(),
            def
        );
    }

    #[test]
    fn an_existing_file_is_replaced_only_with_force_and_never_the_one_read() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("in.d.ts");
        std::fs::write(&source, DEMO).unwrap();
        let dest = dir.path().join("out.json");
        may_write(&dest, &source, false).unwrap();
        write_whole(&dest, "one\n").unwrap();
        let e = may_write(&dest, &source, false).unwrap_err().lines();
        assert!(
            e.starts_with("error: ")
                && e.ends_with("out.json exists; pass --force to replace it\n"),
            "{e}"
        );
        may_write(&dest, &source, true).unwrap();
        write_whole(&dest, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "two\n");
        let e = may_write(dir.path(), &source, true).unwrap_err().lines();
        assert!(e.ends_with("is a directory\n"), "{e}");
        // The file being read is not written over, by any name it has.
        for same in [
            source.clone(),
            dir.path()
                .join("../")
                .join(dir.path().file_name().unwrap())
                .join("in.d.ts"),
        ] {
            let e = may_write(&same, &source, true).unwrap_err().lines();
            assert!(e.contains("in.d.ts is the file being read"), "{e}");
        }
        assert_eq!(std::fs::read_to_string(&source).unwrap(), DEMO);
        // Nothing but the files is left beside them.
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["in.d.ts", "out.json"]);
    }

    #[test]
    fn a_reader_that_stopped_says_so_last_and_json_marks_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("many.json");
        let entries = vec!["7"; 100].join(",");
        std::fs::write(
            &file,
            format!(r#"{{"clax_surface": 1, "name": "demo", "operations": [{entries}]}}"#),
        )
        .unwrap();
        let refusal = read(&file, false).map(|_| ()).unwrap_err();
        assert_eq!(refusal.mistakes.len(), 65);
        let lines = refusal.lines();
        assert_eq!(lines.lines().count(), 65);
        assert!(lines.len() < 16 * 1024, "{}", lines.len());
        assert_eq!(
            lines.lines().last().unwrap(),
            format!(
                "error: {}: 64 mistakes are said, and the 36 entries of the manifest after them were not looked at: put these right, and read it again",
                file.display()
            )
        );
        let json = refusal.json();
        let errors = json["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 65);
        assert!(errors[63].get("stopped").is_none());
        assert_eq!(errors[64]["stopped"], true);
        assert_eq!(errors[64]["unread"], 36);
        assert!(errors[64].get("pointer").is_none());
    }

    #[test]
    fn a_pattern_that_names_no_operation_known_is_noted() {
        let source = "declare const shell: {\n  /** D. @read */\n  windows(): Promise<null>;\n  settings: {\n    /** D. @write */\n    set(): Promise<null>;\n  };\n};\ninterface Kinds {\n  /** K. @allow windows settings.* shell.windows shell.settings.* files.read * */\n  slip(request: null): {};\n  /** K. @allow */\n  none(request: null): {};\n}\n";
        let reserved = Reserved::of(&[]);
        let author = Author::Host {
            reserved: &reserved,
        };
        let def = clax_surface::parse(source, author, &Limits::default()).unwrap();
        assert_eq!(
            notes(&def, None),
            [
                "kind `slip` allows `windows`, which matches no operation of `shell`; it will match only another surface's (an allow pattern names the surface too: `shell.windows`)",
                "kind `slip` allows `settings.*`, which matches no operation of `shell`; it will match only another surface's (an allow pattern names the surface too: `shell.settings.*`)",
                "kind `slip` allows `files.read`, which matches no operation of `shell`; it will match only another surface's",
            ]
        );
        // Beside Clax's own surface, what names an operation of it is
        // not noted.
        let clax = clax_surface::parse(
            "declare const clax: {\n  files: {\n    /** D. @read */\n    read(): Promise<null>;\n  };\n};",
            Author::Clax,
            &Limits::default(),
        )
        .unwrap();
        let beside = notes(&def, Some(&clax));
        assert_eq!(beside.len(), 2);
        assert_eq!(
            beside[0],
            "kind `slip` allows `windows`, which matches no operation of `shell` or of Clax's own surface; it will match only another surface's (an allow pattern names the surface too: `shell.windows`)"
        );
        assert_eq!(
            kinds_table(&def),
            "KIND  ALLOWS\n\
             slip  windows settings.* shell.windows shell.settings.* files.read *\n\
             none  (no operation)\n"
        );
        assert!(notes(&demo(), None).is_empty());
    }

    #[test]
    fn a_refusal_says_each_mistake_with_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.json");
        std::fs::write(
            &file,
            r#"{"clax_surface": 1, "name": "demo", "operations": [
  {"name": "a", "description": "D.", "access": "get", "result": {}},
  {"name": "b", "description": "D.", "access": "read", "result": {"type": "text"}}]}"#,
        )
        .unwrap();
        let refusal = read(&file, false).map(|_| ()).unwrap_err();
        let name = file.display();
        assert_eq!(
            refusal.lines(),
            format!(
                "error: {name}: /operations/0/access: `get` is not an access: an operation's `access` is one of \"read\", \"write\", \"effect\" and \"spend\"\n\
                 error: {name}: /operations/1/result/type: `text` is not a `type` of the format, which has \"string\", \"number\", \"integer\", \"boolean\", \"null\", \"array\" and \"object\"\n"
            )
        );
        let json = refusal.json();
        assert_eq!(json["file"], serde_json::json!(file));
        assert_eq!(json["errors"].as_array().unwrap().len(), 2);
        assert_eq!(json["errors"][0]["pointer"], "/operations/0/access");
        assert!(json["errors"][0].get("line").is_none());
        // A line of a file is said as compilers say one.
        let file = dir.path().join("bad.d.ts");
        std::fs::write(&file, "declare const demo: {\n  x(): Promise<null>;\n};\n").unwrap();
        let refusal = read(&file, false).map(|_| ()).unwrap_err();
        assert!(
            refusal.lines().starts_with(&format!(
                "error: {}:2: operation `x` has no documentation",
                file.display()
            )),
            "{}",
            refusal.lines()
        );
        assert_eq!(
            refusal.json()["errors"][0],
            serde_json::json!({"line": 2, "message": refusal.mistakes[0].message})
        );
        // What is wrong with a file as a whole has no place.
        let refusal = read(&dir.path().join("none.json"), false)
            .map(|_| ())
            .unwrap_err();
        assert!(refusal.lines().starts_with(&format!(
            "error: {}: ",
            dir.path().join("none.json").display()
        )));
        let error = &refusal.json()["errors"][0];
        assert!(error.get("line").is_none() && error.get("pointer").is_none());
    }
}
