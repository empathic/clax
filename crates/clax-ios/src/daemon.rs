//! One daemon per process: started on a thread of its own, which owns a
//! two-worker Tokio runtime and blocks on [`clax_server::serve::serve`],
//! and driven from any thread through [`start_at`], [`stop`], [`state`] and
//! [`trim`].

use clax_core::Home;
use clax_sample::{KeySlot, Sampler};
use clax_server::serve::{EmbeddedControl, ServeConfig, StopOn, serve};
use clax_store::TrimReport;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

/// The daemon's directories under an iOS app's container (`$HOME`).
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    /// `<home_dir>/Library/Application Support/clax-home` and
    /// `<home_dir>/Library/Caches/clax`.
    pub fn under(home_dir: &Path) -> Paths {
        let library = home_dir.join("Library");
        Paths {
            home: library.join("Application Support").join("clax-home"),
            cache: library.join("Caches").join("clax"),
        }
    }

    /// `<cache>/daemon.log`.
    pub fn log(&self) -> PathBuf {
        self.cache.join("daemon.log")
    }
}

/// No daemon has run in this process.
pub const IDLE: i32 = 0;
/// A daemon is starting: its store opens and migrates.
pub const STARTING: i32 = 1;
/// A daemon serves.
pub const UP: i32 = 2;
/// A daemon was asked to stop and has not yet.
pub const STOPPING: i32 = 3;
/// The last daemon stopped cleanly; a run that failed leaves this plus
/// the status `clax serve` would have exited with ([`final_state`]).
pub const EXITED: i32 = 100;

/// The state a run whose thread panicked leaves: 100 + 101, the status a
/// panicking `clax serve` exits with.
const PANICKED: i32 = EXITED + 101;

/// How long the runtime's tasks get to end once `serve` has returned.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// What drives the daemon that is starting, up or stopping.
struct Running {
    stop: watch::Sender<bool>,
    trim: mpsc::Sender<std::sync::mpsc::Sender<TrimReport>>,
}

/// The daemon that is starting, up or stopping; `None` once its thread has
/// ended. [`STATE`] changes from a run's end to a new start only under
/// this lock, so a start never sees a run that is about to end.
static DAEMON: Mutex<Option<Running>> = Mutex::new(None);

/// What [`state`] answers.
static STATE: AtomicI32 = AtomicI32::new(IDLE);

/// The owner's key, as `PUT /api/sample/key` sets it. It lives as long as
/// the process, so a restarted daemon samples with the key put before.
static SLOT: LazyLock<KeySlot> = LazyLock::new(KeySlot::new);

/// Held by a test to keep a run from ending while it looks at it.
#[cfg(test)]
static FINISH: Mutex<()> = Mutex::new(());

fn daemon() -> MutexGuard<'static, Option<Running>> {
    DAEMON.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state an ended run leaves: 100 + [`clax_core::exit::status_of`],
/// 100 for `Ok`.
pub fn final_state(r: &anyhow::Result<()>) -> i32 {
    match r {
        Ok(()) => EXITED,
        Err(e) => EXITED + clax_core::exit::status_of(e.as_ref()),
    }
}

/// Starts a daemon at `paths` unless one is starting, up or stopping
/// (then 1). Test entry point; [`crate::clax_start`] calls it.
pub fn start_at(paths: Paths, port: u16) -> i32 {
    let mut running = daemon();
    if running.is_some() {
        return 1;
    }
    // `serve` creates the home; a cache that cannot be made leaves the
    // daemon without a log file, nothing more.
    let _ = std::fs::create_dir_all(&paths.home);
    let _ = std::fs::create_dir_all(&paths.cache);
    crate::log::init(paths.log());
    let (stop_tx, stop_rx) = watch::channel(false);
    let (trim_tx, trim_rx) = mpsc::channel(4);
    let control = EmbeddedControl {
        stop: stop_rx,
        trim: trim_rx,
    };
    let cfg = config(&paths, port, SLOT.clone(), control);
    STATE.store(STARTING, Ordering::SeqCst);
    let spawned = std::thread::Builder::new()
        .name("clax-daemon".into())
        .spawn(move || run_thread(cfg));
    if let Err(e) = spawned {
        tracing::error!(error = %e, "could not start the daemon's thread");
        STATE.store(EXITED + 1, Ordering::SeqCst);
        return 0;
    }
    *running = Some(Running {
        stop: stop_tx,
        trim: trim_tx,
    });
    0
}

