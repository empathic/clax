//! What a standing join binds into a run of Clax's own kind
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, "A
//! run": "One with a standing join"; the store's rows are
//! `clax_store::bot_joins`). A host that registered a surface says which
//! of Clax's scopes it joins (`PUT /api/bot/surfaces/<name>/joins`); a
//! thread run (a thread sent to ClaxBot, or a follow-up) started on such
//! an artifact while the host is
//! connected is made with a second binding, of the host's surface, and so
//! with bindings of version 2 ([`clax_store::bot_runs::Bindings::bound`]):
//!
//! - its `clax` binding holds the operations a run of the same web grant
//!   has with no join ([`clax_binding`]), so what Clax's own surface does
//!   in it is what it does in any run;
//! - each joined binding holds the surface's definition by hash, the
//!   registration that serves it, no prompt file (the run is sent
//!   Clax's prompt) and the operations the join gives it
//!   ([`joined_binding`]) and the registration's settings, fixed when
//!   the run is made, as a host's own run keeps its binding.
//!
//! A run with no join is made as it always was, with bindings of version
//! 1, and nothing here is asked.

use crate::api;
use crate::bot_surfaces::{Snapshot, allow_patterns};
use crate::surface::Binding;
use crate::web::WebShape;
use clax_store::bot_runs::{Bindings, CLAX_SURFACE, StoredBinding};
use clax_surface::{Access, Limits, RunScope};
use serde_json::Value;

/// The most surfaces a join binds into one run: the design's eight
/// surfaces a run, less Clax's own.
pub const MAX_JOINED: usize = 7;

/// The `clax` binding of a run of Clax's own kind on artifact
/// `artifact_id` whose web grant has the shape `web` ([`clax_store::bot_runs::Run::web`]),
/// as a binding of version 2: every operation the run would have with
/// bindings of version 1 ([`Bindings::clax`], read as
/// [`Binding::from_stored`] reads it with Clax's standing rule), listed,
/// on the scope `{"artifact": "<ID>"}`, with no hash, registration or
/// prompt file.
#[must_use]
pub fn clax_binding(artifact_id: &str, web: &str) -> StoredBinding {
    let v1 = Bindings::clax(artifact_id, web);
    let stored = v1.of(CLAX_SURFACE).expect("a clax run's one binding");
    let had = Binding::from_stored(stored, api::definition(), |op| {
        api::declared(op, WebShape::default())
    });
    StoredBinding::bound(
        CLAX_SURFACE,
        None,
        None,
        None,
        stored.scope().clone(),
        had.operations,
    )
}

