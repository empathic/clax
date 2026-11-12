//! The Worker's log: `tracing`'s events at `WARN` and above, one line each
//! on the console, which Cloudflare keeps as the Worker's log (`wrangler
//! tail`, the dashboard's Logs). Without a subscriber every `tracing` event
//! is dropped, among them the reason behind an `internal` answer
//! (`clax_http::error`'s `io`). Spans are not kept: an event's line is its
//! level, its target, its message and its fields.

use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Installs the panic hook and the console log; once per isolate. Each
/// entry point (the Worker's handlers, each object's `new`) calls it.
pub(crate) fn init() {
    console_error_panic_hook::set_once();
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Console(emit));
    });
}

/// One line of the log: the console on wasm32, standard error natively.
fn emit(level: Level, line: &str) {
    #[cfg(target_arch = "wasm32")]
    if level == Level::ERROR {
        worker::console_error!("{line}");
    } else {
        worker::console_warn!("{line}");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = level;
        eprintln!("{line}");
    }
}

/// A subscriber that hands each event at `WARN` and above, as one line, to
/// its sink.
struct Console<F>(F);

impl<F: Fn(Level, &str) + Send + Sync + 'static> Subscriber for Console<F> {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        *m.level() <= Level::WARN
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::WARN)
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, e: &Event<'_>) {
        let mut f = Fields::default();
        e.record(&mut f);
        let m = e.metadata();
        (self.0)(
            *m.level(),
            &format!("{} {}: {}{}", m.level(), m.target(), f.message, f.rest),
        );
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// An event's message, and its other fields as ` name=value`.
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.rest.push_str(&format!(" {}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message.push_str(&format!("{value:?}"));
        } else {
            self.rest.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn warnings_and_errors_become_one_line_each_and_the_rest_is_dropped() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        let console = Console(move |level: Level, line: &str| {
            sink.lock().unwrap().push((level, line.to_string()));
        });
        tracing::subscriber::with_default(console, || {
            let e = std::io::Error::other("RepoUnavailable: token refused");
            tracing::error!(error = %e, "io");
            tracing::warn!(artifact = "p7j7pykg6zvg", "read interrupted");
            tracing::info!("not kept");
            tracing::debug!("nor this");
        });
        let target = module_path!();
        assert_eq!(
            *lines.lock().unwrap(),
            vec![
                (
                    Level::ERROR,
                    format!("ERROR {target}: io error=RepoUnavailable: token refused")
                ),
                (
                    Level::WARN,
                    format!("WARN {target}: read interrupted artifact=p7j7pykg6zvg")
                ),
            ]
        );
    }
}