/// The `clax-daemon` thread: a runtime of two workers blocking on `serve`,
/// then the state the run leaves. A panic on this thread ends the run, never
/// the caller's.
fn run_thread(cfg: ServeConfig) {
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("clax-rt")
            .enable_all()
            .build()?;
        let r = rt.block_on(run(cfg));
        rt.shutdown_timeout(RUNTIME_SHUTDOWN);
        r
    }));
    let code = match &ended {
        Ok(r) => {
            if let Err(e) = r {
                tracing::error!(error = %format!("{e:#}"), "the daemon stopped");
            }
            final_state(r)
        }
        Err(_) => {
            tracing::error!("the daemon's thread panicked");
            PANICKED
        }
    };
    #[cfg(test)]
    let _held = FINISH.lock().unwrap_or_else(|e| e.into_inner());
    let mut running = daemon();
    *running = None;
    STATE.store(code, Ordering::SeqCst);
}

/// `serve`, with the state set to [`UP`] once it serves, unless a stop was
/// asked first.
async fn run(cfg: ServeConfig) -> anyhow::Result<()> {
    let (ready_tx, ready_rx) = oneshot::channel();
    tokio::spawn(async move {
        if ready_rx.await.is_ok() {
            let _ = STATE.compare_exchange(STARTING, UP, Ordering::SeqCst, Ordering::SeqCst);
        }
    });
    serve(cfg, Some(ready_tx)).await
}

/// Asks the running daemon to stop and returns at once; nothing when none
/// is running.
pub fn stop() {
    if let Some(r) = daemon().as_ref() {
        STATE.store(STOPPING, Ordering::SeqCst);
        let _ = r.stop.send(true);
    }
}

/// [`IDLE`], [`STARTING`], [`UP`], [`STOPPING`], or what the last run
/// left ([`final_state`], or 201 for a panic on its thread).
pub fn state() -> i32 {
    STATE.load(Ordering::SeqCst)
}

/// Asks the running daemon for one trim and waits up to `wait`; `None`
/// when none is up or it did not answer in time.
pub fn trim(wait: Duration) -> Option<TrimReport> {
    let trim = {
        let running = daemon();
        if state() != UP {
            return None;
        }
        running.as_ref()?.trim.clone()
    };
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    trim.try_send(reply_tx).ok()?;
    reply_rx.recv_timeout(wait).ok()
}

