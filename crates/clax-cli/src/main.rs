//! The `clax` command line: run the daemon and manage artifacts.

mod channel;
mod client;
mod codex_approvals;
mod commands;
mod extension_files;
mod hooklog;
mod host;
mod plugin_bin;
mod plugins;
mod remote;
mod term;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "clax",
    version,
    about = "Local artifacts with comment-driven development"
)]
pub struct Cli {
    /// Emit one JSON object on stdout instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Port to use when starting a daemon (0 = any free port). Default:
    /// `CLAX_PORT`, else the home's `[serve] port` in config.toml, else 7480.
    #[arg(long, global = true)]
    pub port: Option<u16>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Start the daemon in the background (or run it here with --foreground).
    Serve(commands::serve::Args),
    /// Stop the running daemon.
    Stop,
    /// Show whether a daemon is running, and which agents are working on what.
    Status(commands::status::Args),
    /// Publish a page (and files) as a new artifact or a new version.
    Publish(Box<commands::publish::Args>),
    /// List artifacts, or the files of one.
    List(commands::list::Args),
    /// Read a published file of an artifact.
    Read(commands::read::Args),
    /// Manage an artifact's assets.
    #[command(subcommand)]
    Asset(commands::asset::Cmd),
    /// List open comment threads, show one, or act on it as you.
    ///
    /// With no argument, every open thread on every artifact, grouped by
    /// artifact, newest activity first; with an artifact (ID or URL), that
    /// artifact's; --all adds resolved threads.
    ///
    /// A thread is named `<artifact>#<n>`: the artifact's ID or URL, then the
    /// number listings show (threads are numbered per artifact in the order
    /// they were made, resolved ones included), or by its thread ID alone.
    /// `clax comments <thread>` shows it, as `show` does.
    ///
    /// reply, resolve, reopen, and send act as you, as the page's own
    /// buttons do in your browser; replies carry the name `clax comments
    /// name` sets.
    Comments(commands::comments::Args),
    /// What agents sent back: replies, versions, new artifacts, questions
    /// and finished work, newest first.
    ///
    /// With no argument, the unread items; --all adds read ones, --read
    /// shows only those; words search the items. Lines are numbered:
    /// `clax inbox show 1` prints the first of the last listing and marks it
    /// read; `read` and `unread` take numbers or item IDs, and `read --all`
    /// marks every item the listing's filters and search match.
    Inbox(commands::inbox::Args),
    /// List an artifact's versions: label, publisher, time, and the threads
    /// each addressed.
    Versions(commands::versions::Args),
    /// Read and write an artifact's page database, as the db_* tools do.
    #[command(subcommand)]
    Db(commands::db::Cmd),
    /// Open an artifact in the browser.
    Open(commands::open::Args),
    /// Delete an artifact.
    Delete(commands::delete::Args),
    /// Pin an artifact to the top of the gallery.
    Pin(commands::pin::Args),
    /// Unpin an artifact.
    Unpin(commands::pin::Args),
    /// Check the installation and storage.
    Doctor(commands::doctor::Args),
    /// Serve the MCP tools over stdin/stdout for one harness session.
    Mcp(commands::mcp::Args),
    /// Handle a harness lifecycle hook (reads the hook input from stdin).
    Hook(commands::hook::Args),
    /// Follow comments sent to an agent session (one line per comment).
    #[command(subcommand)]
    Feedback(commands::feedback::Cmd),
    /// Register the Clax plugins built into this binary with each harness
    /// whose CLI is on PATH, replacing stale registrations, and point the
    /// plugins at this binary (the `bin` setting, see `clax bin`). Also
    /// installs the Chrome extension and its native host (`clax extension
    /// install`); a failure there is reported, never fatal.
    ///
    /// Re-running reinstalls the plugin, which enables it again where it was
    /// disabled. A Pi package is removed only when `init` recorded it or its
    /// package.json names Clax's Pi package; one whose directory is missing
    /// is left and named, with the command that removes it. Grok's plugin is
    /// registered and removed as `clax-grok`, never `clax`. Known miss:
    /// `~user/` paths are not expanded. A Pi entry written that way is left
    /// registered, and a CODEX_HOME, CLAUDE_CONFIG_DIR or PI_CODING_AGENT_DIR
    /// written that way is taken relative to HOME.
    Init(commands::init::Args),
    /// Remove the Clax plugin registrations from each harness whose CLI is
    /// on PATH, the plugins' copy once no harness refers to it, and the
    /// `bin` setting when it names this binary. Also revokes the Chrome
    /// extension's credentials and removes it (`clax extension uninstall`).
    ///
    /// The copy is kept while any harness's registry still names it or
    /// cannot be read. The same known miss as `init` applies.
    Uninit(commands::init::Args),
    /// Show or set which clax binary the plugins run.
    ///
    /// The plugins run, in order: $CLAX_BIN; the `bin` setting in the
    /// home's config.toml (`clax bin set`); else the Clax release they pin,
    /// which they download into <home>/bin/<version> on first use.
    Bin {
        #[command(subcommand)]
        cmd: Option<commands::bin::Cmd>,
    },
    /// Install, remove or check the Clax Chrome extension and its native
    /// messaging host.
    ///
    /// `install` writes the extension to <home>/extension (load it once at
    /// chrome://extensions with Load unpacked) and registers the host with
    /// each installed Chrome, Chromium, Brave and Edge; `clax init` runs it.
    /// `uninstall` removes what install wrote; `clax uninit` runs it.
    #[command(subcommand)]
    Extension(commands::extension::Cmd),
    /// Export Clax's recorded history as Toolpath provenance, and show the
    /// audit journal's state.
    #[command(subcommand)]
    Toolpath(commands::toolpath::Cmd),
    /// Print a haiku about Clax, one of ten, chosen at random.
    Haiku,
    /// Print the version; with --verbose, also the commit it was built from.
    Version(commands::version::Args),
    /// Connect this machine to another machine's Clax as a person's
    /// machine, show the connection, move it to a new address, or end it.
    ///
    /// With an invite (`clax connect http://<IP>:<port>/i/<code>`, as the
    /// owner's People page shows it) the machine connects at once; with the
    /// daemon's address alone it asks the owner, who approves the request
    /// with the code this prints. Connected, this machine's CLI, hooks and
    /// MCP server talk to that daemon, signed, and start none here
    /// (`CLAX_REMOTE=0` turns that off). Writes `remote.json` and
    /// `remote.key` (0600) in the Clax home.
    Connect(commands::connect::Args),
    /// List the people connected to this Clax, with their machines and
    /// running agents, your own machines, and the People link (the
    /// owner's).
    ///
    /// Inviting people, setting their rights and revoking them take your
    /// browser, on the People page: `invite`, `rights` and `revoke` print
    /// its link and change nothing.
    People {
        #[command(subcommand)]
        cmd: Option<commands::people::PeopleCmd>,
    },
    /// List your agents that are running: the owner's here, or, connected
    /// to another machine's Clax, this machine's; or rename one.
    ///
    /// Narrowing and elevating agents take your browser, on the People
    /// page: `narrow` and `elevate` print its link and change nothing.
    Agents {
        #[command(subcommand)]
        cmd: Option<commands::people::AgentsCmd>,
    },
    /// List your approvals and approval requests.
    ///
    /// Deciding requests, approving and revoking take your browser, on the
    /// People page: `approve`, `decide` and `revoke` print its link and
    /// change nothing.
    Approvals {
        #[command(subcommand)]
        cmd: Option<commands::people::ApprovalsCmd>,
    },
    /// The Chrome native messaging host for the Clax extension (Chrome runs it).
    #[command(hide = true)]
    NativeHost(commands::native_host::Args),
}

