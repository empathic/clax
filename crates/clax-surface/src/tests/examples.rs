//! The design's two example files of a host's and the crate's fixture
//! of a surface named `clax` (`own.d.ts`), what a model reads of them,
//! and the round trip through the file printer.

use super::{OWN_DTS, REVIEW_DTS, SHELL_DTS, file, host, own, refused, review, shell};
use crate::{
    Access, Author, CONTENT_DECLARATION, Definition, Limits, Member, Pattern, Reserved, Ty,
    canonical_manifest, declaration, parse, print,
};

fn named(n: &str) -> Ty {
    Ty::Named(n.to_string())
}

fn array(t: Ty) -> Ty {
    Ty::Array(Box::new(t))
}

/// The fields of interface `i`, as (name, optional, type).
fn fields<'a>(d: &'a Definition, i: &str) -> Vec<(&'a str, bool, &'a Ty)> {
    let i = d.interface(i).unwrap_or_else(|| panic!("no interface {i}"));
    i.fields
        .iter()
        .map(|f| (f.name.as_str(), f.optional, &f.ty))
        .collect()
}

#[test]
fn the_fixture_named_clax_is_read_as_written() {
    let clax = own();
    assert_eq!(clax.name(), "clax");
    assert_eq!(clax.rules(), None);
    assert_eq!(clax.operations().len(), 14);
    assert_eq!(clax.interfaces().len(), 20);

    // An operation with every kind of parameter.
    let update = clax.operation("db.update").unwrap();
    assert_eq!(update.name, "db.update");
    assert_eq!(update.op.name, "update");
    assert_eq!(
        update.op.doc,
        "Merges fields into a document at the version you read, at once (not on the draft)."
    );
    assert_eq!(
        (update.op.access, update.op.waits, update.op.once),
        (Access::Write, false, false)
    );
    assert_eq!(
        update.op.throws,
        ["forbidden", "conflict", "not_found", "invalid"]
    );
    let params: Vec<_> = update
        .op
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.optional, &p.ty))
        .collect();
    assert_eq!(
        params,
        [
            ("collection", false, &Ty::Str),
            ("id", false, &Ty::Str),
            ("patch", false, &Ty::Record(Box::new(Ty::Unknown))),
            ("opts", false, &named("IfVersion")),
        ]
    );
    assert_eq!(update.op.result, named("DocRef"));

    // A read, an optional parameter with an inline object, a union result.
    let list = clax.operation("db.list").unwrap();
    assert_eq!(list.op.access, Access::Read);
    assert!(list.op.params[1].optional);
    let Ty::Object(opts) = &list.op.params[1].ty else {
        panic!("{:?}", list.op.params[1].ty)
    };
    assert_eq!(
        opts.iter()
            .map(|f| (f.name.as_str(), f.optional, &f.ty))
            .collect::<Vec<_>>(),
        [("limit", true, &Ty::Int), ("cursor", true, &Ty::Str)]
    );
    assert_eq!(
        clax.operation("db.get").unwrap().op.result,
        Ty::union(named("Doc"), [Ty::Null])
    );
    assert_eq!(
        clax.operation("files.list").unwrap().op.result,
        array(named("FileInfo"))
    );

    // The two that are charged, and a literal union inside an object.
    let fetch = clax.operation("web.fetch").unwrap();
    assert_eq!(fetch.op.access, Access::Spend);
    assert_eq!(fetch.op.throws.len(), 10);
    let Ty::Object(opts) = &fetch.op.params[1].ty else {
        panic!()
    };
    assert_eq!(
        opts[1].ty,
        Ty::union(Ty::StrLit("auto".into()), [Ty::StrLit("raw".into())])
    );
    let spends: Vec<&str> = clax
        .operations()
        .filter(|o| o.op.access == Access::Spend)
        .map(|o| o.name)
        .collect();
    assert_eq!(spends, ["web.search", "web.fetch"]);
    assert_eq!(
        clax.operations()
            .filter(|o| o.op.access == Access::Write)
            .count(),
        6
    );
    assert_eq!(
        clax.operations()
            .filter(|o| o.op.access == Access::Read)
            .count(),
        6
    );

    // A namespace's rules are its doc comment.
    let Some(Member::Namespace(web)) = clax.members().last() else {
        panic!()
    };
    assert_eq!(
        web.rules.as_deref(),
        Some(
            "web.* calls are charged to the person you act for and limited\nper run; `left` on every answer says what the run has left."
        )
    );

    // Whole numbers and numbers are told apart; optional and null too.
    assert_eq!(
        fields(&clax, "WebLeft"),
        [
            ("searches", false, &Ty::Int),
            ("fetches", false, &Ty::Int),
            ("credits", false, &Ty::Num)
        ]
    );
    assert_eq!(
        fields(&clax, "ArtifactInfo")[2],
        ("description", true, &Ty::union(Ty::Str, [Ty::Null]))
    );
    assert_eq!(
        fields(&clax, "WebPage")[6],
        ("nextOffset", false, &Ty::union(Ty::Int, [Ty::Null]))
    );

    // The two kinds, which narrow nothing and take no message.
    let kinds: Vec<_> = clax
        .kinds()
        .iter()
        .map(|k| (k.name.as_str(), &k.request, &k.message, &k.result, &k.allow))
        .collect();
    assert_eq!(
        kinds,
        [
            (
                "thread",
                &named("ThreadRequest"),
                &None,
                &named("Done"),
                &None
            ),
            (
                "brief",
                &named("BriefRequest"),
                &None,
                &named("Done"),
                &None
            ),
        ]
    );
    assert!(
        clax.kind("thread")
            .unwrap()
            .doc
            .starts_with("The request is one or more comment threads")
    );
    let from_sender = &clax.interface("SentComment").unwrap().fields[2];
    assert_eq!(
        from_sender.doc.as_deref(),
        Some("Whether the person the run acts for wrote it: only their words are requests.")
    );
}

