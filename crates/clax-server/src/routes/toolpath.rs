//! The audit history's routes (spec 2026-10-06-toolpath-audit-design
//! §8.3): `GET /api/toolpath/export`, which streams the selected history as
//! a Toolpath document, and `GET /api/toolpath/status`, the journal's
//! state. Both are the owner's alone.

use crate::identity::Identity;
use crate::state::AppState;
use clax_core::toolpath::RenderEnv;
use clax_core::toolpath::project;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Whether `who` may read the audit history: the owner, by the daemon
/// token or the owner cookie (spec §8.3). Checked from the credentials
/// alone, before anything is read or made.
fn owner_reads_history(who: &Identity) -> Result<(), crate::error::ApiError> {
    if who.token || who.owner_cookie {
        Ok(())
    } else {
        Err(crate::error::ApiError::forbidden(
            "forbidden",
            "only the owner (the token or the owner cookie) reads the audit history",
        ))
    }
}

/// The export a `GET /api/toolpath/export` query asks for: the CLI's options
/// as parameters (`artifact`, `live` and `by_session` repeat; `since`,
/// `until`, `shape`, `format` and the flags `no_text`, `no_names`,
/// `no_paths` and `pretty`, `true` or `false`, appear at most once). An
/// unknown or repeated parameter is refused, so a misspelt option never
/// widens an export.
pub fn export_query(query: &str) -> Result<project::Export, crate::error::ApiError> {
    use crate::error::ApiError;
    let mut req = project::Export::default();
    let mut seen: Vec<String> = Vec::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        let (k, v) = (k.into_owned(), v.into_owned());
        let once = !matches!(k.as_str(), "artifact" | "live" | "by_session");
        if once && seen.contains(&k) {
            return Err(ApiError::bad_request(
                "invalid_parameter",
                format!("{k} may be given once"),
            ));
        }
        seen.push(k.clone());
        let flag = |v: &str| match v {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(ApiError::bad_request(
                "invalid_parameter",
                format!("{k} is true or false, not '{v}'"),
            )),
        };
        match k.as_str() {
            "artifact" => req.selection.artifacts.push(v),
            "live" => req.selection.live_pages.push(v),
            "by_session" => req.selection.by_sessions.push(v),
            "since" => req.selection.since = Some(v),
            "until" => req.selection.until = Some(v),
            "shape" => {
                req.shape = project::Shape::parse(&v).ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_parameter",
                        format!("shape is artifacts or journal, not '{v}'"),
                    )
                })?
            }
            "format" => {
                req.format = project::Format::parse(&v).ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_parameter",
                        format!("format is json or jsonl, not '{v}'"),
                    )
                })?
            }
            "no_text" => req.redaction.no_text = flag(&v)?,
            "no_names" => req.redaction.no_names = flag(&v)?,
            "no_paths" => req.redaction.no_paths = flag(&v)?,
            "pretty" => req.pretty = flag(&v)?,
            _ => {
                return Err(ApiError::bad_request(
                    "unknown_parameter",
                    format!(
                        "unknown parameter {k}; the parameters are artifact, live, by_session, since, until, shape, format, no_text, no_names, no_paths and pretty"
                    ),
                ));
            }
        }
    }
    Ok(req)
}

/// The size of the chunks an export streams.
const EXPORT_CHUNK: usize = 64 * 1024;

/// How many chunks wait between an export job and its response body.
const EXPORT_QUEUE: usize = 4;

/// What bounds the daemon's exports: how many run at once (each holds a
/// store worker, a reader connection and its snapshot), how long one waits
/// for its reader to take the next chunk, and how long one may run in all.
pub struct Exports {
    permits: Arc<tokio::sync::Semaphore>,
    /// How long a chunk waits for room before the export gives up on its
    /// reader.
    pub stall: Duration,
    /// How long an export may run before it is ended.
    pub ceiling: Duration,
}

impl Exports {
    /// One export at a time; 60 s for a stalled reader; 15 minutes in all.
    pub fn new() -> Arc<Exports> {
        Arc::new(Exports {
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            stall: Duration::from_secs(60),
            ceiling: Duration::from_secs(15 * 60),
        })
    }

    /// The permits, for a test that holds one.
    pub fn permits(&self) -> Arc<tokio::sync::Semaphore> {
        self.permits.clone()
    }
}

type Chunk = Result<axum::body::Bytes, std::io::Error>;