impl Cli {
    /// `--port` when given, else `CLAX_PORT`, else the home's `[serve]
    /// port`, else 7480.
    ///
    /// # Errors
    /// When `--port` is absent and `CLAX_PORT` is set to something that is
    /// not a port (1-65535), naming the variable; or when neither is given
    /// and the home's `config.toml` exists but cannot be read or parsed, or
    /// holds a `[serve] port` that is not a port, naming the file. An empty
    /// `CLAX_PORT`, a missing file or a missing key means the next source.
    pub fn port_for(&self, home: &clax_core::Home) -> anyhow::Result<u16> {
        self.port_for_with(home, std::env::var_os(PORT_ENV))
    }

    /// As [`Cli::port_for`], with `env` as the value of `CLAX_PORT`.
    pub fn port_for_with(
        &self,
        home: &clax_core::Home,
        env: Option<std::ffi::OsString>,
    ) -> anyhow::Result<u16> {
        if let Some(p) = self.port {
            return Ok(p);
        }
        if let Some(p) = env_port(env) {
            return p;
        }
        let port = clax_core::config::HomeConfig::load(home.root())
            .and_then(|c| c.serve_port())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(port.unwrap_or(clax_server::daemon::DEFAULT_PORT))
    }
}

/// The environment variable that sets the daemon's port, ahead of the
/// home's `config.toml` and behind `--port`.
pub const PORT_ENV: &str = "CLAX_PORT";

