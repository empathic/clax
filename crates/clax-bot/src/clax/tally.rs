//! What Clax's own surface reads of a run's committed steps: the fields
//! a script step's session left in its payload (`changed`, `replied`,
//! `published`, `created`, `base`), folded in one pass.
//!
//! # Payload keys read
//!
//! | Step | Keys |
//! |---|---|
//! | Any committed step | `base`, `published.version`: a cancelled script's `stopped` step carries both |
//! | `script` | `changed`, `replied`, `published`, `created`; the step counts as a script |
//!
//! A step that is not committed is not read, nor is any step's attempt
//! number.

use clax_store::bot_runs::{Step, StepKind};
use serde_json::Value;
use std::collections::BTreeSet;

/// What a run's committed steps add up to for Clax's own surface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    /// The threads replied on since a script last changed the draft or
    /// published (in that script or after it): a reply before the work,
    /// such as the plan, leaves the `finish` summary to be posted.
    pub replied: BTreeSet<String>,
    /// The version the draft is based on, when a script moved it (a
    /// publish, a rebase); `None` for the run's own base version.
    pub base: Option<u32>,
    /// The last version the run published.
    pub published: Option<u32>,
    /// A script of the run created an artifact.
    pub created: bool,
    /// Every draft path a script of the run wrote or deleted.
    pub changed: BTreeSet<String>,
    /// How many script steps are committed.
    pub scripts: u32,
}