/// The writer an export job writes through: chunks of [`EXPORT_CHUNK`]
/// bytes, handed to the response body over a bounded channel, each waiting
/// at most the stall limit for room, until the export's deadline.
pub struct ChunkWriter {
    tx: tokio::sync::mpsc::Sender<Chunk>,
    rt: tokio::runtime::Handle,
    buf: Vec<u8>,
    stall: Duration,
    deadline: Instant,
}

impl ChunkWriter {
    fn ship(&mut self) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind};
        if self.buf.is_empty() {
            return Ok(());
        }
        if Instant::now() > self.deadline {
            return Err(Error::new(
                ErrorKind::TimedOut,
                "the export ran past its time limit",
            ));
        }
        let chunk = axum::body::Bytes::from(std::mem::replace(
            &mut self.buf,
            Vec::with_capacity(EXPORT_CHUNK),
        ));
        let (tx, stall) = (self.tx.clone(), self.stall);
        match self
            .rt
            .block_on(async move { tokio::time::timeout(stall, tx.send(Ok(chunk))).await })
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(Error::new(
                ErrorKind::BrokenPipe,
                "the export's reader went away",
            )),
            Err(_) => Err(Error::new(
                ErrorKind::TimedOut,
                "the export's reader stopped reading",
            )),
        }
    }
}

impl std::io::Write for ChunkWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= EXPORT_CHUNK {
            self.ship()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.ship()
    }
}

/// The response of an export job that `start` runs, writing through the
/// [`ChunkWriter`] it is given (on a thread that may block).
///
/// Nothing the job writes is sent until a whole chunk or the job's end, so
/// a job that fails before its first chunk is an error status. After that,
/// the job's outcome decides how the body ends: once every chunk is sent,
/// the body awaits the job, and anything but success (an error, a reader
/// that stalled, the time limit, a panic) ends it with an error, which
/// aborts the transfer. A truncated export never ends like a whole one.
pub async fn stream_export<F>(
    content_type: &'static str,
    limits: &Exports,
    start: F,
) -> Result<axum::response::Response, crate::error::ApiError>
where
    F: FnOnce(ChunkWriter) -> tokio::task::JoinHandle<clax_core::Result<()>>,
{
    use axum::response::IntoResponse;
    use futures::StreamExt;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Chunk>(EXPORT_QUEUE);
    let w = ChunkWriter {
        tx,
        rt: tokio::runtime::Handle::current(),
        buf: Vec::with_capacity(EXPORT_CHUNK),
        stall: limits.stall,
        deadline: Instant::now() + limits.ceiling,
    };
    let job = start(w);
    let Some(first) = rx.recv().await else {
        return match job.await {
            Ok(Ok(())) => Err(crate::error::ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "the export wrote nothing",
            )),
            Ok(Err(e)) => Err(e.into()),
            Err(e) => {
                tracing::error!(error = %e, "the export task failed");
                Err(clax_core::CoreError::TaskFailed.into())
            }
        };
    };
    let end = futures::stream::once(async move {
        let failed = match job.await {
            Ok(Ok(())) => return None,
            Ok(Err(e)) => e.to_string(),
            Err(e) => e.to_string(),
        };
        tracing::warn!(error = %failed, "an export stopped partway");
        Some(Err(std::io::Error::other("the export stopped partway")))
    })
    .filter_map(std::future::ready);
    let body = futures::stream::once(async move { first })
        .chain(tokio_stream::wrappers::ReceiverStream::new(rx))
        .chain(end);
    let mut res = axum::body::Body::from_stream(body).into_response();
    let h = res.headers_mut();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(content_type),
    );
    h.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(res)
}

/// `GET /api/toolpath/export` (spec §8.3): the selected history as a
/// Toolpath document, streamed. Owner only: anyone else gets 403 before
/// anything is read. One export runs at a time; another gets 503
/// `export_busy` before any work. The export reads one snapshot, in one
/// read transaction on a store worker, and writes as it reads, so memory
/// holds a few chunks and one path's actors, never the document. A refusal
/// is an error status, since it comes before the first byte; a failure
/// after it aborts the transfer ([`stream_export`]).
pub async fn export(
    axum::extract::State(s): axum::extract::State<AppState>,
    who: Identity,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Result<axum::response::Response, crate::error::ApiError> {
    owner_reads_history(&who)?;
    let req = export_query(query.as_deref().unwrap_or(""))?;
    let content_type = match req.format {
        project::Format::Json => "application/json",
        project::Format::Jsonl => "application/x-ndjson",
    };
    let permit = s.exports.permits.clone().try_acquire_owned().map_err(|_| {
        crate::error::ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "export_busy",
            "another export is running; try again when it ends",
        )
    })?;
    let (view_base, version) = (s.browser_base.clone(), s.version);
    let store = s.store.clone();
    stream_export(content_type, &s.exports, move |mut w| {
        tokio::spawn(async move {
            store
                .call(move |st| {
                    let _permit = permit;
                    let env = project::ExportEnv {
                        render: RenderEnv::export(st.install_id()?, view_base),
                        clax_version: version.to_string(),
                        clax_commit: clax_core::build_commit().to_string(),
                    };
                    st.export(&req, &env, &mut w)
                })
                .await
        })
    })
    .await
}