/// The binding a join by `registration`, held now as `snapshot`, gives a
/// run of Clax's own kind: the operations of its definition that the
/// registration's `allow` setting and the join's own allow patterns
/// `allow` both name ([`clax_surface::Definition::joined_operations`]),
/// less those the hub withholds for content while it keeps none
/// (`snapshot.withheld`, [`crate::bot_surfaces::withheld`]): a daemon
/// that keeps content binds a join's content-bearing operations as it
/// gives them to a run the registration starts. A join that gives
/// no `allow` (`None`) names the definition's reads (`@read`) that do
/// not wait (`@waits`) alone: a write or an effect, which acts on the
/// person's machine or product outside Clax, and a read that waits,
/// which asks the person there and parks the run, are bound only when
/// the join's own patterns name them, by name or by a pattern written
/// in that join;
/// on `scope` (what the run is on, [`joined_scope`]), with the
/// definition's hash, no prompt file, and the
/// registration's settings as `snapshot` has them, kept with the run
/// ([`crate::outside::kept_with_run`]) as a host's own run keeps them:
/// its effect limit and call deadlines are those in force when the run
/// was made, over a restart too. `None`
/// when that is no operation, or when either list holds what is no
/// pattern within `limits`: such a join binds nothing.
#[must_use]
pub fn joined_binding(
    registration: &str,
    snapshot: &Snapshot,
    allow: Option<&[String]>,
    limits: &Limits,
    scope: &Value,
) -> Option<StoredBinding> {
    let operator = snapshot
        .settings
        .allow
        .as_deref()
        .map(|list| allow_patterns(list, limits))
        .transpose()
        .ok()?;
    let join = allow
        .map(|list| allow_patterns(list, limits))
        .transpose()
        .ok()?;
    let ops = snapshot.definition.joined_operations(&RunScope {
        operator: operator.as_deref(),
        starter: join.as_deref(),
        content: true,
    });
    let prefix = format!("{}.", snapshot.name);
    let held_back = |op: &str| {
        let op = op.strip_prefix(&prefix).unwrap_or(op);
        snapshot.withheld.iter().any(|w| w.op == op)
    };
    let granted: Vec<String> = ops
        .granted
        .iter()
        .filter(|op| !held_back(op))
        .filter(|op| {
            allow.is_some()
                || snapshot
                    .definition
                    .operation(op)
                    .is_some_and(|o| o.op.access == Access::Read && !o.op.waits)
        })
        .map(|op| (*op).to_string())
        .collect();
    if granted.is_empty() {
        return None;
    }
    Some(
        StoredBinding::bound(
            snapshot.name.as_str(),
            Some(snapshot.hash.clone()),
            Some(registration.to_string()),
            None,
            scope.clone(),
            granted,
        )
        .with_settings(crate::outside::kept_with_run(&snapshot.settings)),
    )
}

/// The scope of a joined binding of a run of Clax's own kind on
/// `artifact_id`, started for the threads `thread_ids`: `{"clax":
/// {"artifact": "<ID>", "thread_ids": [...]}}`, each surface's scope as
/// a host names it in a start's `bind`
/// ([`clax_surface::wire::StartRunRequest::bind`]). Its host is sent it
/// with each call ([`clax_surface::wire::Call::joined`]).
#[must_use]
pub fn joined_scope(artifact_id: &str, thread_ids: &[String]) -> Value {
    serde_json::json!({"clax": {"artifact": artifact_id, "thread_ids": thread_ids}})
}