#[test]
fn shell_and_review_are_read_as_written() {
    let shell = shell();
    assert!(shell.rules().is_some_and(|r| r.starts_with(
        "You are working on a ClaxShell desktop. Each app is an artifact; the\nbar,"
    ) && r.ends_with("before you say it is done.")));
    let ask = shell.operation("shell.ask").unwrap();
    assert_eq!(
        (ask.op.access, ask.op.waits, ask.op.once),
        (Access::Effect, true, true)
    );
    assert_eq!(ask.op.throws, ["declined"]);
    assert_eq!(
        shell
            .operations()
            .filter(|o| o.op.waits)
            .map(|o| o.name)
            .collect::<Vec<_>>(),
        ["shell.ask"]
    );
    let shot = shell.operation("shell.screenshot").unwrap();
    assert_eq!(shot.op.result, Ty::Image);
    assert!(matches!(&shot.op.params[0].ty, Ty::Union(alts) if alts.len() == 2));
    assert_eq!(
        fields(&shell, "Triage"),
        [
            ("ask", false, &Ty::Instruction),
            ("touches", false, &array(named("Touched"))),
            ("clip", true, &Ty::Image)
        ]
    );
    let triage = shell.kind("triage").unwrap();
    let allow: Vec<String> = triage
        .allow
        .as_ref()
        .unwrap()
        .iter()
        .map(Pattern::to_string)
        .collect();
    assert_eq!(
        allow,
        [
            "shell.parts",
            "shell.windows",
            "shell.settings.get",
            "shell.screenshot",
            "shell.ask",
            "threads.open",
            "runs.wait"
        ]
    );
    assert!(triage.doc.ends_with("Change no artifact yourself."));

    let review = review();
    assert_eq!(
        review.operation("review.view").unwrap().op.result,
        Ty::union(named("PullView"), [named("CompareView"), named("RefView")])
    );
    assert_eq!(review.interface("CommentPage").unwrap().extends, ["Listed"]);
    assert_eq!(review.interface("Listed").unwrap().extends, ["Named"]);
    assert_eq!(fields(&review, "Diff")[2], ("diff", false, &Ty::Content));
    assert_eq!(
        review
            .fields_of("FilePage")
            .unwrap()
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        [
            "subject",
            "head_sha",
            "next_cursor",
            "truncated",
            "from",
            "to",
            "files",
            "changed_files"
        ]
    );
    assert_eq!(
        review
            .kinds()
            .iter()
            .map(|k| k.name.as_str())
            .collect::<Vec<_>>(),
        ["range", "about", "annotate", "plan"]
    );
    assert!(
        matches!(&review.kind("annotate").unwrap().result, Ty::Object(fields) if fields.len() == 2)
    );
    assert_eq!(
        review
            .operations()
            .filter(|o| o.op.access != Access::Read)
            .map(|o| o.name)
            .collect::<Vec<_>>(),
        ["review.progress", "review.quest.note"]
    );
}

