# Artifax Phase 5: Room and Sample — Scoped Plan

> **Status:** scoped plan. Expand to step-level tasks with the writing-plans skill when phase 4 has shipped. These are the least load-bearing features and the most likely to change; treat the interfaces below as the current best guess, re-read the 0.2.61 `.d.ts` files at expansion time, and re-check the Anthropic Messages API reference (the `claude-api` skill) before writing the provider.

> **For agentic workers:** when expanded, use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Pages can reach everyone who has them open right now (`room`) and ask an LLM (`sample`) with the claude.ai call shapes.

**Architecture:** One WebSocket per content frame to `/api/artifacts/<aid>/room`, relayed through the shell in opaque-origin mode; in-memory rooms with presence, topics gated by caller level, `join(name)` sub-rooms; nothing persisted. `sample` is a daemon route that streams from a provider trait; the Anthropic provider uses the Messages API with streaming and tool use round-tripped to the page.

**Spec:** §6 (Sample, Room), §9 `room` and `sample`, §14 spend and consent, §17 "Phase 5", §18 `sample()` cost control.

**Depends on:** phase 4 grant manager and bridge protocol; viewer levels.

## Global constraints (additions)

- `room` messages and presence are untrusted; the page must work with `room` resolving `null`.
- `sample` resolves `null` when no provider key is configured; the first call asks consent per viewer per artifact; the shell shows a running call count; `modelTier` maps to configured model IDs from `config.toml` (`sample.models.quick|default|complex`), defaulting to the current Claude model IDs at expansion time.
- Errors reject `{code, message, text?}` with codes `not_granted`, `rate_limited`, `cancelled`, `provider_error`; partial text is returned on cancellation.

## Tasks

### Task 1: Room server
- `tokio::sync::broadcast` per `(artifact, room_name)`; WebSocket handler with `emit`, `presence`, `join`, `leave` messages; peers list with `you`, `kind` (`viewer` or `agent`), `guest: false`; topic permission from declared `room.topics` and caller level (admin default). Messages may drop under backpressure (bounded channel, oldest dropped).
- Acceptance: integration test with two WebSocket clients: presence fan-out, topic gate refusal for `view` level, sub-room isolation.

### Task 2: Room in the bridge and shell
- Per-origin mode: the frame opens the WebSocket directly. Opaque mode: the shell owns the socket and relays over postMessage.
- Acceptance: Playwright, two tabs: cursor presence appears in both; `leave()` removes a peer.

### Task 3: Provider trait and Anthropic implementation
- `trait SampleProvider { fn stream(&self, req: SampleRequest) -> BoxStream<SampleEvent> }` with `SampleRequest { messages, tools, model, max_tokens, cache_key }`; Anthropic implementation over the Messages API with SSE streaming, tool_use blocks emitted as `ToolCall` events, and a 5-minute in-memory response cache keyed by request hash unless `cache: false`.
- Config: `[sample] provider = "anthropic"`, `api_key_env = "ANTHROPIC_API_KEY"`, `[sample.models] quick/default/complex`, optional `daily_call_cap`.
- Acceptance: unit tests with a mocked HTTP server for streaming, tool calls, rate-limit mapping, and cache hits; no live API calls in CI.

### Task 4: Sample route and bridge
- `POST /api/artifacts/<aid>/sample` streams SSE `text`, `tool_call`, `done`, `error`; tool results come back via `POST .../sample/<call_id>/tool_result`; the bridge implements `sample(input, opts)` with `onText` (whole text so far), `signal`, `tools` (execute in page, post results), `images` when the provider reports support, `sample.json`, `sample.limits()`.
- Shell: consent dialog on first call, call counter, cap enforcement with `rate_limited`.
- Acceptance: Playwright against a stub provider (config `provider = "stub"` returning canned streams): streaming text renders progressively; a tool round-trip completes; abort yields `cancelled` with partial text; no key → `use("sample")` is `null`.

### Task 5: Docs and skills
- `docs/contract.md` sections for `room` and `sample`, including what differs from claude.ai (viewer pays → the local key pays; `guest` never true). Skills updated in all three plugins.

## Ship criteria
A two-tab room demo shows presence and events; a `sample()` demo streams from the Anthropic provider with a configured key and resolves `null` cleanly without one; the daily cap, if configured, stops calls with `rate_limited`.