/// The bindings of a run of Clax's own kind on `artifact_id`, with web
/// grant `web`, that the joins' bindings `joined` were bound into: its
/// `clax` binding ([`clax_binding`]) first, then the first
/// [`MAX_JOINED`] of `joined` in their order; those past it are left out
/// and named in the log. `None` for no joined binding, and for a set
/// that makes no bindings ([`Bindings::bound`]: two of one surface, say),
/// which is logged: the run is made with bindings of version 1, as a run
/// with no join is, and is not refused for its joins.
#[must_use]
pub fn bindings_with(artifact_id: &str, web: &str, joined: Vec<StoredBinding>) -> Option<Bindings> {
    if joined.is_empty() {
        return None;
    }
    if joined.len() > MAX_JOINED {
        let left: Vec<String> = joined[MAX_JOINED..]
            .iter()
            .map(|b| format!("{} ({})", b.surface(), b.registration().unwrap_or("-")))
            .collect();
        tracing::warn!(
            "clax-bot: a run on {artifact_id} joins {MAX_JOINED} surfaces at the most; not \
             bound: {}",
            left.join(", ")
        );
    }
    let mut all = vec![clax_binding(artifact_id, web)];
    all.extend(joined.into_iter().take(MAX_JOINED));
    match Bindings::bound(all) {
        Ok(bindings) => Some(bindings),
        Err(e) => {
            tracing::warn!(
                "clax-bot: the joins on {artifact_id} make no bindings ({e}); the run is made \
                 with no join"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_surface::Reserved;
    use clax_surface::wire::Settings;
    use std::sync::Arc;

    /// A run's `clax` binding of version 2 has the operations the same
    /// run's binding of version 1 gives it, for every web grant.
    #[test]
    fn the_clax_binding_lists_what_a_run_of_its_web_grant_has() {
        let def = api::definition();
        for web in ["", "search", "fetch", "search,fetch"] {
            let v1 = Bindings::clax("a1", web);
            let had = Binding::from_stored(v1.of(CLAX_SURFACE).unwrap(), def, |op| {
                api::declared(op, WebShape::default())
            });
            let v2 = clax_binding("a1", web);
            assert_eq!(v2.operations(), Some(had.operations.as_slice()), "{web:?}");
            assert_eq!(v2.scope(), &serde_json::json!({"artifact": "a1"}));
            assert_eq!(
                (v2.hash(), v2.registration(), v2.prompt()),
                (None, None, None)
            );
            assert_eq!(
                v2.operations().unwrap().iter().any(|o| o == "web.search"),
                web.contains("search")
            );
        }
    }

    fn desk(allow: Option<Vec<String>>) -> Snapshot {
        let source = "/** Keep the desk tidy. */\ndeclare const desk: {\n  /** The lamps. @read */\n  \
                      lamps(): Promise<string[]>;\n  /** Turns one off. @write */\n  \
                      off(lamp: string): Promise<null>;\n};\n";
        let reserved = Reserved::of(&[api::definition()]);
        let author = clax_surface::Author::Host {
            reserved: &reserved,
        };
        let definition = clax_surface::parse(source, author, &Limits::default()).unwrap();
        Snapshot {
            name: "desk".into(),
            definition: Arc::new(definition),
            hash: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
            prompt: None,
            prompt_hash: None,
            settings: Settings {
                allow,
                ..Settings::default()
            },
            withheld: Vec::new(),
        }
    }

    /// A join's operation whose types hold content follows the hub's
    /// content switch, as a run the registration starts does: withheld
    /// (the registration's `withheld` names it) while the daemon keeps
    /// no content, bound once it does.
    #[test]
    fn a_joins_content_operation_is_bound_as_the_hub_withholds_it() {
        let source = "/** Keep the desk tidy. */\ndeclare const desk: {\n  /** The lamps. @read */\n  \
                      lamps(): Promise<string[]>;\n  /** A photo of the desk. @read */\n  \
                      photo(): Promise<Image>;\n};\n";
        let reserved = Reserved::of(&[api::definition()]);
        let author = clax_surface::Author::Host {
            reserved: &reserved,
        };
        let definition = clax_surface::parse(source, author, &Limits::default()).unwrap();
        let snapshot = |content: bool| Snapshot {
            withheld: crate::bot_surfaces::withheld(&definition, content),
            definition: Arc::new(definition.clone()),
            ..desk(None)
        };
        let limits = Limits::default();
        let bound = |content: bool| {
            joined_binding("r1", &snapshot(content), None, &limits, &Value::Null)
                .unwrap()
                .operations()
                .unwrap()
                .to_vec()
        };
        assert_eq!(
            snapshot(false)
                .withheld
                .iter()
                .map(|w| w.op.as_str())
                .collect::<Vec<_>>(),
            ["photo"]
        );
        assert_eq!(bound(false), ["desk.lamps"]);
        assert_eq!(bound(true), ["desk.lamps", "desk.photo"]);
    }

    /// A read that waits asks the person on their machine and parks the
    /// run: a join with no `allow` does not bind it, and one that names
    /// it does.
    #[test]
    fn a_join_with_no_allow_binds_no_read_that_waits() {
        let source = "/** Keep the desk tidy. */\ndeclare const desk: {\n  /** The lamps. @read */\n  \
                      lamps(): Promise<string[]>;\n  /** Asks the person on their screen. @read @waits */\n  \
                      ask(question: string): Promise<Instruction>;\n};\n";
        let reserved = Reserved::of(&[api::definition()]);
        let author = clax_surface::Author::Host {
            reserved: &reserved,
        };
        let definition = clax_surface::parse(source, author, &Limits::default()).unwrap();
        let snapshot = Snapshot {
            definition: Arc::new(definition),
            ..desk(None)
        };
        let limits = Limits::default();
        let ops = |allow: Option<&[String]>| {
            joined_binding("r1", &snapshot, allow, &limits, &Value::Null)
                .unwrap()
                .operations()
                .unwrap()
                .to_vec()
        };
        assert_eq!(ops(None), ["desk.lamps"]);
        assert_eq!(ops(Some(&["desk.ask".to_string()])), ["desk.ask"]);
        assert_eq!(
            ops(Some(&["desk.*".to_string()])),
            ["desk.lamps", "desk.ask"]
        );
    }

    #[test]
    fn a_join_binds_what_the_operator_and_the_join_both_name() {
        let limits = Limits::default();
        let list = |l: &[&str]| l.iter().map(|p| (*p).to_string()).collect::<Vec<_>>();
        // A join that names nothing binds the reads alone.
        let reads = joined_binding("r1", &desk(None), None, &limits, &Value::Null).unwrap();
        assert_eq!(reads.surface(), "desk");
        assert_eq!(reads.registration(), Some("r1"));
        assert_eq!(reads.prompt(), None);
        assert_eq!(reads.scope(), &Value::Null);
        let scope = joined_scope("a1", &["t1".to_string()]);
        let scoped = joined_binding("r1", &desk(None), None, &limits, &scope).unwrap();
        assert_eq!(
            scoped.scope(),
            &serde_json::json!({"clax": {"artifact": "a1", "thread_ids": ["t1"]}})
        );
        assert_eq!(reads.operations().unwrap(), ["desk.lamps"]);
        // A write is bound when the join names it, by its name or by a
        // pattern the join holds.
        for named in [&["desk.lamps", "desk.off"][..], &["desk.*"]] {
            let all = joined_binding("r1", &desk(None), Some(&list(named)), &limits, &Value::Null)
                .unwrap();
            assert_eq!(
                all.operations().unwrap(),
                ["desk.lamps", "desk.off"],
                "{named:?}"
            );
        }
        assert_eq!(reads.settings(), Some(&serde_json::json!({})));
        let read = joined_binding(
            "r1",
            &desk(None),
            Some(&list(&["desk.lamps"])),
            &limits,
            &Value::Null,
        );
        assert_eq!(read.unwrap().operations().unwrap(), ["desk.lamps"]);
        let snap = desk(Some(list(&["desk.off"])));
        assert_eq!(
            joined_binding(
                "r1",
                &snap,
                Some(&list(&["desk.off"])),
                &limits,
                &Value::Null
            )
            .unwrap()
            .operations()
            .unwrap(),
            ["desk.off"]
        );
        // The registration's `allow` gives a write, which a join that
        // names nothing does not bind.
        assert!(joined_binding("r1", &snap, None, &limits, &Value::Null).is_none());
        // What neither names together binds nothing, and nor does a list
        // that is no pattern.
        assert!(
            joined_binding(
                "r1",
                &snap,
                Some(&list(&["desk.lamps"])),
                &limits,
                &Value::Null
            )
            .is_none()
        );
        assert!(
            joined_binding(
                "r1",
                &desk(None),
                Some(&list(&["desk..x"])),
                &limits,
                &Value::Null
            )
            .is_none()
        );
        // The bindings of a joined run: `clax`'s first, the joined ones
        // after, at most seven of them; none joined is no bindings.
        assert!(bindings_with("a1", "", Vec::new()).is_none());
        let many: Vec<StoredBinding> = (0..9)
            .map(|n| {
                StoredBinding::bound(
                    format!("s{n}"),
                    None,
                    Some(format!("r{n}")),
                    None,
                    Value::Null,
                    vec![],
                )
            })
            .collect();
        let made = bindings_with("a1", "", many).unwrap();
        let names: Vec<&str> = made.all().iter().map(StoredBinding::surface).collect();
        assert_eq!(names, ["clax", "s0", "s1", "s2", "s3", "s4", "s5", "s6"]);
    }
}
