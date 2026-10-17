//! Where an agent should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1): a suggestion, from what the
//! daemon knows of where that person last engaged, of the surface (the
//! terminal or Clax), the narrowest place in Clax, and how sure it is.
//! Pure: the caller gathers the signals by the daemon's clock and passes
//! the time. The windows are defaults to tune with use (§ "Settled" 1).

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// Newest signals on the two sides closer than this many seconds are a
/// tie: unsure.
pub const TIE_SECS: i64 = 60;
/// With no signal on either side within this many seconds, the suggestion
/// is unsure.
pub const QUIET_SECS: i64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Clax,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Clear,
    Unsure,
}

/// The narrowest place in Clax: a thread of an artifact, an artifact, or
/// neither (the inbox).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Place {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
}

/// Where to ask. Whatever the surface, `where` is the place in Clax a
/// question, or the terminal card of one asked in the terminal, goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub surface: Surface,
    pub r#where: Place,
    pub confidence: Confidence,
    /// Why, in a sentence the agent may weigh against what it knows.
    pub why: String,
}

/// The person's last prompt, or inferred turn, to the session in the
/// terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSignal {
    pub at: DateTime<Utc>,
    /// A turn with no Clax delivery behind it stood in for a prompt the
    /// harness does not report.
    pub inferred: bool,
}

/// The session's last delivery from Clax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub at: DateTime<Utc>,
    pub artifact_id: String,
    /// Its thread, when it still is one of `artifact_id`'s.
    pub thread_id: Option<String>,
}

/// What the daemon knows of a session's person, now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub terminal: Option<TerminalSignal>,
    pub delivery: Option<Delivery>,
    /// The person's newest input, or focused report, on a tab of the
    /// session's pages: an unfocused tab gives only its last input; a
    /// hidden tab with no input gives none.
    pub clax_at: Option<DateTime<Utc>>,
    /// The session's page, which places a question when no delivery does.
    pub artifact_id: Option<String>,
    /// A Clax tab or the extension's panel of the person's can show a
    /// question now.
    pub clax_open: bool,
    /// The extension's side panel is open (its focus is not known).
    pub panel_open: bool,
}

/// The place: the delivery's thread (else artifact) when Clax instigated
/// the current work, else the session's page.
fn place_of(s: &Signals) -> Place {
    match &s.delivery {
        Some(d) if s.terminal.as_ref().is_none_or(|t| d.at >= t.at) => Place {
            thread_id: d.thread_id.clone(),
            artifact_id: Some(d.artifact_id.clone()),
        },
        _ => Place {
            thread_id: None,
            artifact_id: s.artifact_id.clone(),
        },
    }
}

