//! What a run of Clax's own kind is sent when a join bound a host's
//! surface into it (`crate::joins`; [`super::system_of`] for a kind of
//! Clax's): Clax's own prompt, its declaration holding the joined
//! surface's operations beside Clax's, and the joined surface's rules
//! under its name as the prompt's last section. With no joined surface,
//! or one that gives the run no operation or whose definition is not
//! known, the prompt is a run's with no join, byte for byte.

use super::tests::host_surface;
use super::*;
use crate::golden::golden;

/// The host protocol's contract fixture, whose `files` hold the
/// surface `desk`.
const CONTRACT: &str = include_str!("../../../../docs/claxbot-surfaces/contract.json");

fn desk() -> Definition {
    let fixture: Value = serde_json::from_str(CONTRACT).unwrap();
    host_surface(fixture["files"]["desk.d.ts"].as_str().unwrap())
}

/// A clax run of `kind`'s binding of Clax's own surface, as a joined
/// run is made with it ([`crate::joins::clax_binding`]), on web grant
/// `web`.
fn clax(web: &str) -> Binding {
    let stored = crate::joins::clax_binding("A1", web);
    Binding {
        surface: CLAX_SURFACE.into(),
        hash: None,
        scope: stored.scope().clone(),
        operations: stored.operations().unwrap().to_vec(),
        limits: crate::surface::SurfaceLimits::default(),
    }
}

fn binding(surface: &str, ops: &[&str]) -> Binding {
    Binding {
        surface: surface.into(),
        hash: Some("h".into()),
        scope: Value::Null,
        operations: ops.iter().map(ToString::to_string).collect(),
        limits: crate::surface::SurfaceLimits::default(),
    }
}

fn run_of(kind: &str, bindings: Vec<Binding>) -> RunInfo {
    RunInfo {
        id: "R1".into(),
        person: "person".into(),
        model: "model".into(),
        kind: kind.into(),
        request: json!({"thread_ids": ["T1"]}),
        bindings,
    }
}

const SHAPES: [(&str, WebShape); 4] = [
    (
        "",
        WebShape {
            search: false,
            fetch: false,
        },
    ),
    (
        "search",
        WebShape {
            search: true,
            fetch: false,
        },
    ),
    (
        "fetch",
        WebShape {
            search: false,
            fetch: true,
        },
    ),
    (
        "search,fetch",
        WebShape {
            search: true,
            fetch: true,
        },
    ),
];

/// A thread run that the fixture's `desk` joined with the operations a
/// join gives it while Clax stores no content (every one but the
/// snapshot, an `Image`) is sent Clax's prompt with `desk` declared on
/// `clax` after Clax's own, the core's words on a host's failures, and
/// `# desk` with its rules as the last section.
#[test]
fn a_thread_run_desk_joined_is_sent_claxs_prompt_with_desk() {
    let desk = desk();
    let ops: Vec<&str> = desk
        .joined_operations(&clax_surface::RunScope::default())
        .granted;
    assert_eq!(
        ops,
        ["desk.lamps", "desk.settings.set", "desk.notify", "desk.ask"]
    );
    let clax_def = crate::api::definition();
    for kind in ["thread", "brief"] {
        let run = run_of(
            &format!("clax.{kind}"),
            vec![clax(""), binding("desk", &ops)],
        );
        let bound: Vec<(&Binding, &Definition)> =
            vec![(&run.bindings[0], clax_def), (&run.bindings[1], &desk)];
        let system = system_of(&run, clax_def, &bound, None).unwrap();
        assert!(
            system.ends_with("\n\nA desk and what is on it. Ask before you move anything.\n"),
            "{system}"
        );
        // The desk's section says first that its operations act outside
        // Clax, and who alone is reason to call a write.
        assert!(
            system.contains(
                "```\n\n# desk\n\nThe operations on `clax.desk` act on the person's own \
                 machine or product, outside Clax."
            ),
            "{system}"
        );
        let clax_at = system.find("\n  files: {\n").expect("Clax's own");
        let desk_at = system.find("\n  desk: {\n").expect("the desk's");
        assert!(clax_at < desk_at, "{system}");
        assert!(!system.contains("snapshot"), "{system}");
        assert!(
            system.contains(standard::HOST_NOTE.trim_start()),
            "{system}"
        );
        // Its pieces name what it was made of: the file, then the
        // slots, `surfaces` among them.
        let pieces = system_pieces_of(&run, clax_def, &bound, None).unwrap();
        assert_eq!(pieces[0].name, PROMPT_FILE_PIECE);
        assert!(pieces.iter().any(|p| p.name == "surfaces"), "{pieces:?}");
        if kind == "thread" {
            golden(
                "the system prompt of a clax.thread run the desk joined",
                "src/prompt.system.thread.joined-desk.golden",
                &system,
            );
        }
    }
}

/// With no joined surface, a joined surface with no operation, or one
/// whose definition is not its own (its provider is away), a run of
/// Clax's own kind is sent what a run with no join is, for each web
/// grant, and so are its pieces; its `clax` binding of version 2 is the
/// one of version 1.
#[test]
fn a_run_of_claxs_kind_with_nothing_joined_is_sent_what_it_was() {
    let desk = desk();
    let clax_def = crate::api::definition();
    let away = &*crate::registry::NOTHING;
    for (web, shape) in SHAPES {
        for kind in [RequestKind::Thread, RequestKind::Brief] {
            let name = format!("clax.{}", kind.as_str());
            for others in [
                vec![],
                vec![(binding("desk", &[]), &desk)],
                vec![(binding("desk", &["desk.lamps"]), away)],
            ] {
                let mut bindings = vec![clax(web)];
                bindings.extend(others.iter().map(|(b, _)| b.clone()));
                let run = run_of(&name, bindings);
                let mut bound: Vec<(&Binding, &Definition)> = vec![(&run.bindings[0], clax_def)];
                for (n, (_, def)) in others.iter().enumerate() {
                    bound.push((&run.bindings[n + 1], *def));
                }
                assert_eq!(
                    system_of(&run, clax_def, &bound, None).unwrap(),
                    system_prompt(kind, shape),
                    "{name} {web:?}"
                );
                assert_eq!(
                    system_pieces_of(&run, clax_def, &bound, None).unwrap(),
                    system_pieces(kind, shape),
                    "{name} {web:?}"
                );
            }
        }
    }
}
