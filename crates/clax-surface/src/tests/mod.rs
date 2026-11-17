//! The crate's tests: the design's two example files of a host's and a
//! fixture of the crate's own beside them, a surface named `clax`
//! (`examples`), every refusal of the `.d.ts` reader with its line
//! (`refusals`), definitions built without that reader (`model`), input
//! made to be too large or too deep (`hostile`), and value checks and
//! allow patterns (`values`), and the host protocol's messages against
//! their contract fixture (`wire`).

mod canonical;
mod examples;
mod hostile;
mod manifest;
mod model;
mod refusals;
mod rules;
mod runs;
mod schema;
mod values;
mod wire;

use crate::{Author, Definition, Error, Limits, Reserved, Ty, parse};
use serde_json::Value;

/// The crate's own fixture: a surface named `clax`, written as its
/// author's ([`Author::Clax`]), in the shape of one Clax provides
/// itself. It is no copy of ClaxBot's file
/// (`crates/clax-bot/src/clax.d.ts`, which that crate reads and tests):
/// nothing holds the two equal, and each is free to change.
const OWN_DTS: &str = include_str!("own.d.ts");
const SHELL_DTS: &str =
    include_str!("../../../../docs/superpowers/specs/2026-11-10-claxbot-surfaces/shell.d.ts");
const REVIEW_DTS: &str =
    include_str!("../../../../docs/superpowers/specs/2026-11-10-claxbot-surfaces/review.d.ts");

fn own() -> Definition {
    parse(OWN_DTS, Author::Clax, &Limits::default()).expect("own.d.ts is a definition")
}

/// Reads `src` as a host's file, beside the fixture named `clax`,
/// within `limits`.
fn host_within(src: &str, limits: &Limits) -> Result<Definition, Error> {
    let reserved = Reserved::of(&[&own()]);
    parse(
        src,
        Author::Host {
            reserved: &reserved,
        },
        limits,
    )
}

fn host(src: &str) -> Result<Definition, Error> {
    host_within(src, &Limits::default())
}

fn shell() -> Definition {
    host(SHELL_DTS).expect("shell.d.ts is a definition")
}

fn review() -> Definition {
    host(REVIEW_DTS).expect("review.d.ts is a definition")
}

/// A host's file with these members and these declarations after them:
/// the members start on line 2, and with one line of members the
/// declarations start on line 4.
fn file(members: &str, rest: &str) -> String {
    format!("declare const demo: {{\n{members}\n}};\n{rest}")
}

/// `src` is refused on `line` with a message that holds `needle`.
#[track_caller]
fn refused(src: &str, line: usize, needle: &str) {
    match host(src) {
        Ok(_) => panic!("accepted:\n{src}"),
        Err(e) => {
            assert!(
                e.message.contains(needle),
                "`{needle}` is not in: {e}\n{src}"
            );
            assert_eq!(e.line().unwrap(), line, "{e}\n{src}");
            assert_eq!(e.to_string(), format!("line {line}: {}", e.message));
        }
    }
}

/// [`crate::check_held`], with the refusal as its message.
fn held(def: &Definition, ty: &Ty, v: &Value) -> Result<(), String> {
    crate::check_held(def, ty, v).map_err(|e| e.message)
}

/// [`crate::check_sent`], with the refusal as its message.
fn sent(def: &Definition, ty: &Ty, v: &Value) -> Result<(), String> {
    crate::check_sent(def, ty, v).map_err(|e| e.message)
}

/// The stacks the crate's documentation says it runs in, in a build
/// without optimisation too: everything, under the default limits;
/// everything, with `depth` at its hard maximum; and a check of a value
/// against a type that nests a few levels, however deep the value.
const SMALL_STACK: usize = 512 * 1024;
const DEEP_STACK: usize = 1024 * 1024;
const VALUE_STACK: usize = 64 * 1024;

/// Runs `f` on a thread with a stack of so many bytes: input that nests
/// without bound overflows it, which ends the process, and so the test
/// run.
fn on_a_stack<T: Send + 'static>(bytes: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(bytes)
        .spawn(f)
        .expect("a thread")
        .join()
        .expect("the thread did not panic")
}

/// Runs `f` on a thread with a stack of 512 KiB, a quarter of what a
/// thread gets by default and half of what wasm32 gives a module: what
/// the default limits allow fits in it.
fn on_a_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    on_a_stack(SMALL_STACK, f)
}