/// The port `CLAX_PORT`'s value `env` sets: `None` when it is unset or empty;
/// an error naming the variable when it is not a port (1-65535).
pub fn env_port(env: Option<std::ffi::OsString>) -> Option<anyhow::Result<u16>> {
    let v = env.filter(|v| !v.is_empty())?;
    Some(
        v.to_str()
            .and_then(|t| t.trim().parse::<u16>().ok())
            .filter(|p| *p != 0)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{PORT_ENV} is {:?}, which is not a port (1-65535)",
                    v.to_string_lossy()
                )
            }),
    )
}

/// Whether `cmd` runs while this home is connected to another machine's
/// Clax (spec §6.7): the commands that list, read and act through the
/// remote daemon as the person (`list`, `read`, `open`, `versions`, `db`,
/// `comments`, `agents`, `approvals`, `status`, `connect`; `agents` and
/// `approvals` changes print the remote People link), the agent
/// sides that run over it (`mcp`, `hook`, `feedback`), and this machine's
/// own (its local daemon with `serve` and `stop`, installing, `doctor`,
/// `bin`, `version`, `haiku`). Every other command says it is not
/// available on a remote Clax and exits 2.
fn remote_ok(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::List(_)
        | Cmd::Read(_)
        | Cmd::Open(_)
        | Cmd::Versions(_)
        | Cmd::Db(_)
        | Cmd::Comments(_)
        | Cmd::Agents { .. }
        | Cmd::Approvals { .. }
        | Cmd::Status(_)
        | Cmd::Connect(_) => true,
        Cmd::Mcp(_) | Cmd::Hook(_) | Cmd::Feedback(_) => true,
        Cmd::Serve(_)
        | Cmd::Stop
        | Cmd::Doctor(_)
        | Cmd::Init(_)
        | Cmd::Uninit(_)
        | Cmd::Bin { .. }
        | Cmd::Haiku
        | Cmd::Version(_) => true,
        Cmd::Publish(_)
        | Cmd::Asset(_)
        | Cmd::Inbox(_)
        | Cmd::Delete(_)
        | Cmd::Pin(_)
        | Cmd::Unpin(_)
        | Cmd::Extension(_)
        | Cmd::Toolpath(_)
        | Cmd::People { .. }
        | Cmd::NativeHost(_) => false,
    }
}

/// The subcommand the command line names: its first argument that is
/// neither a flag nor `--port`'s value.
fn invoked_subcommand() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => {
                args.next();
            }
            s if s.starts_with('-') => {}
            _ => return Some(a),
        }
    }
    None
}

/// The value after `--agent` on the command line, or `-`.
fn agent_arg() -> String {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--agent")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "-".into())
}

/// The Clax home, as an absolute path, so commands that run elsewhere (the
/// daemon, the harness CLIs `init` runs in HOME) see the same directory.
fn home_from_env() -> Result<clax_core::Home, String> {
    let home = clax_core::Home::from_env().map_err(|e| e.to_string())?;
    let root = std::path::absolute(home.root()).map_err(|e| format!("CLAX_HOME: {e}"))?;
    Ok(clax_core::Home::at(root))
}

