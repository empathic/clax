//! Tests of [`crate::golden`]. They are a module of the library and not
//! of `src/golden.rs`, which the integration test includes too: here
//! they are compiled and run once.

use crate::golden::{first_difference, rewrite_command};

#[test]
fn a_short_line_that_differs_is_shown_whole() {
    assert_eq!(
        first_difference("a\nb\nc", "a\nB\nc"),
        "line 2\n  after:  \"a\"\n  golden: \"b\"\n  now:    \"B\""
    );
    assert_eq!(
        first_difference("a\n", "a\nb\n"),
        "line 2\n  after:  \"a\"\n  golden: \"\"\n  now:    \"b\""
    );
    assert_eq!(
        first_difference("a", "a\nb"),
        "line 2\n  after:  \"a\"\n  golden: (no such line)\n  now:    \"b\""
    );
    assert_eq!(
        first_difference("a\nb", "a"),
        "line 2\n  after:  \"a\"\n  golden: \"b\"\n  now:    (no such line)"
    );
    // The two lines before the difference, and no more; none before line 1.
    assert_eq!(
        first_difference("a\nb\nc\nd", "a\nb\nc\nD"),
        "line 4\n  after:  \"b\"\n  after:  \"c\"\n  golden: \"d\"\n  now:    \"D\""
    );
    assert_eq!(
        first_difference("a", "A"),
        "line 1\n  golden: \"a\"\n  now:    \"A\""
    );
}

#[test]
fn a_long_line_that_differs_is_shown_around_its_first_difference() {
    let line = |mid: &str| format!("{}{mid}{}", "é".repeat(150), "z".repeat(300));
    let said = first_difference(&line("golden"), &line("now"));
    assert_eq!(
        said,
        format!(
            "line 1, byte 300 of it (the golden's line is 606 bytes, the line now 603 bytes)\n  \
             golden: …\"{0}golden{1}\"…\n  now:    …\"{0}now{2}\"…",
            "é".repeat(40),
            "z".repeat(74),
            "z".repeat(77),
        )
    );
    // One side ends where the other goes on.
    let (short, long) = ("x".repeat(250), "x".repeat(260));
    assert_eq!(
        first_difference(&short, &long),
        format!(
            "line 1, byte 250 of it (the golden's line is 250 bytes, the line now 260 bytes)\n  \
             golden: …\"{}\"\n  now:    …\"{}\"",
            "x".repeat(80),
            "x".repeat(90),
        )
    );
}

/// A long line one side does not have is cut like any other: a request's
/// bytes, 15 KB on one line, are not printed whole.
#[test]
fn a_long_line_one_side_lacks_is_shown_from_its_start() {
    let long = "y".repeat(15_000);
    let now = format!("a\n{long}");
    let said = first_difference("a", &now);
    assert_eq!(
        said,
        format!(
            "line 2, byte 0 of it (the golden's line is absent, the line now 15000 bytes)\n  \
             after:  \"a\"\n  golden: (no such line)\n  now:    \"{}\"…",
            "y".repeat(80),
        )
    );
    assert_eq!(
        first_difference(&now, "a"),
        format!(
            "line 2, byte 0 of it (the golden's line is 15000 bytes, the line now absent)\n  \
             after:  \"a\"\n  golden: \"{}\"…\n  now:    (no such line)",
            "y".repeat(80),
        )
    );
}

/// The command names the Bazel test that is running, whichever it is, and
/// the package under Cargo.
#[test]
fn the_rewrite_command_names_the_test_that_runs() {
    assert_eq!(
        rewrite_command(Some("//crates/clax-bot:integration"), "clax-bot"),
        "`CLAX_UPDATE_GOLDEN=1 bazel run //crates/clax-bot:integration`"
    );
    assert_eq!(
        rewrite_command(None, "clax-bot"),
        "`CLAX_UPDATE_GOLDEN=1 cargo test -p clax-bot`"
    );
}