/// `clax serve`'s ServeConfig for `paths` (`crates/clax-cli/src/serve.rs`):
/// 127.0.0.1, the 30 s stale check and 60 s reaper, Codex push disabled
/// (`Default::default()`), the sampler [`Sampler::from_home_with_slot`],
/// the embedded web UI ([`crate::web::web_ui`]), no hosted mode,
/// `require_credential: true`, and `stop: StopOn::Embedded(control)`.
pub fn config(paths: &Paths, port: u16, slot: KeySlot, control: EmbeddedControl) -> ServeConfig {
    ServeConfig {
        home: Home::at(paths.home.clone()),
        bind: Ipv4Addr::LOCALHOST.into(),
        port,
        version: env!("CARGO_PKG_VERSION"),
        stale_check_interval: Duration::from_secs(30),
        reap_interval: Duration::from_secs(60),
        codex: Default::default(),
        sample: Arc::new(Sampler::from_home_with_slot(&paths.home, slot)),
        web: crate::web::web_ui(),
        hosted: None,
        require_credential: true,
        stop: StopOn::Embedded(control),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::daemon::{DaemonInfo, read_daemon_info};
    use std::time::Instant;

    /// One daemon per process: the lifecycle tests take turns.
    static LOCK: Mutex<()> = Mutex::new(());

    /// Takes the turn, and stops whatever an earlier test left running.
    fn turn() -> MutexGuard<'static, ()> {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        stop();
        until("no daemon runs", || state() == IDLE || state() >= EXITED);
        g
    }

    /// Polls `cond` every 20 ms, failing after 30 s.
    fn until(what: &str, cond: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !cond() {
            assert!(Instant::now() < deadline, "{what}: state {}", state());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn up(home: &Home) -> DaemonInfo {
        until("the daemon is up", || state() == UP);
        read_daemon_info(home).expect("daemon.json")
    }

    fn stopped() {
        stop();
        until("the daemon stopped", || state() >= EXITED);
        assert_eq!(state(), EXITED);
    }

    /// `method path` on the daemon at `port`: its status and body.
    fn http(
        method: reqwest::Method,
        port: u16,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut r =
                reqwest::Client::new().request(method, format!("http://127.0.0.1:{port}{path}"));
            if let Some(t) = token {
                r = r.bearer_auth(t);
            }
            if let Some(b) = body {
                r = r
                    .header("content-type", "application/json")
                    .body(b.to_owned());
            }
            let resp = r.send().await.unwrap();
            (resp.status().as_u16(), resp.text().await.unwrap())
        })
    }

    fn sample_available(info: &DaemonInfo) -> bool {
        let (status, body) = http(
            reqwest::Method::GET,
            info.port,
            "/api/sample",
            Some(&info.token),
            None,
        );
        assert_eq!(status, 200, "{body}");
        body.contains(r#""available":true"#)
    }

    #[test]
    fn start_stop_start_in_one_process() {
        let _t = turn();
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(Paths::under(tmp.path()).home);
        assert_eq!(start_at(Paths::under(tmp.path()), 0), 0);
        let first = up(&home);
        assert_eq!(first.pid, std::process::id());
        let (status, _) = http(reqwest::Method::GET, first.port, "/healthz", None, None);
        assert_eq!(status, 200);
        assert_eq!(start_at(Paths::under(tmp.path()), 0), 1);
        stopped();
        assert!(read_daemon_info(&home).is_none(), "daemon.json is removed");

        assert_eq!(start_at(Paths::under(tmp.path()), 0), 0);
        let second = up(&home);
        assert_ne!(second.token, first.token);
        let (status, _) = http(reqwest::Method::GET, second.port, "/healthz", None, None);
        assert_eq!(status, 200);
        stopped();
    }

    #[test]
    fn lifecycle_races() {
        let _t = turn();
        let tmp = tempfile::tempdir().unwrap();
        {
            let _held = FINISH.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(start_at(Paths::under(tmp.path()), 0), 0);
            stop();
            assert_eq!(state(), STOPPING);
            assert_eq!(start_at(Paths::under(tmp.path()), 0), 1);
            assert_eq!(state(), STOPPING);
        }
        until("a stop while starting ends the run", || state() >= EXITED);
        assert_eq!(state(), EXITED);
        stop();
        assert_eq!(state(), EXITED);
        assert!(trim(Duration::from_secs(5)).is_none());
    }

    #[test]
    fn the_key_slot_survives_a_restart() {
        let _t = turn();
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(Paths::under(tmp.path()).home);
        assert_eq!(start_at(Paths::under(tmp.path()), 0), 0);
        let info = up(&home);
        let (status, body) = http(
            reqwest::Method::PUT,
            info.port,
            "/api/sample/key",
            Some(&info.token),
            Some(r#"{"key":"sk-x"}"#),
        );
        assert_eq!(status, 204, "{body}");
        assert!(sample_available(&info));
        stopped();
        assert_eq!(start_at(Paths::under(tmp.path()), 0), 0);
        let info = up(&home);
        assert!(sample_available(&info));
        stopped();
    }

    #[test]
    fn trim_answers_with_a_checkpoint() {
        let _t = turn();
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(Paths::under(tmp.path()).home);
        assert_eq!(start_at(Paths::under(tmp.path()), 0), 0);
        up(&home);
        let r = trim(Duration::from_secs(5)).expect("a trim report");
        assert_eq!(r.checkpointed, r.wal_frames);
        stopped();
    }

    #[test]
    fn final_state_maps_statuses() {
        assert_eq!(final_state(&Ok(())), 100);
        let migration = anyhow::Error::new(clax_core::CoreError::MigrationFailed {
            at: 27,
            detail: "duplicate column name: draft_of".into(),
        })
        .context("opening the store");
        assert_eq!(final_state(&Err(migration)), 165);
        assert_eq!(final_state(&Err(anyhow::anyhow!("x"))), 101);
    }

    #[test]
    fn paths_sit_in_the_app_container() {
        let p = Paths::under(Path::new("/c"));
        assert_eq!(
            p.home,
            Path::new("/c/Library/Application Support/clax-home")
        );
        assert_eq!(p.cache, Path::new("/c/Library/Caches/clax"));
        assert_eq!(p.log(), Path::new("/c/Library/Caches/clax/daemon.log"));
    }
}
