//! OTLP export of the daemon's spans: on when `OTEL_EXPORTER_OTLP_ENDPOINT`
//! (or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) names a collector, off otherwise.
//!
//! Off, nothing is built: [`from_env`] gives no layer, and [`accept_parent`]
//! is one atomic load per request. On, a batch exporter on its own thread
//! sends every span the log filter admits over OTLP/HTTP, as service
//! `OTEL_SERVICE_NAME` (by default `clax`), and each `request` span
//! continues the trace of the caller's `traceparent`
//! ([W3C Trace Context](https://www.w3.org/TR/trace-context/)). The sampler is
//! the default, parent-based: a caller that marks its trace unsampled is
//! not exported. A collector that is down loses spans; the daemon is not
//! affected.

use axum::http::HeaderMap;
use opentelemetry::propagation::{Extractor, TextMapPropagator as _};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing_opentelemetry::{OpenTelemetryLayer, OpenTelemetrySpanExt as _};

/// Whether a layer is installed, so a request skips the parsing without one.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Flushes what is still queued when dropped. Hold it for the life of the
/// daemon.
#[must_use = "dropping the guard stops the export"]
pub struct Guard(Option<SdkTracerProvider>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(provider) = self.0.take() {
            ACTIVE.store(false, Ordering::Relaxed);
            if let Err(e) = provider.shutdown() {
                tracing::warn!(error = %e, "could not flush the last spans");
            }
        }
    }
}

/// The layer that exports spans and its guard, when the environment names a
/// collector. A collector the exporter cannot be built for is reported on
/// stderr and leaves the export off: telemetry does not stop the daemon.
pub fn from_env<S>() -> (Option<OpenTelemetryLayer<S, SdkTracer>>, Guard)
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let configured = |k: &str| std::env::var(k).is_ok_and(|v| !v.trim().is_empty());
    if !configured("OTEL_EXPORTER_OTLP_ENDPOINT")
        && !configured("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
    {
        return (None, Guard(None));
    }
    // The exporter reads both endpoint variables and adds `/v1/traces`.
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("clax: OTLP export is off: {e}");
            return (None, Guard(None));
        }
    };
    let name = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "clax".to_owned());
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::builder().with_service_name(name).build())
        .build();
    let (layer, guard) = install(provider);
    (Some(layer), guard)
}

/// The layer for `provider`, which from here on requests continue the
/// traces of.
fn install<S>(provider: SdkTracerProvider) -> (OpenTelemetryLayer<S, SdkTracer>, Guard)
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("clax"));
    ACTIVE.store(true, Ordering::Relaxed);
    (layer, Guard(Some(provider)))
}

/// Makes `span` continue the trace `headers` names, when the export is on
/// and they name one. Call it before the span is first entered.
pub fn accept_parent(span: &tracing::Span, headers: &HeaderMap) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let cx = TraceContextPropagator::new().extract(&Headers(headers));
    // Without a well-formed `traceparent` this context has no span, and the
    // request starts a trace of its own.
    let _ = span.set_parent(cx);
}

struct Headers<'a>(&'a HeaderMap);

impl Extractor for Headers<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::trace::{SpanData, SpanExporter};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt as _;

    #[derive(Clone, Debug, Default)]
    struct Collected(Arc<Mutex<Vec<SpanData>>>);

    impl SpanExporter for Collected {
        async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
            self.0.lock().unwrap().extend(batch);
            Ok(())
        }
    }

    fn headers(traceparent: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("traceparent", traceparent.parse().unwrap());
        h
    }

    #[test]
    fn a_span_continues_the_callers_trace() {
        let out = Collected::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(out.clone())
            .build();
        let (layer, guard) = install(provider);
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            let span = tracing::info_span!("request", "otel.kind" = "server");
            accept_parent(
                &span,
                &headers("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            );
            span.in_scope(|| {});
            drop(span);
            let other = tracing::info_span!("request");
            accept_parent(&other, &headers("not a traceparent"));
            other.in_scope(|| {});
        });
        drop(guard);
        let spans = out.0.lock().unwrap();
        assert_eq!(spans.len(), 2);
        let continued = &spans[0];
        assert_eq!(
            continued.span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(continued.parent_span_id.to_string(), "00f067aa0ba902b7");
        assert_eq!(continued.span_kind, opentelemetry::trace::SpanKind::Server);
        let own = &spans[1];
        assert_ne!(
            own.span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }
}
