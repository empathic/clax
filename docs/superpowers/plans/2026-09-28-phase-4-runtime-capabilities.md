# Artifax Phase 4: Runtime Capabilities — Scoped Plan

> **Status:** scoped plan. Expand to step-level tasks with the writing-plans skill when phase 3 has shipped. Interfaces and acceptance criteria are the commitments.
>
> **For agentic workers:** when expanded, use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** A page written for claude.ai's runtime contract 0.2.61 using `permissions`, `artifact` (and `self`), `db`, `downloads`, `user`, `comments`, and `assets` runs unchanged in Artifax, in both frame modes.

**Architecture:** The bridge implements `claude.use(name)` by requesting a grant from the shell over postMessage; the shell holds grants per viewer per artifact, talks to the daemon, and relays `db` snapshots from SSE. `db` documents and rules live in the daemon. The `.d.ts` files from claude.ai are the contract and are shipped in `web/contract/` and referenced from the skills.

**Spec:** §5 (`docs`, `viewers`), §6 (Docs routes), §9 entire including the capability table, §12 `db_*` tools, §14 caller levels, §17 "Phase 4".

**Depends on:** phase 3 threads (for `comments`), viewers, and the shell/bridge protocol.

## Global constraints (additions)

- `use()` never rejects; undeclared or unavailable names resolve `null`; resolution happens after the page's first script run (microtask at minimum).
- Resolved namespaces are frozen. Permission prompts happen on first call, never on `use()`.
- `db` writes require `if_version` on existing documents; last-writer-wins; `data/users/<id>/` is private to that viewer; rules evaluate with levels `view < interact < admin < owner`, owner shell on localhost is `admin`, named LAN viewer `interact`, unnamed `view`.
- The `artifact` capability republishes the whole document with `if_version` = shown version; `conflict` reloads to the winner.
- The declared `capabilities` object is a full-set declaration; omitted keeps, `{}` clears (already accepted by publish in phase 1; PATCH gains the field here).

## Tasks

### Task 1: `docs` storage and rules engine
- Migration 4: `docs(artifact_id, path, json, version, updated_at)`; `Store::doc_get/set/update/delete/list/query(where, order_by, limit, cursor)/batch`, `str_replace`; `rules::evaluate(declared_rules, path, op, caller_level) -> bool`; lease `acquire({holder})` with 30 s TTL in a `leases` table.
- Acceptance: unit tests for every operator in `where`, cursor paging, `if_version` failure naming the current version, `__delete__` field removal, rules precedence (most specific path wins), private user subtree.

### Task 2: Docs routes and `db_*` tools
- Routes per spec §6 "Docs"; caller level from token (admin) or viewer cookie + name (interact) or none (view); `as_level` query param narrows. SSE `doc` events carry path and version.
- MCP tools `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch` (up to 50, atomic).
- Acceptance: integration tests for levels and `as_level`; shim tests for batch atomicity.

### Task 3: Shell grant manager and bridge handshake
- Protocol: `artifax:use {name, id}` → `artifax:use-result {id, granted: bool, config}`; `artifax:call {ns, method, args, id}` → `artifax:call-result {id, ok, value|error{code,message}}`; `artifax:event {ns, topic, data}` for snapshots. Grants stored in shell `localStorage` per viewer per artifact; one consent dialog per capability per page load; denial final for the load.
- Bridge: namespace objects generated from the `.d.ts` method lists, frozen; `permissions.state()/request()` built in.
- Acceptance: vitest for the bridge state machine; Playwright: `use("db")` resolves `null` when undeclared and an object when declared; prompt appears once.

### Task 4: `artifact` (and `self`), `downloads`, `user`, `assets`
- `artifact.publish(html)`: shell posts a new version with `if_version`; on 201 every open view reloads (SSE `version`); on 409 rejects `conflict` and reloads; LAN viewer rejects `not_writer`. Wrapper recognises the republished full document (phase 1 rule).
- `downloads.save({filename, data})`: shell confirm dialog then a Blob download.
- `user`: `id()/me()/isOwner()/canEdit()/can()/profiles()/search()` from the viewer cookie and `viewers` table; `guest` always `false`.
- `assets`: `upload/list/delete` for the owner shell only.
- Acceptance: Playwright with a poll page ported from claude.ai: vote → republish → both tabs reload to the same state; a downloads button saves a file; `user.isOwner()` true on localhost, false on the LAN host.

### Task 5: `db` capability in the bridge
- `db.doc(path)`/`collection(path)` with get/set/update/delete/where/orderBy/limit/onSnapshot; snapshots via the shell's SSE subscription filtered by path prefix; `acquire`.
- Acceptance: Playwright: two tabs, a write in one appears in the other's `onSnapshot` within 500 ms; a stale `if_version` write rejects.

### Task 6: `comments` capability
- `composer_only`: `openComposer({element}|{range})` opens the phase 3 composer anchored at the element; full form adds write verbs as the viewer; `customAnchors()` registers named anchors the phase 3 resolver understands (`kind: "custom"`).
- Acceptance: Playwright: a page button opens the composer anchored on its element; a custom anchor thread survives a republish.

### Task 7: Contract docs and skills
- `web/contract/0.2.61/*.d.ts` committed; `docs/contract.md` describes each capability's behaviour and differences from claude.ai (no `files`, no `mcp`, `guest` always false, LAN mode limits). Skills in all three plugins reference it. `PATCH /api/artifacts/{aid}` accepts `capabilities`.

## Ship criteria
Three claude.ai pages (a poll using `artifact`, a tracker using `db` with rules, a page using `downloads` and `user`) run unchanged in both frame modes with the same observable behaviour; the `db_*` tools seed and read the tracker; `docs/contract.md` is complete.
