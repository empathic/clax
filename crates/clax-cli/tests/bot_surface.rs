//! `clax bot surface check` and `convert`, on the design's two example
//! surfaces of a host's and on Clax's own: each reads a file and writes
//! text, with no Clax home and no daemon.
use assert_cmd::Command;
use predicates::prelude::*;

const SHELL: &str =
    include_str!("../../../docs/superpowers/specs/2026-11-10-claxbot-surfaces/shell.d.ts");
const REVIEW: &str =
    include_str!("../../../docs/superpowers/specs/2026-11-10-claxbot-surfaces/review.d.ts");
const CLAX: &str = include_str!("../../clax-bot/src/clax.d.ts");

/// `clax` with no environment at all: neither `HOME` nor `CLAX_HOME`
/// names a home, so nothing the command does can be in one.
fn clax() -> Command {
    let mut c = Command::new(crate::common::clax());
    c.env_clear();
    c
}

/// A scratch directory holding the three surface files.
fn files() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("shell.d.ts", SHELL),
        ("review.d.ts", REVIEW),
        ("clax.d.ts", CLAX),
    ] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    dir
}

/// What a `clax bot surface` command that succeeds prints.
#[track_caller]
fn printed(args: &[&str], dir: &std::path::Path) -> String {
    let out = clax()
        .current_dir(dir)
        .args(["bot", "surface"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Nothing is said on stderr but notes, which are no refusal.
    let said = String::from_utf8(out.stderr).unwrap();
    assert!(
        said.lines().all(|l| l.starts_with("note: ")),
        "{args:?}: {said}"
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A `clax bot surface` command is refused: it exits with 1, prints
/// nothing on stdout, and says `message` after `error: `.
#[track_caller]
fn refused(args: &[&str], dir: &std::path::Path, message: &str) {
    let out = clax()
        .current_dir(dir)
        .args(["bot", "surface"])
        .args(args)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{args:?}");
    assert!(out.stdout.is_empty(), "{args:?}");
    let said = String::from_utf8(out.stderr).unwrap();
    assert!(
        said.starts_with("error: ") && said.contains(message),
        "{args:?}: `{message}` is not in: {said}"
    );
}

#[test]
fn check_prints_what_a_model_would_read() {
    let dir = files();
    let shell = printed(&["check", "shell.d.ts"], dir.path());
    assert!(
        shell.starts_with("declare namespace shell {\n  interface Part {\n    /** `bar`, `dock`, `launcher`, … */\n    name: string;\n"),
        "{shell}"
    );
    assert!(shell.contains(
        "\ndeclare const clax: {\n  shell: {\n    /** Which artifact fills each part of the shell, and which is being worked on. */\n    parts(): Promise<shell.Part[]>;\n"
    ));
    assert!(shell.contains(
        "    /** Asks the person in the Comment box, and waits for their answer. (writes) (may take long) Errors: declined. */\n    ask(question: string, choices?: string[]): Promise<string>;\n  };\n};\n"
    ));
    // Kinds and their types are not the model's to read.
    assert!(!shell.contains("triage") && !shell.contains("Touched"));
    let review = printed(&["check", "review.d.ts"], dir.path());
    assert!(review.starts_with("declare namespace review {\n  interface Page {\n"));
    assert!(
        review.contains(
            "    view(): Promise<review.PullView | review.CompareView | review.RefView>;\n"
        )
    );
    // Clax's own surface is not a host's: its name and its namespaces
    // are the ones a host may not take.
    refused(&["check", "clax.d.ts"], dir.path(), "clax.d.ts:");
    refused(
        &["check", "clax.d.ts"],
        dir.path(),
        "`clax` is a reserved name: a host's surface may not be named `clax` or as one of Clax's own namespaces (artifact, artifacts, clax, content, db, draft, files, runs, threads, web)",
    );
    let own = printed(&["check", "--own", "clax.d.ts"], dir.path());
    assert!(
        own.contains("\ndeclare const clax: {\n  artifact: {\n"),
        "{own}"
    );
}

#[test]
fn check_prints_the_file_again_or_its_operations() {
    let dir = files();
    let dts = printed(&["check", "--dts", "shell.d.ts"], dir.path());
    assert!(dts.starts_with("/**\n * You are working on a ClaxShell desktop."));
    assert!(dts.contains("\n  /** A part or a window as it draws now. @read @throws not_found */\n  screenshot(of: { part: string } | { window: string }): Promise<Image>;\n"));
    assert!(dts.ends_with(
        "interface TriageResult {\n  asked: { artifact: string; run: string; outcome: string }[];\n  summary: string;\n}\n\ninterface Kinds {\n  /**\n   * The request is a comment the person drew across several apps. Decide\n   * what each app it touches should be asked, and ask each with\n   * `clax.threads.open`. Wait for those runs with `clax.runs.wait`, then\n   * finish with what each did. Change no artifact yourself.\n   * @allow shell.parts shell.windows shell.settings.get shell.screenshot shell.ask threads.open runs.wait\n   */\n  triage(request: Triage): TriageResult;\n}\n"
    ));
    // Written again, it is the same file.
    std::fs::write(dir.path().join("again.d.ts"), &dts).unwrap();
    assert_eq!(printed(&["check", "--dts", "again.d.ts"], dir.path()), dts);
    assert_eq!(
        printed(&["check", "--ops", "shell.d.ts"], dir.path()),
        "OPERATION           ACCESS  WAITS  ONCE  ERRORS\n\
         shell.parts         read    -      -     -\n\
         shell.windows       read    -      -     -\n\
         shell.workspaces    read    -      -     -\n\
         shell.bridge        read    -      -     -\n\
         shell.screenshot    read    -      -     not_found\n\
         shell.settings.get  read    -      -     -\n\
         shell.settings.set  write   -      -     invalid\n\
         shell.window.open   effect  -      -     not_found\n\
         shell.notify        effect  -      -     -\n\
         shell.ask           effect  yes    yes   declined\n\
         \n\
         KIND    ALLOWS\n\
         triage  shell.parts shell.windows shell.settings.get shell.screenshot shell.ask threads.open runs.wait\n"
    );
    let ops = printed(&["check", "--ops", "review.d.ts"], dir.path());
    assert!(
        ops.contains("\nreview.files       read    -      -     forbidden, off_track\n"),
        "{ops}"
    );
    clax()
        .current_dir(dir.path())
        .args(["bot", "surface", "check", "--ops", "--dts", "shell.d.ts"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
    let v: serde_json::Value =
        serde_json::from_str(&printed(&["check", "--json", "shell.d.ts"], dir.path())).unwrap();
    assert_eq!(v["name"], "shell");
    assert_eq!(v["notation"], "dts");
    assert_eq!(
        v["kinds"],
        serde_json::json!([{"name": "triage", "allow": ["shell.parts", "shell.windows", "shell.settings.get", "shell.screenshot", "shell.ask", "threads.open", "runs.wait"]}])
    );
    // Neither is an operation of `shell` or of Clax's surface as it is:
    // `runs` is the core's, which no file here holds.
    let other = |pattern: &str| {
        format!(
            "kind `triage` allows `{pattern}`, which matches no operation of `shell` or of Clax's own surface; it will match only another surface's"
        )
    };
    assert_eq!(
        v["notes"],
        serde_json::json!([other("threads.open"), other("runs.wait")])
    );
    assert_eq!(
        v["operations"][9],
        serde_json::json!({"name": "shell.ask", "access": "effect", "waits": true, "once": true, "throws": ["declined"]})
    );
    assert_eq!(
        v["text"].as_str().unwrap(),
        printed(&["check", "shell.d.ts"], dir.path())
    );
}

/// `first`, then `rest`.
fn both<'a>(first: &[&'a str], rest: &[&'a str]) -> Vec<&'a str> {
    [first, rest].concat()
}

#[test]
fn convert_writes_the_other_notation_and_back() {
    let dir = files();
    for (name, own) in [("shell", false), ("review", false), ("clax", true)] {
        let own: &[&str] = if own { &["--own"] } else { &[] };
        let file = format!("{name}.d.ts");
        let canonical = printed(&both(&["check", "--dts", &file], own), dir.path());
        let manifest = printed(&both(&["convert", &file], own), dir.path());
        assert!(
            manifest.starts_with(&format!(
                "{{\n  \"clax_surface\": 1,\n  \"name\": \"{name}\",\n"
            )),
            "{manifest}"
        );
        let json = format!("{name}.json");
        std::fs::write(dir.path().join(&json), &manifest).unwrap();
        // The manifest is the same definition: it reads to the same file,
        // the same manifest, and the same words for a model.
        assert_eq!(
            printed(&both(&["convert", &json], own), dir.path()),
            canonical
        );
        assert_eq!(
            printed(&both(&["check", "--dts", &json], own), dir.path()),
            canonical
        );
        assert_eq!(
            printed(&both(&["convert", "--to", "json", &json], own), dir.path()),
            manifest
        );
        assert_eq!(
            printed(&both(&["convert", "--to", "dts", &file], own), dir.path()),
            canonical
        );
        assert_eq!(
            printed(&both(&["check", &json], own), dir.path()),
            printed(&both(&["check", &file], own), dir.path())
        );
        assert_eq!(
            printed(&both(&["check", "--ops", &json], own), dir.path()),
            printed(&both(&["check", "--ops", &file], own), dir.path())
        );
        // And the file it reads to converts to the manifest again.
        let back = format!("{name}-back.d.ts");
        std::fs::write(dir.path().join(&back), &canonical).unwrap();
        assert_eq!(
            printed(&both(&["convert", &back], own), dir.path()),
            manifest
        );
    }
}

#[test]
fn convert_writes_a_file_and_replaces_one_only_when_told() {
    let dir = files();
    let manifest = printed(&["convert", "shell.d.ts"], dir.path());
    assert_eq!(
        printed(&["convert", "shell.d.ts", "-o", "shell.json"], dir.path()),
        "wrote shell.json\n"
    );
    let dest = dir.path().join("shell.json");
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), manifest);
    // An existing file is kept, as `clax toolpath export` keeps one.
    std::fs::write(&dest, "mine\n").unwrap();
    refused(
        &["convert", "shell.d.ts", "-o", "shell.json"],
        dir.path(),
        "shell.json exists; pass --force to replace it",
    );
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "mine\n");
    assert_eq!(
        printed(
            &["convert", "shell.d.ts", "--output", "shell.json", "--force"],
            dir.path()
        ),
        "wrote shell.json\n"
    );
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), manifest);
    let v: serde_json::Value = serde_json::from_str(&printed(
        &["convert", "shell.json", "-o", "back.d.ts", "--json"],
        dir.path(),
    ))
    .unwrap();
    assert_eq!(
        v,
        serde_json::json!({"name": "shell", "notation": "dts", "wrote": "back.d.ts"})
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("back.d.ts")).unwrap(),
        printed(&["check", "--dts", "shell.d.ts"], dir.path())
    );
    // A file that is refused is not converted, and nothing is written.
    std::fs::write(
        dir.path().join("bad.d.ts"),
        "declare const bad: {\n  x(): Promise<null>;\n};\n",
    )
    .unwrap();
    refused(
        &["convert", "bad.d.ts", "-o", "bad.json"],
        dir.path(),
        "bad.d.ts:2: operation `x` has no documentation",
    );
    refused(
        &["convert", "--force", "shell.d.ts"],
        dir.path(),
        "--output",
    );
    let left: std::collections::BTreeSet<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        left.into_iter().collect::<Vec<_>>(),
        [
            "back.d.ts",
            "bad.d.ts",
            "clax.d.ts",
            "review.d.ts",
            "shell.d.ts",
            "shell.json"
        ]
    );
}