fn main() {
    clax_core::set_build_commit(env!("CLAX_BUILD_COMMIT"));
    let started = std::time::Instant::now();
    let invoked = invoked_subcommand();
    // A hook must never fail its harness, so its startup failures exit 0.
    let hook = invoked.as_deref() == Some("hook");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if hook && let Ok(home) = home_from_env() {
                let text = e.to_string();
                commands::hook::log_run(&home, &agent_arg(), "-", started, Some(text.trim()), None);
            }
            let shown = matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion);
            if invoked.as_deref() == Some("native-host") && !shown {
                // Chrome reads only framed replies; the text goes to stderr.
                let _ = e.print();
                commands::native_host::run_unparsed(&e.to_string());
            }
            let code = match e.kind() {
                _ if hook => 0,
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 1,
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let home = match home_from_env() {
        Ok(home) => home,
        Err(e) => {
            if matches!(cli.cmd, Cmd::NativeHost(_)) {
                commands::native_host::run_without_home(&e);
            }
            eprintln!("error: {e}");
            std::process::exit(if hook { 0 } else { 1 });
        }
    };
    if !remote_ok(&cli.cmd) && clax_core::remote::Mode::of(&home).is_remote() {
        let name = invoked.as_deref().unwrap_or("this command");
        eprintln!("{}", client::NotRemote::line_for(&home, name));
        std::process::exit(2);
    }
    let result = match &cli.cmd {
        Cmd::Serve(a) => commands::serve::run(&cli, &home, a),
        Cmd::Stop => commands::stop::run(&cli, &home),
        Cmd::Status(a) => commands::status::run(&cli, &home, a),
        Cmd::Publish(a) => commands::publish::run(&cli, &home, a),
        Cmd::List(a) => commands::list::run(&cli, &home, a),
        Cmd::Read(a) => commands::read::run(&cli, &home, a),
        Cmd::Asset(c) => commands::asset::run(&cli, &home, c),
        Cmd::Comments(a) => commands::comments::run(&cli, &home, a),
        Cmd::Inbox(a) => commands::inbox::run(&cli, &home, a),
        Cmd::Versions(a) => commands::versions::run(&cli, &home, a),
        Cmd::Db(c) => commands::db::run(&cli, &home, c),
        Cmd::Open(a) => commands::open::run(&cli, &home, a),
        Cmd::Delete(a) => commands::delete::run(&cli, &home, a),
        Cmd::Pin(a) => commands::pin::run(&cli, &home, a, true),
        Cmd::Unpin(a) => commands::pin::run(&cli, &home, a, false),
        Cmd::Doctor(a) => commands::doctor::run(&cli, &home, a),
        Cmd::Mcp(a) => commands::mcp::run(&cli, &home, a),
        Cmd::Hook(a) => commands::hook::run(&cli, &home, a),
        Cmd::Feedback(c) => commands::feedback::run(&cli, &home, c),
        Cmd::Init(a) => commands::init::init(&cli, &home, a),
        Cmd::Uninit(a) => commands::init::uninit(&cli, &home, a),
        Cmd::Bin { cmd } => commands::bin::run(&cli, &home, cmd.as_ref()),
        Cmd::Extension(c) => commands::extension::run(&cli, &home, c),
        Cmd::Toolpath(c) => commands::toolpath::run(&cli, &home, c),
        Cmd::Haiku => commands::haiku::run(&cli),
        Cmd::Version(a) => commands::version::run(&cli, a),
        Cmd::NativeHost(a) => commands::native_host::run(&cli, &home, a),
        Cmd::Connect(a) => commands::connect::run(&cli, &home, a),
        Cmd::People { cmd } => commands::people::people(&cli, &home, cmd.as_ref()),
        Cmd::Agents { cmd } => commands::people::agents(&cli, &home, cmd.as_ref()),
        Cmd::Approvals { cmd } => commands::people::approvals(&cli, &home, cmd.as_ref()),
    };
    if let Err(e) = result {
        if let Some(n) = e.downcast_ref::<client::NotForAgents>() {
            // Under a harness, a route that serves people: one readable line.
            println!("{}", n.line());
            std::process::exit(2);
        }
        if let Some(n) = e.downcast_ref::<client::ForPeople>() {
            println!("{n}");
            std::process::exit(2);
        }
        if let Some(n) = e.downcast_ref::<client::NotRemote>() {
            eprintln!("{n}");
            std::process::exit(2);
        }
        eprintln!("error: {e:#}");
        std::process::exit(exit_code(&e));
    }
}

/// The status `clax` exits with when its database's schema could not be
/// brought up to date ([`clax_core::CoreError::MigrationFailed`]): a
/// failure that repeats on every start until the database or the binary
/// changes, so a supervisor stops restarting on it (the Clax app's
/// `DaemonSupervisor.migrationFailedExit`).
const MIGRATION_FAILED_EXIT: i32 = 65;

