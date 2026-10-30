//! A fake platform opener that records what it was handed, for tests of
//! how a URL reaches the browser (`clax_mcp::tools::open_in_browser`).

/// The fake opener's script: it writes its argument count, its first
/// argument and its standard input to `got` beside itself, as
/// `<count>:<first>|<stdin>`. Its text is the same in every test, so the
/// executable is assessed once.
pub const RECORDING_OPENER: &str =
    "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s:%s|' \"$#\" \"$1\" > \"$d/got\"\ncat >> \"$d/got\"\n";

/// The URL a [`RECORDING_OPENER`] was handed (`got`, its file's text),
/// asserting it came the platform's way: on macOS on standard input as
/// `open location "<url>"` with no argument, so no process's arguments
/// show it; elsewhere as the one argument.
pub fn recorded_url(got: &str) -> String {
    let url = if cfg!(target_os = "macos") {
        got.strip_prefix("0:|open location \"")
            .and_then(|s| s.strip_suffix("\"\n"))
    } else {
        got.strip_prefix("1:").and_then(|s| s.strip_suffix('|'))
    };
    url.unwrap_or_else(|| panic!("the opener was handed {got:?}"))
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}