/// `GET /api/toolpath/status` (spec §8.3): the journal's state. Owner
/// only. Until the journal appender runs, `journal` is false and the
/// segment, cursor, lag and last error are null; `newest_seq` is the
/// newest recorded event's.
pub async fn status(
    axum::extract::State(s): axum::extract::State<AppState>,
    who: Identity,
) -> Result<axum::Json<serde_json::Value>, crate::error::ApiError> {
    owner_reads_history(&who)?;
    let newest = s.store_call(|st| st.newest_seq()).await?;
    Ok(axum::Json(serde_json::json!({
        "journal": false,
        "dir": s.home.root().join("toolpath").join("journal"),
        "segment": null,
        "cursor": null,
        "newest_seq": newest,
        "lag_ms": null,
        "last_error": null,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn limits(stall: Duration) -> Exports {
        Exports {
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            stall,
            ceiling: Duration::from_secs(600),
        }
    }

    /// The body of `res`, or the error that ended it.
    async fn body(res: axum::response::Response) -> Result<Vec<u8>, String> {
        axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .map(|b| b.to_vec())
            .map_err(|e| e.to_string())
    }

    /// A job on a blocking thread that writes `bytes` bytes, then does
    /// `then`.
    fn job(
        bytes: usize,
        then: fn() -> clax_core::Result<()>,
    ) -> impl FnOnce(ChunkWriter) -> tokio::task::JoinHandle<clax_core::Result<()>> {
        move |mut w| {
            tokio::task::spawn_blocking(move || {
                w.write_all(&vec![b'x'; bytes])?;
                then()?;
                w.flush()?;
                Ok(())
            })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_whole_export_ends_cleanly() {
        let res = stream_export(
            "application/json",
            &limits(Duration::from_secs(60)),
            job(200_000, || Ok(())),
        )
        .await
        .unwrap();
        assert_eq!(body(res).await.unwrap().len(), 200_000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failure_before_the_first_chunk_is_an_error_status() {
        let e = stream_export(
            "application/json",
            &limits(Duration::from_secs(60)),
            job(100, || {
                Err(clax_core::CoreError::invalid(
                    "unknown_session",
                    "no session",
                ))
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_error_after_the_first_chunk_aborts_the_body() {
        let res = stream_export(
            "application/json",
            &limits(Duration::from_secs(60)),
            job(200_000, || Err(clax_core::CoreError::TaskFailed)),
        )
        .await
        .unwrap();
        assert!(body(res).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_panic_after_the_first_chunk_aborts_the_body() {
        let res = stream_export(
            "application/json",
            &limits(Duration::from_secs(60)),
            job(200_000, || panic!("a bug in the export")),
        )
        .await
        .unwrap();
        assert!(body(res).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalled_reader_ends_the_export_and_aborts_the_body() {
        // With no room to wait for, the job gives up as soon as the queue
        // is full, which it is: nothing reads the body until it has ended.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let res = stream_export("application/json", &limits(Duration::ZERO), move |mut w| {
            tokio::task::spawn_blocking(move || {
                let piece = vec![b'x'; EXPORT_CHUNK];
                let out = (0..40)
                    .try_for_each(|_| w.write_all(&piece))
                    .and_then(|()| w.flush());
                let _ = done_tx.send(out.is_err());
                out?;
                Ok(())
            })
        })
        .await
        .unwrap();
        assert!(done_rx.await.unwrap(), "the job gave up on its reader");
        assert!(body(res).await.is_err());
    }

    #[test]
    fn flags_are_true_or_false() {
        assert!(export_query("pretty=true&no_text=false").is_ok());
        for q in ["pretty=1", "no_text=0", "pretty=yes", "pretty"] {
            assert!(export_query(q).is_err(), "{q}");
        }
    }
}