/// The exit status for a command that failed with `e`:
/// [`MIGRATION_FAILED_EXIT`] for a failed migration, otherwise 1.
fn exit_code(e: &anyhow::Error) -> i32 {
    let migration = e.chain().any(|c| {
        matches!(
            c.downcast_ref::<clax_core::CoreError>(),
            Some(clax_core::CoreError::MigrationFailed { .. })
        )
    });
    if migration { MIGRATION_FAILED_EXIT } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_migration_exits_with_its_own_status() {
        let e = anyhow::Error::from(clax_core::CoreError::MigrationFailed {
            at: 27,
            detail: "duplicate column name: draft_of".into(),
        })
        .context("starting the daemon");
        assert_eq!(exit_code(&e), MIGRATION_FAILED_EXIT);
        assert_eq!(exit_code(&anyhow::anyhow!("other")), 1);
        let newer = clax_core::CoreError::SchemaNewer {
            found: 40,
            known: 31,
        };
        assert_eq!(exit_code(&anyhow::Error::from(newer)), 1);
    }

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn inbox_takes_search_words_filters_and_subcommands() {
        let Cmd::Inbox(a) = cli(&["clax", "inbox", "footer", "green"]).cmd else {
            panic!("inbox");
        };
        assert_eq!(a.search, ["footer", "green"]);
        assert!(a.cmd.is_none());
        let Cmd::Inbox(a) = cli(&["clax", "inbox", "--kind", "reply", "-n", "5", "header"]).cmd
        else {
            panic!("inbox");
        };
        assert_eq!(
            (a.filters.kinds.as_slice(), a.search.as_slice(), a.limit),
            (&["reply".to_string()][..], &["header".to_string()][..], 5)
        );
        let Cmd::Inbox(a) = cli(&[
            "clax", "inbox", "read", "--all", "--kind", "reply", "--search", "header",
        ])
        .cmd
        else {
            panic!("inbox");
        };
        let Some(commands::inbox::Cmd::Read {
            all: true,
            filters,
            search: Some(search),
            ..
        }) = a.cmd
        else {
            panic!("read --all");
        };
        assert_eq!(
            (filters.kinds.as_slice(), search.as_str()),
            (&["reply".to_string()][..], "header")
        );
        let Cmd::Inbox(a) = cli(&["clax", "inbox", "show", "1"]).cmd else {
            panic!("inbox");
        };
        assert!(matches!(a.cmd, Some(commands::inbox::Cmd::Show { .. })));
        assert!(Cli::try_parse_from(["clax", "inbox", "--kind", "nope"]).is_err());
    }

    #[test]
    fn port_for_uses_the_flag_then_the_config_then_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        assert_eq!(
            cli(&["clax", "list"]).port_for_with(&home, None).unwrap(),
            clax_server::daemon::DEFAULT_PORT
        );
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 7481\n").unwrap();
        assert_eq!(
            cli(&["clax", "list"]).port_for_with(&home, None).unwrap(),
            7481
        );
        assert_eq!(
            cli(&["clax", "--port", "0", "list"])
                .port_for_with(&home, None)
                .unwrap(),
            0
        );
    }

    #[test]
    fn clax_port_comes_between_the_flag_and_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 7481\n").unwrap();
        let env = |v: &str| Some(std::ffi::OsString::from(v));
        let list = cli(&["clax", "list"]);
        assert_eq!(list.port_for_with(&home, env("7490")).unwrap(), 7490);
        assert_eq!(list.port_for_with(&home, env("")).unwrap(), 7481);
        assert_eq!(list.port_for_with(&home, None).unwrap(), 7481);
        assert_eq!(
            cli(&["clax", "--port", "7491", "list"])
                .port_for_with(&home, env("7490"))
                .unwrap(),
            7491
        );
        // CLAX_PORT is read ahead of the config, so a broken config does not
        // matter while it is set.
        std::fs::write(dir.path().join("config.toml"), "[serve\n").unwrap();
        assert_eq!(list.port_for_with(&home, env("7490")).unwrap(), 7490);
        for bad in ["0", "74810", "x", "-1"] {
            let e = list.port_for_with(&home, env(bad)).unwrap_err().to_string();
            assert!(
                e.contains("CLAX_PORT") && e.contains("not a port"),
                "{bad}: {e}"
            );
        }
    }

    #[test]
    fn port_for_rejects_a_config_that_does_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        std::fs::write(dir.path().join("config.toml"), "[serve\nport = 7481\n").unwrap();
        let e = cli(&["clax", "list"])
            .port_for_with(&home, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn port_for_rejects_a_config_that_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        // A directory where the file should be: present, but unreadable as text.
        std::fs::create_dir(dir.path().join("config.toml")).unwrap();
        let e = cli(&["clax", "list"])
            .port_for_with(&home, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn port_for_rejects_a_serve_port_that_is_not_a_port() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 74810\n").unwrap();
        let e = cli(&["clax", "list"])
            .port_for_with(&home, None)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("config.toml") && e.contains("[serve] port"),
            "{e}"
        );
    }
}