impl Tally {
    /// The tally of `steps`, in one pass.
    #[must_use]
    pub(crate) fn of(steps: &[Step]) -> Tally {
        let mut t = Tally::default();
        for s in steps.iter().filter(|s| s.committed_at.is_some()) {
            let p = s.payload.as_ref().unwrap_or(&Value::Null);
            if let Some(b) = p["base"].as_u64() {
                t.base = u32::try_from(b).ok();
            }
            if let Some(v) = p["published"]["version"].as_u64() {
                t.published = u32::try_from(v).ok();
            }
            if s.kind != StepKind::Script {
                continue;
            }
            t.scripts += 1;
            // A script that changed the draft or published starts the
            // replies over: one before it (the plan) does not say what
            // was done.
            let wrote =
                p["changed"].as_array().is_some_and(|c| !c.is_empty()) || !p["published"].is_null();
            if wrote {
                t.replied.clear();
            }
            let strings = |v: &Value| -> Vec<String> {
                v.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            };
            t.replied.extend(strings(&p["replied"]));
            t.changed.extend(strings(&p["changed"]));
            t.created |= p["created"].as_bool().unwrap_or(false);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_fixtures::{a_run, open, step};
    use serde_json::json;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn script(n: i64, payload: Value) -> Step {
        step(n, StepKind::Script, payload)
    }

    #[test]
    fn before_any_step_nothing_is_tallied() {
        assert_eq!(Tally::of(&[]), Tally::default());
    }

    #[test]
    fn a_run_is_tallied_step_by_step() {
        let run = a_run();
        // A turn says nothing of the surface, whatever its payload holds.
        assert_eq!(Tally::of(&run[..1]), Tally::default());
        // The script edited, published version 2 and replied after it.
        let after_script = Tally {
            replied: set(&["t1"]),
            base: Some(2),
            published: Some(2),
            created: false,
            changed: set(&["index.html"]),
            scripts: 1,
        };
        assert_eq!(Tally::of(&run[..2]), after_script);
        // Turns and the give-up add nothing.
        assert_eq!(Tally::of(&run), after_script);
    }

    #[test]
    fn a_write_or_a_publish_starts_the_replies_over() {
        let plan = script(1, json!({"replied": ["t1", "t2"]}));
        assert_eq!(
            Tally::of(std::slice::from_ref(&plan)).replied,
            set(&["t1", "t2"])
        );
        let wrote = script(2, json!({"changed": ["a.css"]}));
        assert!(Tally::of(&[plan.clone(), wrote]).replied.is_empty());
        let published = script(2, json!({"published": {"version": 4}}));
        assert!(Tally::of(&[plan.clone(), published]).replied.is_empty());
        // A reply in the script that wrote counts.
        let both = script(2, json!({"changed": ["a.css"], "replied": ["t2"]}));
        assert_eq!(Tally::of(&[plan.clone(), both]).replied, set(&["t2"]));
        // A script that wrote nothing keeps them and adds its own.
        let idle = script(
            2,
            json!({"changed": [], "published": null, "replied": ["t3"]}),
        );
        assert_eq!(Tally::of(&[plan, idle]).replied, set(&["t1", "t2", "t3"]));
    }

    #[test]
    fn a_published_value_of_any_shape_starts_the_replies_over() {
        let plan = script(1, json!({"replied": ["t1"]}));
        for published in [json!({"version": "4"}), json!({}), json!(true), json!(4)] {
            let after = script(2, json!({"published": published}));
            assert!(
                Tally::of(&[plan.clone(), after]).replied.is_empty(),
                "published {published}"
            );
        }
        // An empty list of changed paths does not.
        let after = script(2, json!({"changed": [], "published": null}));
        assert_eq!(Tally::of(&[plan, after]).replied, set(&["t1"]));
    }

    #[test]
    fn changed_paths_and_scripts_add_up_and_created_stays() {
        let run = [
            script(1, json!({"changed": ["b", "a"], "created": true})),
            script(2, json!({"changed": ["a", "c", 7], "created": false})),
            script(3, json!({})),
        ];
        let t = Tally::of(&run);
        assert_eq!(t.changed, set(&["a", "b", "c"]));
        assert_eq!(t.scripts, 3);
        assert!(t.created);
        assert!(!Tally::of(&run[1..]).created);
    }

    #[test]
    fn the_base_and_the_published_version_are_the_last_any_step_left() {
        let run = [
            script(1, json!({"base": 2, "published": {"version": 2}})),
            script(2, json!({"base": null, "published": null})),
            script(3, json!({"base": 5})),
        ];
        let t = Tally::of(&run[..2]);
        assert_eq!((t.base, t.published), (Some(2), Some(2)));
        let t = Tally::of(&run);
        assert_eq!((t.base, t.published), (Some(5), Some(2)));
        // A number that is no version leaves none.
        let big = script(
            4,
            json!({"base": u64::MAX, "published": {"version": u64::MAX}}),
        );
        let t = Tally::of(&[run[0].clone(), big]);
        assert_eq!((t.base, t.published), (None, None));
    }

    #[test]
    fn a_cancelled_scripts_stopped_step_moves_the_base_and_nothing_else() {
        let cancelled = step(
            2,
            StepKind::Stopped,
            json!({"reason": "cancelled", "notice": null, "title": "t", "code": "c",
                   "published": {"version": 3}, "replied": ["t9"], "base": 3,
                   "changed": ["x"], "created": true}),
        );
        let t = Tally::of(&[script(1, json!({"replied": ["t1"]})), cancelled]);
        assert_eq!(
            t,
            Tally {
                replied: set(&["t1"]),
                base: Some(3),
                published: Some(3),
                created: false,
                changed: BTreeSet::new(),
                scripts: 1,
            }
        );
    }

    #[test]
    fn terminal_steps_and_turns_are_not_scripts() {
        let facts = json!({"changed": ["x"], "replied": ["t"], "created": true});
        for kind in [
            StepKind::Turn,
            StepKind::Finish,
            StepKind::GiveUp,
            StepKind::Stopped,
            StepKind::Failed,
        ] {
            assert_eq!(
                Tally::of(&[step(1, kind, facts.clone())]),
                Tally::default(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_step_that_is_not_committed_or_has_no_payload_adds_no_fact() {
        let mut kept = open(1, StepKind::Script, 2);
        kept.payload = Some(json!({"base": 9, "changed": ["x"], "created": true}));
        assert_eq!(Tally::of(&[kept]), Tally::default());
        let mut bare = script(1, Value::Null);
        bare.payload = None;
        assert_eq!(
            Tally::of(&[bare]),
            Tally {
                scripts: 1,
                ..Tally::default()
            }
        );
    }

    #[test]
    fn an_attempt_number_changes_nothing() {
        let run = a_run();
        let mut again = run.clone();
        for s in &mut again {
            s.attempt += 2;
        }
        assert_eq!(Tally::of(&again), Tally::of(&run));
    }
}