#[test]
fn the_example_files_round_trip() {
    for (name, def, own) in [
        ("clax", own(), true),
        ("shell", shell(), false),
        ("review", review(), false),
    ] {
        let printed = print(&def);
        let again = if own {
            parse(&printed, Author::Clax, &Limits::default())
        } else {
            host(&printed)
        }
        .unwrap_or_else(|e| panic!("{name}, printed, does not parse: {e}\n{printed}"));
        assert_eq!(again, def, "{name}, printed, parses to another definition");
        assert_eq!(print(&again), printed, "printing {name} is not stable");
    }
}

/// `src` with the `;` that ends a line taken off every line outside a
/// doc comment: a file written without semicolons, as TypeScript reads
/// one whose members each end at a line break.
fn without_semicolons(src: &str) -> String {
    let mut in_doc = false;
    let mut out = String::new();
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        let starts_doc = t.starts_with("/**");
        let doc = in_doc || starts_doc;
        if starts_doc && !t.contains("*/") {
            in_doc = true;
        } else if in_doc && t.contains("*/") {
            in_doc = false;
        }
        let (body, end) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        match body.strip_suffix(';') {
            Some(bare) if !doc => {
                out.push_str(bare);
                out.push_str(end);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// A member ends at a line break as it does at `;` or `,`, as
/// TypeScript's do: the example files, as written and as printed, read
/// without their semicolons to the same definitions, with the same
/// canonical bytes. Two members on one line still need one between
/// them, and a type continued on the next line is still one member's.
#[test]
fn members_end_at_a_line_break_as_at_a_semicolon() {
    for (name, src, def, own) in [
        ("clax", OWN_DTS, own(), true),
        ("shell", SHELL_DTS, shell(), false),
        ("review", REVIEW_DTS, review(), false),
    ] {
        let read = |text: &str| {
            if own {
                parse(text, Author::Clax, &Limits::default())
            } else {
                host(text)
            }
        };
        for text in [src.to_string(), print(&def)] {
            let bare = without_semicolons(&text);
            assert_ne!(bare, text, "{name}");
            let again = read(&bare).unwrap_or_else(|e| panic!("{name}: {e}\n{bare}"));
            assert_eq!(again, def, "{name}");
            assert_eq!(
                canonical_manifest(&again),
                canonical_manifest(&def),
                "{name}"
            );
        }
    }
    let plain = host(&file(
        "  /** Reads. @read */\n  get(): Promise<Named>;\n  /** A namespace. */\n  ns: {\n    /** Reads. @read */\n    x(): Promise<null>;\n  };",
        "interface Named {\n  id: string;\n  title: string;\n}\ninterface Kinds {\n  /** K. @allow demo.get */\n  k(request: { a: string; b: string }): { summary: string };\n}",
    ))
    .unwrap();
    let bare = host(&file(
        "  /** Reads. @read */\n  get(): Promise<Named>\n  /** A namespace. */\n  ns: {\n    /** Reads. @read */\n    x(): Promise<null>\n  }",
        "interface Named {\n  id: string\n  title: string\n}\ninterface Kinds {\n  /** K. @allow demo.get */\n  k(request: {\n    a: string\n    b: string\n  }): { summary: string }\n}",
    ))
    .unwrap();
    assert_eq!(bare, plain);
    // A union continued on the next line is the one field's type.
    let union = host(&file(
        "  /** Reads. @read */\n  get(): Promise<A>;",
        "interface A {\n  s:\n    | \"x\"\n    | \"y\"\n  t: string\n}",
    ))
    .unwrap();
    assert_eq!(
        union.interface("A").unwrap().fields.len(),
        2,
        "{}",
        print(&union)
    );
    // On one line, members are still separated.
    refused(
        &file("", "interface A { id: string title: string }"),
        4,
        "expected `;`, `,`, a line break or `}` after a member, found `title`",
    );
}

/// Small files that between them use every form the printer writes.
#[test]
fn a_family_of_small_definitions_round_trips() {
    let cases: &[(&str, &str)] = &[
        (
            "escapes in literals",
            "declare const demo: {\n  /** D. @read */\n  x(a: \"a\\\"b\" | \"c\\\\d\" | \"\" | \" \" | \"*/\" | \"@read\"): Promise<null>;\n};",
        ),
        (
            "nested unions and arrays",
            "declare const demo: {\n  /** D. @read */\n  x(a: (string | null)[][], b?: Record<string, (A | B)[] | null>, c?: { u: (A | B)[]; v?: { w: true | false }[] }): Promise<(A | (B | null))[]>;\n};\ninterface A { k: \"a\" }\ninterface B extends A, C { j: \"b\" }\ninterface C {}",
        ),
        (
            "a leading pipe",
            "declare const demo: {\n  /** D. @read */\n  x(a:\n    | \"a\"\n    | \"b\"): Promise<null>;\n};",
        ),
        (
            "a kind with a message and two allow lines",
            "declare const demo: {};\ninterface Kinds {\n  /**\n   * Para one.\n   *\n   * Para two.\n   * @allow * demo.* demo.a.b\n   * @allow files.read\n   */\n  go(request: { ask: Instruction }, message?: Instruction | { n: Int }): { outcome: { ok: true } | { ok: false; why: string } };\n}",
        ),
        (
            "commas as separators",
            "declare const demo: {\n  /** D. @read */\n  x(a: { p: string, q?: Int, },): Promise<null>,\n  ns: { },\n};\ninterface A { a: string, b: Int }",
        ),
        (
            "prose with bullets, a line that is only a star, and an indented example",
            "declare const demo: {\n  /**\n   * Does:\n   * * one\n   * *\n   *\n   *     clax.demo.x()\n   * @write @once @waits @throws a @throws b\n   */\n  x(): Promise<null>;\n};",
        ),
        (
            "prose that looks like comments",
            "declare const demo: {\n  /** Globs like a/* and // are prose. @read */\n  x(): Promise<null>;\n};",
        ),
        (
            "docs everywhere",
            "/**\n * Rules,\n * on two lines.\n */\ndeclare const demo: {\n  /** A namespace's rules. */\n  ns: {\n    deeper: {\n      /** D. @effect */\n      x(): Promise<A>;\n    };\n  };\n};\n/** An A. */\ninterface A {\n  /**\n   * A field,\n   *   documented at length.\n   */\n  a?: Content | Image | null;\n}",
        ),
        ("an empty surface", "declare const demo: {};"),
    ];
    for (what, src) in cases {
        let def = host(src).unwrap_or_else(|e| panic!("{what}: {e}"));
        let printed = print(&def);
        let again = host(&printed)
            .unwrap_or_else(|e| panic!("{what}, printed, does not parse: {e}\n{printed}"));
        assert_eq!(
            again, def,
            "{what}, printed, parses to another definition:\n{printed}"
        );
        assert_eq!(print(&again), printed, "printing {what} is not stable");
    }
}

#[test]
fn the_file_printer_writes_each_form() {
    let def = host(
        "/** Rules. */\ndeclare const demo: {\n  /**\n   * Does:\n   *     an example\n   * @write @once @waits @throws a @throws b\n   */\n  x(a: \"q\\\"\" | \"b\\\\\", b?: (A | null)[][]): Promise<Record<string, { n: Int }>>;\n  ns: {};\n};\ninterface A extends B { /** Doc. */ a: Int }\ninterface B {}\ninterface Kinds {\n  /** Go. @allow demo.* */\n  go(request: A, message?: string): B;\n}",
    )
    .unwrap();
    assert_eq!(
        print(&def),
        "/** Rules. */\ndeclare const demo: {\n  /**\n   * Does:\n   *     an example\n   * @write @waits @once @throws a @throws b\n   */\n  x(a: \"q\\\"\" | \"b\\\\\", b?: (A | null)[][]): Promise<Record<string, { n: Int }>>;\n  ns: {\n  };\n};\n\ninterface A extends B {\n  /** Doc. */\n  a: Int;\n}\ninterface B {\n}\n\ninterface Kinds {\n  /** Go. @allow demo.* */\n  go(request: A, message?: string): B;\n}\n"
    );
}

#[test]
fn a_surface_named_clax_is_refused_as_a_hosts() {
    let none = Reserved::of(&[]);
    let e = parse(
        OWN_DTS,
        Author::Host { reserved: &none },
        &Limits::default(),
    )
    .unwrap_err();
    assert!(e.message.contains("`clax` is a reserved name"), "{e}");
    assert_eq!(
        OWN_DTS.lines().nth(e.line().unwrap() - 1),
        Some("declare const clax: {")
    );
}

/// A host's surface may not be named as a field of a run's step: each
/// such name is refused at the surface's declaration (`tool_use_id`
/// as no surface's name at all), and Clax's own surface may still take
/// one.
#[test]
fn a_hosts_surface_named_as_a_field_of_a_step_is_refused() {
    let none = Reserved::of(&[]);
    let host = Author::Host { reserved: &none };
    for name in crate::STEP_FIELDS {
        let src = format!("declare const {name}: {{}};");
        let e = parse(&src, host, &Limits::default()).unwrap_err();
        assert_eq!(e.line(), Some(1), "{name}");
        if name.contains('_') {
            assert!(e.message.contains("is not one lowercase word"), "{e}");
            continue;
        }
        assert!(
            e.message.contains(&format!(
                "`{name}` is a reserved name: a host's surface may not be named as a field of a run's step"
            )),
            "{name}: {e}"
        );
        assert!(
            parse(&src, Author::Clax, &Limits::default()).is_ok(),
            "{name}"
        );
    }
    let mut sorted = crate::STEP_FIELDS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, crate::STEP_FIELDS, "in order, each once");
    assert!(parse("declare const shell: {};", host, &Limits::default()).is_ok());
}

#[test]
fn the_reserved_names_are_claxs_namespaces_and_the_cores_surfaces() {
    let clax = own();
    assert_eq!(
        Reserved::of(&[&clax]).names().collect::<Vec<_>>(),
        [
            "artifact",
            "artifacts",
            "clax",
            "content",
            "db",
            "draft",
            "files",
            "runs",
            "threads",
            "web"
        ]
    );
    // With no definition in hand, the fixed ones are still there.
    assert_eq!(
        Reserved::of(&[]).names().collect::<Vec<_>>(),
        ["clax", "content", "runs"]
    );
    // Another surface of Clax's reserves its own name.
    let memory = parse(
        "declare const memory: {};",
        Author::Clax,
        &Limits::default(),
    )
    .unwrap();
    assert!(Reserved::of(&[&clax, &memory]).holds("memory"));
    assert!(!Reserved::of(&[&clax]).holds("shell"));
}

#[test]
fn operations_are_listed_by_dotted_name_in_declaration_order() {
    let names = |d: &Definition| {
        d.operations()
            .map(|o| o.name.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&shell()),
        [
            "shell.parts",
            "shell.windows",
            "shell.workspaces",
            "shell.bridge",
            "shell.screenshot",
            "shell.settings.get",
            "shell.settings.set",
            "shell.window.open",
            "shell.notify",
            "shell.ask",
        ]
    );
    let clax = own();
    let all = names(&clax);
    assert_eq!(all[..3], ["artifact.get", "artifacts.create", "files.list"]);
    assert_eq!(all[all.len() - 2..], ["web.search", "web.fetch"]);
    assert!(clax.operation("fetch").is_none());
    assert!(clax.operation("clax.web.fetch").is_none());
    // Each is found by the name it is listed under.
    for d in [&clax, &shell(), &review()] {
        for o in d.operations() {
            let found = d.operation(o.name).unwrap();
            assert!(std::ptr::eq(found.op, o.op), "{}", o.name);
        }
    }
    assert_eq!(shell().top_level_names(), ["shell"]);
    assert_eq!(
        clax.top_level_names(),
        [
            "artifact",
            "artifacts",
            "files",
            "draft",
            "db",
            "threads",
            "web"
        ]
    );
    // An operation after a namespace, and one in a namespace two deep.
    let d = host(&file(
        "  a: {\n    b: {\n      /** D. @read */\n      x(): Promise<null>;\n    };\n    /** D. @read */\n    y(): Promise<null>;\n  };\n  /** D. @read */\n  z(): Promise<null>;",
        "",
    ))
    .unwrap();
    assert_eq!(names(&d), ["demo.a.b.x", "demo.a.y", "demo.z"]);
    assert_eq!(d.operation("demo.a.y").unwrap().op.name, "y");
}

// ------------------------------------------------------- what a model reads

#[test]
fn every_surface_is_a_member_of_the_one_clax_object() {
    let (clax, shell) = (own(), shell());
    let both = declaration(&[&clax, &shell], &|_| true).unwrap();
    assert_eq!(both.matches("declare const").count(), 1, "{both}");
    // Clax's part is what it is alone, to the byte, up to where the host's begins.
    let alone = declaration(&[&clax], &|_| true).unwrap();
    let (alone_types, alone_object) = alone.split_at(alone.find("\ndeclare const clax").unwrap());
    assert!(both.starts_with(alone_types), "{both}");
    let object = &both[both.find("declare const clax: {\n").expect("the const")..];
    assert!(
        object.starts_with(alone_object[1..].strip_suffix("};\n").unwrap()),
        "{object}"
    );
    assert!(object.ends_with("  };\n};\n"), "{object}");
    // The host's one namespace, its own nested, its types named through it.
    assert!(
        object.contains(
            "\n  shell: {\n    /** Which artifact fills each part of the shell, and which is being worked on. */\n    parts(): Promise<shell.Part[]>;\n"
        ),
        "{object}"
    );
    assert!(object.contains(
        "    settings: {\n      /** The appearance settings. */\n      get(): Promise<shell.Settings>;\n      /** Changes appearance settings; the fields left out keep their values. (writes) Errors: invalid. */\n      set(patch: shell.SettingsPatch): Promise<shell.Settings>;\n    };\n"
    ));
    assert!(object.contains(
        "    /** Opens an app's window on the person's screen. (writes) Errors: not_found. */\n"
    ));
    assert!(object.contains(
        "    /** Asks the person in the Comment box, and waits for their answer. (writes) (may take long) Errors: declined. */\n    ask(question: string, choices?: string[]): Promise<string>;\n"
    ));
    // The core's types are everyone's.
    assert!(
        object.contains(
            "    screenshot(of: { part: string } | { window: string }): Promise<Image>;\n"
        )
    );
    // The host's interfaces are in a namespace of its name, between
    // Clax's and the object, and name each other as the file does.
    let types = &both[alone_types.len()..both.len() - object.len()];
    assert!(
        types.starts_with("declare namespace shell {\n  interface Part {\n"),
        "{types}"
    );
    // The core's types come after them: the shell names an `Image`.
    assert!(
        types.ends_with(&format!("  }}\n}}\n{CONTENT_DECLARATION}\n")),
        "{types}"
    );
    assert!(types.contains(
        "  interface Part {\n    /** `bar`, `dock`, `launcher`, … */\n    name: string;\n    artifact: string;\n    title: string;\n    /** A ClaxBot run is editing it now. */\n    working: boolean;\n  }\n"
    ));
    assert!(types.contains(
        "  interface Window {\n    id: string;\n    artifact: string;\n    title: string;\n    workspace: number;\n    focused: boolean;\n  }\n"
    ));
    assert!(types.contains("  interface Bridge {\n    calls: { name: string; doc: string }[];\n"));
    for absent in ["Kinds", "Triage", "ClaxShell desktop", "@", "Int"] {
        assert!(!both.contains(absent), "{absent} in {both}");
    }
}

#[test]
fn a_run_reads_only_what_it_has() {
    let (clax, shell) = (own(), shell());
    let has = |op: &str| matches!(op, "files.read" | "shell.settings.get");
    assert_eq!(
        declaration(&[&clax, &shell], &has).unwrap(),
        "declare namespace shell {\n  interface Settings {\n    theme: \"light\" | \"dark\";\n    accent: string;\n    corners: number;\n  }\n}\n\n\
         declare const clax: {\n  files: {\n    /** A draft file's text. Errors: invalid_path, not_found, not_text. */\n    read(path: string): Promise<string>;\n  };\n  \
         shell: {\n    settings: {\n      /** The appearance settings. */\n      get(): Promise<shell.Settings>;\n    };\n  };\n};\n"
    );
    assert_eq!(
        declaration(&[&clax, &shell], &|_| false).unwrap(),
        "declare const clax: {\n};\n"
    );
    // A host's operations that reach no interface bring no namespace of types.
    assert_eq!(
        declaration(&[&shell], &|op| op == "shell.notify").unwrap(),
        "declare const clax: {\n  shell: {\n    /** Shows the person a notification. (writes) */\n    notify(text: string): Promise<null>;\n  };\n};\n"
    );
}

#[test]
fn two_surfaces_types_of_one_name_do_not_meet() {
    let (clax, review) = (own(), review());
    // Both declare `Comment`, differently, and both are reached.
    let d = declaration(&[&clax, &review], &|_| true).unwrap();
    assert!(d.contains("\ninterface Comment {\n  id: string;\n  author: string;\n  at: string;\n  text: string;\n}\n"));
    assert!(d.contains("  interface Comment {\n    kind: \"comment\" | \"review\" | \"thread_comment\";\n    id: string;\n"));
    assert!(d.contains("interface Thread {\n  id: string;\n  resolved: boolean;\n  anchor?: unknown;\n  comments: Comment[];\n}\n"));
    assert!(d.contains("  interface CommentPage extends Listed {\n    items: Comment[];\n    total: number | null;\n  }\n"));
    assert!(d.contains("  interface Listed extends Named {\n"));
    assert!(d.contains("    comments(page?: review.Page): Promise<review.CommentPage>;\n"));
    assert!(
        d.contains("    view(): Promise<review.PullView | review.CompareView | review.RefView>;\n")
    );
    assert!(d.contains("    read(id: string): Promise<Thread>;\n"));
    // A host's types stand between Clax's and the object, in one namespace.
    assert_eq!(d.matches("declare namespace ").count(), 1);
    let (clax_types, rest) = d.split_at(d.find("declare namespace review {\n").unwrap());
    assert!(!clax_types.contains("  interface "), "{clax_types}");
    assert!(rest.contains(&format!(
        "\n}}\n{CONTENT_DECLARATION}\ndeclare const clax: {{\n"
    )));
    // Two hosts with a type of one name, too.
    let other = host("declare const notes: {\n  /** Lists. @read */\n  list(page?: Page): Promise<Page>;\n};\ninterface Page { n: Int }").unwrap();
    let d = declaration(&[&review, &other], &|_| true).unwrap();
    assert!(d.contains("declare namespace notes {\n  interface Page {\n    n: number;\n  }\n}\n"));
    assert!(d.contains("    list(page?: notes.Page): Promise<notes.Page>;\n"));
    assert!(d.contains("declare namespace review {\n  interface Page {\n    cursor?: string;\n    limit?: number;\n  }\n"));
}

#[test]
fn inherited_interfaces_are_read_with_the_ones_that_extend_them() {
    let review = review();
    let d = declaration(&[&review], &|op| op == "review.comments").unwrap();
    assert_eq!(
        d.lines()
            .filter(|l| l.starts_with("  interface "))
            .collect::<Vec<_>>(),
        [
            "  interface Page {",
            "  interface Named {",
            "  interface Listed extends Named {",
            "  interface CommentPage extends Listed {",
            "  interface Comment {",
        ]
    );
    assert!(d.contains(
        "  review: {\n    /** A pull request's conversation, newest first, then its review threads. Errors: forbidden. */\n    comments(page?: review.Page): Promise<review.CommentPage>;\n  };\n"
    ));
}

#[test]
fn surfaces_that_put_one_name_on_clax_cannot_be_printed_together() {
    let clax = own();
    let e = declaration(&[&clax, &clax], &|_| true).unwrap_err();
    assert!(e.contains("two surfaces put `artifact` on `clax`"), "{e}");
    // Clax's own surfaces may take a name `clax` already has.
    let files = parse(
        "declare const files: { /** Doc. @read */ x(): Promise<null>; };",
        Author::Clax,
        &Limits::default(),
    )
    .unwrap();
    let e = declaration(&[&clax, &files], &|_| true).unwrap_err();
    assert!(e.contains("two surfaces put `files` on `clax`"), "{e}");
    let shell = shell();
    assert!(declaration(&[&shell, &shell], &|_| true).is_err());
}

#[test]
fn the_models_wording_for_each_tag() {
    let d = parse(
        "declare const store: {\n  /** Buys it. @spend @waits @once @throws declined @throws credits_exhausted */\n  buy(what: Int): Promise<Int>;\n  /** Sends it. @effect @once */\n  send(): Promise<null>;\n  /** Waits for it. @read @waits */\n  wait(): Promise<null>;\n  /**\n   * Sets it,\n   *   on two lines.\n   * @write @throws invalid\n   */\n  set(): Promise<null>;\n};",
        Author::Clax,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(
        declaration(&[&d], &|_| true).unwrap(),
        "declare const clax: {\n  store: {\n    /** Buys it. (charged) (may take long) Errors: declined, credits_exhausted. */\n    buy(what: number): Promise<number>;\n    /** Sends it. (writes) */\n    send(): Promise<null>;\n    /** Waits for it. (may take long) */\n    wait(): Promise<null>;\n    /** Sets it, on two lines. (writes) Errors: invalid. */\n    set(): Promise<null>;\n  };\n};\n"
    );
}

#[test]
fn the_cores_types_are_read_by_their_names() {
    let d = host(&file(
        "  /** Shows it. @read */\n  shot(of?: Content): Promise<Image>;\n  /** Asks. @effect @waits */\n  ask(): Promise<Instruction | null>;\n  /** Lists. @read */\n  all(): Promise<Shot[]>;",
        "/** One shot. */\ninterface Shot {\n  /** The picture. */\n  image: Image;\n  text?: Content;\n}",
    ))
    .unwrap();
    // The core's two types are declared after the surface's own.
    assert_eq!(
        declaration(&[&d], &|_| true).unwrap(),
        format!(
            "declare namespace demo {{\n  /** One shot. */\n  interface Shot {{\n    /** The picture. */\n    image: Image;\n    text?: Content;\n  }}\n}}\n{CONTENT_DECLARATION}\n\
             declare const clax: {{\n  demo: {{\n    /** Shows it. */\n    shot(of?: Content): Promise<Image>;\n    /** Asks. (writes) (may take long) */\n    ask(): Promise<Instruction | null>;\n    /** Lists. */\n    all(): Promise<demo.Shot[]>;\n  }};\n}};\n"
        )
    );
    // A run that has none of the operations that name them reads neither.
    let none = declaration(&[&d], &|op| op == "demo.ask").unwrap();
    assert!(!none.contains("interface Content"), "{none}");
}
