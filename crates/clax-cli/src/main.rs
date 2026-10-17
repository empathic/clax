//! The `clax` binary: the commands (`clax-commands`) with the files the
//! binary embeds (`clax-embedded`) and the daemon it runs in process
//! (`clax-server`). A new commit, a web UI edit or a daemon edit rebuilds
//! this crate, which is a few hundred lines; the commands are not rebuilt.

mod bundle;
mod serve;

fn main() -> std::process::ExitCode {
    clax_commands::run(clax_commands::Env {
        embedded: &bundle::Embedded,
        serve: serve::run,
        build_commit: env!("CLAX_BUILD_COMMIT"),
    })
}