/// The suggestion for `s` at `now`. The newest engagement wins; signals a
/// minute apart, no signal in five minutes, or an inferred turn with no
/// Clax signal to weigh it against are unsure, and unsure leans to Clax.
/// An inferred turn a minute or more newer than the newest Clax signal
/// gives the terminal, as a prompt would. With no Clax tab or panel open,
/// Clax cannot show a question: the terminal, clear. The `why` speaks of
/// the person in the third person, for the agent, and names an open panel
/// whatever the suggestion.
pub fn suggest(s: &Signals, now: DateTime<Utc>) -> Suggestion {
    let place = place_of(s);
    let make = |surface, confidence, why: String| Suggestion {
        surface,
        r#where: place.clone(),
        confidence,
        why,
    };
    if !s.clax_open {
        return make(
            Surface::Terminal,
            Confidence::Clear,
            "No Clax tab or panel of the person's is open, so Clax cannot show the question."
                .into(),
        );
    }
    let panel = if s.panel_open {
        " The extension's panel is open."
    } else {
        ""
    };
    let clax = s
        .clax_at
        .into_iter()
        .chain(s.delivery.as_ref().map(|d| d.at))
        .max();
    let term = s.terminal.as_ref();
    let recent = |t: DateTime<Utc>| now - t <= Duration::seconds(QUIET_SECS);
    let ago = |t: DateTime<Utc>| (now - t).num_seconds().max(0);
    let tie = Duration::seconds(TIE_SECS);
    // The terminal side in words: a prompt, or a turn standing in for one.
    let in_terminal = |t: &TerminalSignal| {
        if t.inferred {
            format!(
                "the person's last terminal turn on this session was {} s ago (inferred: this harness reports no prompts)",
                ago(t.at)
            )
        } else {
            format!(
                "the person typed to this session in the terminal {} s ago",
                ago(t.at)
            )
        }
    };
    let quiet = || {
        make(
            Surface::Clax,
            Confidence::Unsure,
            format!(
                "Neither the terminal nor Clax has seen the person in the last {} minutes; Clax is the default.{panel}",
                QUIET_SECS / 60
            ),
        )
    };
    if !term.is_some_and(|t| recent(t.at)) && !clax.is_some_and(recent) {
        return quiet();
    }
    match (term, clax) {
        (Some(t), None) if t.inferred => make(
            Surface::Clax,
            Confidence::Unsure,
            format!(
                "The person's last terminal turn, {} s ago, is inferred: this harness reports no prompts, and Clax has seen nothing of the person.{panel}",
                ago(t.at)
            ),
        ),
        (Some(t), c) if c.is_none_or(|c| t.at > c) => match c {
            Some(c) if t.at - c < tie => make(
                Surface::Clax,
                Confidence::Unsure,
                format!(
                    "{}, and the person was active in Clax {} s ago, too close to tell.{panel}",
                    capitalized(&in_terminal(t)),
                    ago(c)
                ),
            ),
            Some(c) => make(
                Surface::Terminal,
                Confidence::Clear,
                format!(
                    "{}, after the person's last activity in Clax, {} s ago.{panel}",
                    capitalized(&in_terminal(t)),
                    ago(c)
                ),
            ),
            None => make(
                Surface::Terminal,
                Confidence::Clear,
                format!(
                    "{}; Clax has seen nothing of the person.{panel}",
                    capitalized(&in_terminal(t))
                ),
            ),
        },
        (t, Some(c)) => match t {
            Some(t) if c - t.at < tie => make(
                Surface::Clax,
                Confidence::Unsure,
                format!(
                    "The person was active in Clax {} s ago and {}, too close to tell.{panel}",
                    ago(c),
                    in_terminal(t)
                ),
            ),
            Some(t) => make(
                Surface::Clax,
                Confidence::Clear,
                format!(
                    "The person was active on this session's page in Clax {} s ago; before that, {}.{panel}",
                    ago(c),
                    in_terminal(t)
                ),
            ),
            None => make(
                Surface::Clax,
                Confidence::Clear,
                format!(
                    "The person was active on this session's page in Clax {} s ago; the terminal has seen no prompt or turn from the person.{panel}",
                    ago(c)
                ),
            ),
        },
        (_, None) => quiet(),
    }
}

