# Tracing a request through Clax

Local-first: everything here runs on this machine with no account and no
hosted tracing product.

## OTLP export

`clax serve --foreground` sends its spans over OTLP/HTTP when
`OTEL_EXPORTER_OTLP_ENDPOINT` (or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) is set,
as service `OTEL_SERVICE_NAME`, by default `clax` (`clax_http::otel`). Unset,
no exporter or thread exists and each request costs one atomic load. A request
with a well-formed W3C `traceparent` continues that trace: its `request` span
is a child of the caller's span. A collector that is down loses spans and does
not affect the daemon. A trace is exported on the caller's say: `-00` in
`traceparent` means unsampled, and nothing is sent. The Worker's wasm32 build
has none of this.

A page's overlay sends a sampled `traceparent` on the report it files or
reopens, one trace per attempt, and passes the same trace to `auth` for each
token it asks for since the attempt before, so a site's token route can join it.
An attempt on a token minted for an earlier attempt has no mint in its trace.

## Request ids and causes

Every request the daemon answers has an id (`clax_http::request_id`): the
caller's `x-request-id` when it is 1–64 of `[A-Za-z0-9._-]`, else the trace ID
of its `traceparent`, else a new one (12 hex digits of epoch milliseconds, then
16 random). The answer carries it back as `x-request-id`. Requests of one trace
with no `x-request-id` of their own share the ID, so a `cause` then names the
trace rather than one request.

The handler runs under that id as its *cause* (`clax_core::cause`), and so
do the store jobs it hands off (`AppState::store_call*`): every event
published while it runs carries `"cause": "<id>"` beside its fields on
`/api/events`. Events no request caused (timers, watchers, a future
`tokio::spawn`ed from a handler) carry none. To keep a cause across a spawn,
wrap the spawned future in `clax_core::cause::scope(id, fut)` with
`clax_core::cause::current()`.

`RUST_LOG=info` lines inside a request carry a `request{id=… method=…
path=…}` span, so `grep <id>` over the log follows one request.

Not yet: the `/api/stream` WebSocket deltas do not carry `cause`.

## In the browser

- **Server-Timing.** Each answer has `Server-Timing: req;desc="<id>",
  app;dur=<ms>` (time inside the daemon until the answer's head). DevTools →
  Network → a request → *Timing* shows it under "Server Timing".
- **User Timing.** `causedFetch(label, url, init)` (`web/shell/src/events.ts`)
  sends a request under an id the page minted and marks
  `clax:req:<id>`; when an `/api/events` event naming that id arrives, the
  artifact view (`onArtifactEvent`) measures `clax <label> → <event type>`.
  DevTools → Performance → record: the measures sit in the *Timings* track
  beside the network waterfall. `patchArtifact` and `deleteArtifact` use it.
- **EventStream tab.** Network → the `/api/events` request → *EventStream*
  lists each SSE message (type, data with `cause`, id `<epoch>-<n>`).
- **Messages tab.** Network → the `/api/stream` request (WS) → *Messages*
  shows each WebSocket frame both ways.
- **Perfetto.** Performance panel → save profile (`.json`), open it at
  <https://ui.perfetto.dev> (runs in the page; nothing is uploaded) for a
  zoomable, queryable timeline with the User Timing measures.

## The agent hop

`just mcp-inspector [agent]` opens the official MCP Inspector (a local page)
on this checkout's `clax exec mcp` against the dev home, to call Clax's MCP
tools by hand and watch the events they cause arrive with their `cause`.

## CPU and SQLite

- **samply** (`cargo install samply`): `samply record ./target/debug/clax
  serve --foreground --port 7481` with `CLAX_HOME` set, drive the slow path,
  stop; it opens the Firefox Profiler locally. Build with
  `--profile release` plus `debug = true` for meaningful frames.
- **EXPLAIN QUERY PLAN**: open the home's database read-only,
  `sqlite3 -readonly "$CLAX_HOME/clax.db"`, then
  `EXPLAIN QUERY PLAN <the query>;` — a `SCAN` of a large table where a
  `SEARCH … USING INDEX` was meant is the usual finding.
  `PRAGMA compile_options;` lists how the bundled SQLite was built.