#[test]
fn a_file_that_is_not_a_definition_is_refused_with_its_place() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    write(
        "tags.d.ts",
        "declare const demo: {\n  /** Reads it. @read @throws gone when it is missing */\n  get(): Promise<string>;\n};\n",
    );
    write(
        "schema.json",
        r#"{"clax_surface": 1, "name": "demo", "operations": [
  {"name": "get", "description": "Reads it.", "access": "read",
   "params": [{"name": "limit", "schema": {"type": "integer", "minimum": 1}}],
   "result": {"type": "string"}}]}"#,
    );
    write(
        "broken.json",
        "{\"clax_surface\": 1,\n \"name\": \"demo\",\n}",
    );
    write("future.json", r#"{"clax_surface": 2, "name": "demo"}"#);
    write("files.json", r#"{"clax_surface": 1, "name": "files"}"#);
    write(
        "charged.d.ts",
        "declare const demo: {\n  /** Costs. @spend */\n  buy(): Promise<null>;\n};\n",
    );
    write("notes.txt", "declare const demo: {};");
    write("large.json", &" ".repeat(256 * 1024 + 1));
    write("large.d.ts", &" ".repeat(64 * 1024 + 1));
    for (file, message) in [
        (
            "tags.d.ts",
            "tags.d.ts:2: `@throws` takes one code, and `when` follows it: write `@throws` again for each further code",
        ),
        (
            "schema.json",
            "schema.json: /operations/0/params/0/schema/minimum: `minimum` is not read here: a type is written with `type`",
        ),
        (
            "broken.json",
            "broken.json:3: the manifest is not JSON: trailing comma (column 1)",
        ),
        (
            "future.json",
            "future.json: /clax_surface: this build reads version 1 of the manifest notation",
        ),
        (
            "files.json",
            "files.json: /name: `files` is a reserved name",
        ),
        (
            "charged.d.ts",
            "charged.d.ts:3: operation `demo.buy` is tagged @spend, which only Clax's own surface may use",
        ),
        (
            "notes.txt",
            "notes.txt: a surface file is named `<name>.d.ts`, or `<name>.json` for a manifest",
        ),
        (
            "large.json",
            "large.json: the file is 262145 bytes long, and the limit is 262144",
        ),
        (
            "large.d.ts",
            "large.d.ts: the file is 65537 bytes long, and the limit is 65536",
        ),
        ("missing.d.ts", "missing.d.ts: "),
    ] {
        refused(&["check", file], dir.path(), message);
        refused(&["convert", file], dir.path(), message);
    }
    // The error codes an author wrote are the ones the table shows.
    write(
        "codes.d.ts",
        "declare const demo: {\n  /** Reads it. @read @throws gone @throws busy */\n  get(): Promise<string>;\n};\n",
    );
    assert_eq!(
        printed(&["check", "--ops", "codes.d.ts"], dir.path()),
        "OPERATION  ACCESS  WAITS  ONCE  ERRORS\ndemo.get   read    -      -     gone, busy\n"
    );
}

/// A manifest is written by a program, which gets every entry wrong the
/// same way: each entry's first mistake is said, a line each, and with
/// --json as one object on stdout.
#[test]
fn every_mistake_of_a_manifest_is_said_and_json_says_them_as_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tools.json"),
        r#"{"clax_surface": 1, "name": "demo", "operations": [
  {"name": "a", "description": "D.", "access": "get", "result": {}},
  {"name": "b", "description": "D.", "access": "read", "result": {"type": "object", "properties": {}}},
  {"name": "c", "access": "read", "result": {}}],
 "interfaces": {"T": {"type": "object", "properties": {"n": {"type": "integer", "minimum": 0}}, "additionalProperties": false}}}"#,
    )
    .unwrap();
    let out = clax()
        .current_dir(dir.path())
        .args(["bot", "surface", "check", "tools.json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let said = String::from_utf8(out.stderr).unwrap();
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 4, "{said}");
    for (line, starts) in lines.iter().zip([
        "error: tools.json: /operations/0/access: `get` is not an access",
        "error: tools.json: /operations/1/result: an object type says `\"additionalProperties\": false`",
        "error: tools.json: /operations/2: operation `c` has no `description`",
        "error: tools.json: /interfaces/T/properties/n/minimum: `minimum` is not read here",
    ]) {
        assert!(line.starts_with(starts), "{line}");
    }
    for command in ["check", "convert"] {
        let out = clax()
            .current_dir(dir.path())
            .args(["bot", "surface", command, "tools.json", "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{command}");
        assert!(out.stderr.is_empty(), "{command}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["file"], "tools.json");
        let errors = v["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 4);
        assert_eq!(errors[2]["pointer"], "/operations/2");
        assert_eq!(
            errors[2]["message"],
            "operation `c` has no `description`: say what it does, for the model to read"
        );
    }
    // A line, and a file as a whole, in the same shape.
    std::fs::write(
        dir.path().join("bad.d.ts"),
        "declare const demo: {\n  x(): Promise<null>;\n};\n",
    )
    .unwrap();
    for (file, error) in [
        (
            "bad.d.ts",
            serde_json::json!({"line": 2, "message": "operation `x` has no documentation: write a doc comment above it that says what it does, then its tags (`/** What it does. @read */`)"}),
        ),
        (
            "notes.txt",
            serde_json::json!({"message": "notes.txt: a surface file is named `<name>.d.ts`, or `<name>.json` for a manifest"}),
        ),
    ] {
        let out = clax()
            .current_dir(dir.path())
            .args(["--json", "bot", "surface", "check", file])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert!(out.stderr.is_empty(), "{file}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v, serde_json::json!({"file": file, "errors": [error]}));
    }
}

/// A manifest of nothing but wrong entries, as long as a manifest may
/// be, gets an answer a person can read: 64 mistakes and that the rest
/// was not looked at, or one that it holds too many values.
#[test]
fn what_is_said_of_a_hostile_manifest_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let padded = |entry: &str| {
        let open = r#"{"clax_surface": 1, "name": "demo", "operations": ["#;
        let n = (256 * 1024 - open.len() - 2) / (entry.len() + 1);
        format!("{open}{}]}}", vec![entry; n].join(","))
    };
    std::fs::write(dir.path().join("numbers.json"), padded("1")).unwrap();
    std::fs::write(
        dir.path().join("entries.json"),
        padded(r#"{"name": "x", "description": "An operation with no access."}"#),
    )
    .unwrap();
    for (file, lines, last) in [
        (
            "numbers.json",
            1,
            "error: numbers.json:1: more than 65536 values, the limit for a manifest (column ",
        ),
        (
            "entries.json",
            65,
            "error: entries.json: 64 mistakes are said, and the 4232 entries of the manifest after them were not looked at: put these right, and read it again",
        ),
    ] {
        let out = clax()
            .current_dir(dir.path())
            .args(["bot", "surface", "check", file])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert!(out.stdout.is_empty(), "{file}");
        let said = String::from_utf8(out.stderr).unwrap();
        assert_eq!(said.lines().count(), lines, "{file}");
        assert!(said.len() < 16 * 1024, "{file}: {} bytes", said.len());
        assert!(
            said.lines().last().unwrap().starts_with(last),
            "{file}: {said}"
        );
        let out = clax()
            .current_dir(dir.path())
            .args(["bot", "surface", "check", "--json", file])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert!(
            out.stdout.len() < 16 * 1024 && out.stderr.is_empty(),
            "{file}"
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let errors = v["errors"].as_array().unwrap();
        assert_eq!(errors.len(), lines, "{file}");
        if lines == 65 {
            assert_eq!(errors[64]["stopped"], true);
            assert_eq!(errors[64]["unread"], 4232);
            assert!(errors[63].get("stopped").is_none());
        }
    }
}

/// An allow pattern written without its surface's name allows nothing
/// of its own surface: a kind may allow another surface's operations,
/// so a pattern of several names cannot be refused (a single name that
/// no operation of Clax's own surface has is). `check` shows what each
/// kind allows and notes it.
#[test]
fn check_notes_an_allow_pattern_that_matches_nothing_it_knows() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("shell.json"),
        r#"{"clax_surface": 1, "name": "shell", "operations": [
  {"name": "windows", "description": "D.", "access": "read", "result": {}},
  {"name": "settings.set", "description": "D.", "access": "write", "result": {}}],
 "kinds": [{"name": "slip", "description": "K.", "allow": ["settings.*", "files.read", "shell.windows"], "request": {}, "result": {"type": "object", "properties": {}, "additionalProperties": false}}]}"#,
    )
    .unwrap();
    let notes = "note: shell.json: kind `slip` allows `settings.*`, which matches no operation of `shell` or of Clax's own surface; it will match only another surface's (an allow pattern names the surface too: `shell.settings.*`)\n";
    for args in [
        vec!["check", "shell.json"],
        vec!["check", "--ops", "shell.json"],
        vec!["check", "--dts", "shell.json"],
    ] {
        let out = clax()
            .current_dir(dir.path())
            .args(["bot", "surface"])
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert_eq!(String::from_utf8(out.stderr).unwrap(), notes, "{args:?}");
        assert!(!out.stdout.is_empty());
    }
    assert_eq!(
        printed(&["check", "--ops", "shell.json"], dir.path()),
        "OPERATION           ACCESS  WAITS  ONCE  ERRORS\n\
         shell.windows       read    -      -     -\n\
         shell.settings.set  write   -      -     -\n\
         \n\
         KIND  ALLOWS\n\
         slip  settings.* files.read shell.windows\n"
    );
    let out = clax()
        .current_dir(dir.path())
        .args(["bot", "surface", "check", "--json", "shell.json"])
        .output()
        .unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["notes"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["kinds"],
        serde_json::json!([{"name": "slip", "allow": ["settings.*", "files.read", "shell.windows"]}])
    );
    // One name with no `.` that no operation of Clax's own surface has
    // names nothing anywhere: it is refused, not noted.
    std::fs::write(
        dir.path().join("bare.json"),
        r#"{"clax_surface": 1, "name": "shell", "operations": [
  {"name": "windows", "description": "D.", "access": "read", "result": {}}],
 "kinds": [{"name": "slip", "description": "K.", "allow": ["windows"], "request": {}, "result": {"type": "object", "properties": {}, "additionalProperties": false}}]}"#,
    )
    .unwrap();
    let out = clax()
        .current_dir(dir.path())
        .args(["bot", "surface", "check", "bare.json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8(out.stderr).unwrap();
    assert!(
        said.contains(
            "kind `slip` allows `windows`, which names no operation: an allow pattern names \
             the surface too (`shell.windows`)"
        ),
        "{said}"
    );
    // A file with nothing to note says nothing on stderr.
    let dir = files();
    let out = clax()
        .current_dir(dir.path())
        .args(["bot", "surface", "check", "review.d.ts"])
        .output()
        .unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
}

/// A file's `//` comments and its layout are not its definition:
/// `convert` does not write its own writing of a file over the file.
#[test]
fn convert_does_not_write_over_the_file_it_reads() {
    let dir = files();
    for dest in ["shell.d.ts", "./shell.d.ts", "../shell.d.ts"] {
        let cwd = if dest.starts_with("..") {
            std::fs::create_dir_all(dir.path().join("sub")).unwrap();
            dir.path().join("sub")
        } else {
            dir.path().to_path_buf()
        };
        let source = if dest.starts_with("..") {
            "../shell.d.ts"
        } else {
            "shell.d.ts"
        };
        refused(
            &["convert", source, "--to", "dts", "-o", dest, "--force"],
            &cwd,
            "shell.d.ts is the file being read: written again, a surface file loses its `//` comments and its layout",
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("shell.d.ts")).unwrap(),
        SHELL
    );
    // Tools on Windows put a byte-order mark before JSON: it is skipped.
    let manifest = printed(&["convert", "shell.d.ts"], dir.path());
    std::fs::write(
        dir.path().join("marked.json"),
        format!("\u{feff}{manifest}"),
    )
    .unwrap();
    assert_eq!(
        printed(&["convert", "--to", "json", "marked.json"], dir.path()),
        manifest
    );
}

/// The commands leave a home alone when one is named, and need none.
#[test]
fn no_home_is_read_or_made_and_no_daemon_started() {
    let dir = files();
    let home = dir.path().join("home");
    let out = clax()
        .current_dir(dir.path())
        .env("HOME", &home)
        .env("CLAX_HOME", home.join("ax"))
        .args([
            "bot",
            "surface",
            "convert",
            "shell.d.ts",
            "-o",
            "shell.json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!home.exists(), "a home was made");
    // With no home to name, a command that needs one says so, and these
    // run.
    clax()
        .current_dir(dir.path())
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "neither CLAX_HOME nor HOME is set",
        ));
    printed(&["check", "shell.json"], dir.path());
    // A home connected to another machine's Clax does not stop them.
    std::fs::create_dir_all(home.join("ax")).unwrap();
    std::fs::write(
        home.join("ax/remote.json"),
        r#"{"url": "http://192.0.2.1:7480", "person": "p", "machine": "m"}"#,
    )
    .unwrap();
    clax()
        .current_dir(dir.path())
        .env("CLAX_HOME", home.join("ax"))
        .args(["bot", "surface", "check", "--ops", "shell.json"])
        .assert()
        .success();
    let left: Vec<_> = std::fs::read_dir(home.join("ax"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["remote.json"]);
}

/// What a run of a kind of ClaxBot's own file, and of its fake's, is
/// given (`Definition::run_operations`), and where their requests hold
/// the person's words: both files belong to clax-bot, which this crate
/// reads them through.
#[test]
fn a_run_of_claxbots_own_kinds_and_of_the_fakes_is_given_its_operations() {
    let clax = clax_bot::api::definition();
    let all: Vec<&str> = clax.operations().map(|o| o.name).collect();
    for kind in ["thread", "brief"] {
        // Its kinds say no `@allow`, and none of its operations holds
        // content: a run has every one.
        let ops = clax.run_operations(kind, &Default::default()).unwrap();
        assert_eq!(ops.granted, all, "{kind}");
        assert!(
            ops.withheld.is_empty() && ops.elsewhere.is_empty(),
            "{kind}"
        );
        assert_eq!(format!("{:?}", ops.request_content), "None");
    }
    // A comment's text is a `string`: nothing of the request is typed
    // as the person's words.
    let thread = clax.kind("thread").unwrap();
    let request = serde_json::json!({
        "artifact": {"id": "a", "title": "T", "description": null, "version": 1, "url": null},
        "for": "sam",
        "threads": [{"id": "t", "anchor": null, "resolved": false, "comments": [
            {"id": "c", "author": "sam", "from_sender": true, "at": "now", "text": "Fix it"},
        ]}],
    });
    assert!(
        thread
            .instruction_places(clax, &request)
            .unwrap()
            .is_empty()
    );

    let fake = clax_bot::surface::FakeSurfaceProvider::definition();
    let ops = fake.run_operations("job", &Default::default()).unwrap();
    assert_eq!(
        ops.granted,
        [
            "fake.count",
            "fake.get",
            "fake.set",
            "fake.fail",
            "fake.refuse",
            "fake.ask"
        ]
    );
    assert_eq!(format!("{:?}", ops.withheld), r#"[("fake.hidden", Kind)]"#);
    assert!(ops.elsewhere.is_empty());
    let job = fake.kind("job").unwrap();
    assert_eq!(
        job.instruction_places(fake, &serde_json::json!({"ask": "count"}))
            .unwrap(),
        ["/ask"]
    );
}