/// `s` with its first letter in upper case.
fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// `s` as one line of a delivery's text.
pub fn line(s: &Suggestion) -> String {
    let surface = match (s.surface, &s.r#where.thread_id) {
        (Surface::Terminal, _) => "in the terminal (chat)".to_string(),
        (Surface::Clax, Some(t)) => format!("in Clax, in thread {t}"),
        (Surface::Clax, None) => "in Clax".to_string(),
    };
    let sure = match s.confidence {
        Confidence::Clear => "clear",
        Confidence::Unsure => "unsure",
    };
    format!(
        "[clax] If you ask the person anything now, ask {surface} ({sure}): {}",
        s.why
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-10-16T10:10:00Z";

    fn at(secs_ago: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(NOW)
            .unwrap()
            .with_timezone(&Utc)
            - Duration::seconds(secs_ago)
    }
    fn now() -> DateTime<Utc> {
        at(0)
    }
    fn open() -> Signals {
        Signals {
            clax_open: true,
            artifact_id: Some("7q3k9mzx2b4t".into()),
            ..Signals::default()
        }
    }
    fn typed(secs_ago: i64) -> Option<TerminalSignal> {
        Some(TerminalSignal {
            at: at(secs_ago),
            inferred: false,
        })
    }
    fn sent(secs_ago: i64, thread: Option<&str>) -> Option<Delivery> {
        Some(Delivery {
            at: at(secs_ago),
            artifact_id: "7q3k9mzx2b4t".into(),
            thread_id: thread.map(Into::into),
        })
    }
    fn says(s: &Signals) -> (Surface, Confidence) {
        let x = suggest(s, now());
        (x.surface, x.confidence)
    }

    #[test]
    fn with_no_clax_open_the_terminal_is_clear_whatever_else() {
        let s = Signals {
            clax_open: false,
            clax_at: Some(at(1)),
            ..open()
        };
        let x = suggest(&s, now());
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Terminal, Confidence::Clear)
        );
        assert!(x.why.contains("No Clax tab or panel"), "{}", x.why);
    }

    #[test]
    fn a_prompt_newer_than_any_clax_input_is_the_terminal() {
        assert_eq!(
            says(&Signals {
                terminal: typed(10),
                ..open()
            }),
            (Surface::Terminal, Confidence::Clear)
        );
        assert_eq!(
            says(&Signals {
                terminal: typed(10),
                clax_at: Some(at(200)),
                ..open()
            }),
            (Surface::Terminal, Confidence::Clear)
        );
    }

    #[test]
    fn clax_input_or_focus_newer_than_the_prompt_is_clax() {
        let x = suggest(
            &Signals {
                terminal: typed(200),
                clax_at: Some(at(10)),
                ..open()
            },
            now(),
        );
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Clax, Confidence::Clear)
        );
        assert_eq!(
            x.r#where,
            Place {
                thread_id: None,
                artifact_id: Some("7q3k9mzx2b4t".into())
            }
        );
    }

    #[test]
    fn a_delivery_from_clax_is_clax_at_its_thread() {
        let x = suggest(
            &Signals {
                terminal: typed(200),
                delivery: sent(10, Some("t1")),
                ..open()
            },
            now(),
        );
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Clax, Confidence::Clear)
        );
        assert_eq!(x.r#where.thread_id.as_deref(), Some("t1"));
    }

    #[test]
    fn work_instigated_in_the_terminal_is_placed_on_the_artifact_not_an_older_thread() {
        let x = suggest(
            &Signals {
                terminal: typed(5),
                delivery: sent(100, Some("t1")),
                ..open()
            },
            now(),
        );
        assert_eq!(x.surface, Surface::Terminal);
        assert_eq!(
            x.r#where,
            Place {
                thread_id: None,
                artifact_id: Some("7q3k9mzx2b4t".into())
            }
        );
    }

    #[test]
    fn signals_within_a_minute_of_each_other_are_unsure_and_lean_to_clax() {
        for (t, c) in [(10, 69), (69, 10), (30, 30)] {
            assert_eq!(
                says(&Signals {
                    terminal: typed(t),
                    clax_at: Some(at(c)),
                    ..open()
                }),
                (Surface::Clax, Confidence::Unsure),
                "terminal {t} s ago, Clax {c} s ago"
            );
        }
        assert_eq!(
            says(&Signals {
                terminal: typed(10),
                clax_at: Some(at(70)),
                ..open()
            }),
            (Surface::Terminal, Confidence::Clear),
            "a minute apart is not a tie"
        );
    }

    #[test]
    fn nothing_in_five_minutes_is_unsure_and_leans_to_clax() {
        let x = suggest(
            &Signals {
                terminal: typed(QUIET_SECS + 1),
                clax_at: Some(at(QUIET_SECS + 400)),
                ..open()
            },
            now(),
        );
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Clax, Confidence::Unsure)
        );
        assert!(x.why.contains("5 minutes"), "{}", x.why);
        assert_eq!(
            says(&open()),
            (Surface::Clax, Confidence::Unsure),
            "no signal at all"
        );
        assert_eq!(
            says(&Signals {
                terminal: typed(QUIET_SECS),
                ..open()
            }),
            (Surface::Terminal, Confidence::Clear),
            "exactly five minutes is still recent"
        );
    }

    #[test]
    fn an_inferred_turn_alone_is_unsure() {
        let s = Signals {
            terminal: Some(TerminalSignal {
                at: at(5),
                inferred: true,
            }),
            ..open()
        };
        let x = suggest(&s, now());
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Clax, Confidence::Unsure)
        );
        assert!(x.why.contains("inferred"), "{}", x.why);
        let older_clax = Signals {
            clax_at: Some(at(200)),
            ..s.clone()
        };
        let x = suggest(&older_clax, now());
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Terminal, Confidence::Clear),
            "an inferred turn a minute or more after a Clax signal decides, as a prompt would"
        );
        assert!(x.why.contains("inferred"), "{}", x.why);
        let tied_clax = Signals {
            clax_at: Some(at(30)),
            ..s.clone()
        };
        assert_eq!(says(&tied_clax), (Surface::Clax, Confidence::Unsure));
        let newer_clax = Signals {
            clax_at: Some(at(1)),
            terminal: Some(TerminalSignal {
                at: at(100),
                inferred: true,
            }),
            ..open()
        };
        assert_eq!(says(&newer_clax), (Surface::Clax, Confidence::Clear));
    }

    #[test]
    fn the_panel_is_named_but_never_decides() {
        let s = Signals {
            panel_open: true,
            ..open()
        };
        let x = suggest(&s, now());
        assert_eq!(x.confidence, Confidence::Unsure);
        assert!(x.why.contains("panel"), "{}", x.why);
        let x = suggest(
            &Signals {
                terminal: typed(5),
                ..s
            },
            now(),
        );
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Terminal, Confidence::Clear)
        );
        assert!(
            x.why.contains("panel"),
            "a clear why names it too: {}",
            x.why
        );
    }

    #[test]
    fn a_side_with_no_signal_is_never_said_to_have_one() {
        let x = suggest(
            &Signals {
                terminal: typed(10),
                ..open()
            },
            now(),
        );
        assert_eq!(x.surface, Surface::Terminal);
        assert!(!x.why.contains("after"), "{}", x.why);
        assert!(
            x.why.contains("Clax has seen nothing of the person"),
            "{}",
            x.why
        );

        let x = suggest(
            &Signals {
                clax_at: Some(at(10)),
                ..open()
            },
            now(),
        );
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Clax, Confidence::Clear)
        );
        assert!(!x.why.contains("after"), "{}", x.why);
        assert!(
            x.why.contains("the terminal has seen no prompt or turn"),
            "{}",
            x.why
        );

        let x = suggest(
            &Signals {
                clax_at: Some(at(10)),
                terminal: Some(TerminalSignal {
                    at: at(200),
                    inferred: true,
                }),
                ..open()
            },
            now(),
        );
        assert_eq!(
            (x.surface, x.confidence),
            (Surface::Clax, Confidence::Clear)
        );
        assert!(
            !x.why.contains("last prompt"),
            "an inferred turn is no prompt: {}",
            x.why
        );
        assert!(x.why.contains("inferred"), "{}", x.why);
    }

    #[test]
    fn every_why_speaks_of_the_person_to_the_agent() {
        let inferred = |secs_ago| {
            Some(TerminalSignal {
                at: at(secs_ago),
                inferred: true,
            })
        };
        let cases = [
            Signals {
                clax_open: false,
                ..open()
            },
            open(),
            Signals {
                terminal: typed(10),
                ..open()
            },
            Signals {
                terminal: typed(10),
                clax_at: Some(at(200)),
                ..open()
            },
            Signals {
                terminal: typed(10),
                clax_at: Some(at(30)),
                ..open()
            },
            Signals {
                terminal: typed(200),
                clax_at: Some(at(10)),
                ..open()
            },
            Signals {
                terminal: typed(30),
                clax_at: Some(at(10)),
                ..open()
            },
            Signals {
                clax_at: Some(at(10)),
                ..open()
            },
            Signals {
                terminal: inferred(5),
                ..open()
            },
            Signals {
                terminal: inferred(5),
                clax_at: Some(at(200)),
                ..open()
            },
            Signals {
                terminal: inferred(200),
                clax_at: Some(at(10)),
                ..open()
            },
        ];
        for s in cases {
            for panel_open in [false, true] {
                let why = suggest(
                    &Signals {
                        panel_open,
                        ..s.clone()
                    },
                    now(),
                )
                .why;
                let words: Vec<String> = why
                    .split(|c: char| !c.is_alphanumeric() && c != '\'')
                    .map(str::to_lowercase)
                    .collect();
                assert!(
                    !words
                        .iter()
                        .any(|w| ["you", "your", "yours"].contains(&w.as_str())),
                    "{why}"
                );
                // An open panel is a Clax surface: `clax_open` holds whenever it does.
                if s.clax_open {
                    assert_eq!(why.contains("panel is open"), panel_open, "{why}");
                }
            }
        }
    }

    #[test]
    fn a_session_with_no_page_names_no_place() {
        let x = suggest(
            &Signals {
                artifact_id: None,
                ..open()
            },
            now(),
        );
        assert_eq!(x.r#where, Place::default());
        assert_eq!(
            serde_json::to_value(&x).unwrap()["where"],
            serde_json::json!({})
        );
    }

    #[test]
    fn it_serializes_as_the_contract_names_it_and_reads_as_one_line() {
        let x = suggest(
            &Signals {
                terminal: typed(200),
                delivery: sent(10, Some("t1")),
                ..open()
            },
            now(),
        );
        let v = serde_json::to_value(&x).unwrap();
        assert_eq!(v["surface"], "clax");
        assert_eq!(v["confidence"], "clear");
        assert_eq!(
            v["where"],
            serde_json::json!({"thread_id": "t1", "artifact_id": "7q3k9mzx2b4t"})
        );
        assert!(
            line(&x).starts_with(
                "[clax] If you ask the person anything now, ask in Clax, in thread t1 (clear): "
            ),
            "{}",
            line(&x)
        );
    }
}
