# `path p import clax` Implementation Plan (TOOLPATH-REPO WORK)

> **STATUS: FUTURE WORK, NOT SCHEDULED** (owner correction, 2026-10-06:
> "We don't have to worry about import right now because we're just
> recording Clax logs as toolpath.")
>
> Clax currently only records. This plan is kept for when a reader is
> wanted. Do not execute it until the owner schedules it.
>
> Decisions that already apply when it is scheduled (Clax spec O4, O5, O7):
> - `at-revision` goes into the correlation RFC (Task 7 is approved).
> - Deleted history is kept.
> - Redaction hashes are unsalted.
> - The kind is hosted at `https://toolpath.net/kinds/clax-audit/v1.0.0`.
> - Clax paths follow artifacts, not agent sessions.
> - Tool calls are joined by exact call ID, else by (tool name, canonical
>   argument hash, time window), for every harness.
>
> Where this plan and the Clax spec disagree, the spec wins. In particular,
> Clax writes no per-session paths; agent sessions are actors and `agent://`
> references only.

> **This plan is executed in the Toolpath repository** (`empathic/toolpath`),
> not in Clax. It lives in the Clax tree only so that it can be reviewed next
> to the spec it implements. When it is executed, copy it to the Toolpath
> repo's `docs/superpowers/plans/` and work on a branch there. Nothing in
> this plan touches the Clax repository.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Toolpath can import Clax's history and link it to what Toolpath already imports:

- `path p import clax` reads Clax's journal segments (`~/.clax/toolpath/journal/**/*.path.jsonl`) or a `clax toolpath export` document, and produces cached Toolpath documents;
- `p cache sync clax` keeps them fresh;
- a new `path p correlate` links Clax steps to harness-session paths and their tool calls, and to git paths, following the correlation RFC.

**Architecture:**
- A new provider crate, `toolpath-clax`, reads segments through the existing `Graph::from_jsonl_reader` and re-projects journal steps into per-artifact paths plus the install path. The rules are the same as the ones Clax's export uses (spec §8.2), and a shared golden fixture keeps the two equal.
- `path-cli` gains the import source, an `ArtifactType::Clax` sync source, and a `p correlate` command. That command implements the correlation RFC's algorithm, plus the Clax joins: agent session, `at-revision`, and tool call (exact ID, or tool name plus argument hash plus time window).

**Spec:** Clax repo `docs/superpowers/specs/2026-10-06-toolpath-audit-design.md` ("the Clax spec"). Toolpath's `RFC.md`, `docs/RFC-jsonl.md` and `docs/RFC-correlation.md`.

**Before Task 1:**
- Read the Toolpath repo's `CLAUDE.md`: the version-bump and new-crate checklists, and "Things to know".
- Read the Clax spec §7, §8, §9.4, §10 and §12.
- Copy the golden fixtures from Clax's `crates/clax-core/tests/toolpath/expected/` into `crates/toolpath-clax/tests/fixtures/`, with a `SOURCE` file naming the Clax commit.

## Global Constraints

- Follow Toolpath's conventions:
  - edition 2024;
  - `cargo clippy --workspace -- -D warnings`;
  - before calling the branch ready, run `scripts/quality_gates.sh` (or `just ci`); it is the gate.
  - every new crate goes through items 5–11 of the CLAUDE.md "adding a new crate" checklist;
  - every version bump goes through items 1–4.
- `path-cli` bumps minor, since this is a feature. `toolpath` and `toolpath-claude` bump patch for additive changes.
- Keep `docs/agents/formats/` in sync. Add `docs/agents/formats/clax.md`.
- Cache files are `0600` under `~/.toolpath/documents/`, and cache IDs come from `make_id`.
- Importers never write to the source. Clax's `~/.clax` is read-only to Toolpath.
- Commit and PR conventions are the Toolpath repo's own. Do not push or open PRs unless the owner asks.

## Review Focus

1. Projection parity: importing a journal yields the same artifact and install paths as Clax's own export of the same history. Pinned in Task 3 (`journal_projection_equals_clax_export_fixture`).
2. Correlation produces no false positives. A Clax step's HEAD never becomes `same-change` with a git step; it becomes `at-revision`. Pinned in Task 6 (`head_is_at_revision_not_same_change`).
3. Claude chain resolution: a Clax session recorded with a later segment's ID links to the chain head's path. Pinned in Task 5 (`later_segment_id_links_chain_head`).
4. `p correlate` is idempotent (correlation RFC). Pinned in Task 6 (`correlate_twice_is_noop`).

## File structure

```
crates/toolpath-clax/                    new crate: reader, projection, discovery
  src/lib.rs                             #![doc = include_str!("../README.md")]
  src/read.rs                            segments → steps (ordered by meta.clax.seq), exports → Graph
  src/project.rs                         artifact / install projections (Clax spec §8.2)
  src/discover.rs                        journal dir discovery, stat stamps for sync
  tests/fixtures/                        golden segments + exports copied from Clax
site/kinds/clax-audit/v1.0.0/{index.md,schema.json}
crates/path-cli/kinds/clax-audit/        schema.json (symlink target, bundled by p validate)
crates/path-cli/src/cmd_import.rs        ImportSource::Clax
crates/path-cli/src/artifact.rs          ArtifactType::Clax
crates/path-cli/src/sync/sources.rs      Clax ArtifactSource
crates/path-cli/src/cmd_correlate.rs     p correlate
crates/toolpath-claude/src/derive.rs     stamp chain segment IDs
docs/agents/formats/clax.md              format reference
docs/RFC-correlation.md                  amendment: at-revision (owner-approved, Clax spec O5)
```

---

### Task 1: The `clax-audit/v1.0.0` kind

**Files:**
- Create: `site/kinds/clax-audit/v1.0.0/index.md`, `site/kinds/clax-audit/v1.0.0/schema.json`, and `crates/path-cli/kinds/clax-audit/v1.0.0/schema.json`, with the symlink arrangement `agent-coding-session` uses
- Modify: `site/kinds/index.md` (table row), `crates/toolpath/src/types.rs` (`pub const PATH_KIND_CLAX_AUDIT: &str = "https://toolpath.net/kinds/clax-audit/v1.0.0";`), `crates/path-cli/src/kinds.rs` (registry)

The kind spec defines:
- path `meta.clax` (`projection`: journal, artifact or install; install; segment fields);
- step `meta.clax` (Clax spec §10.3);
- the `clax.<kind>` structural types and their fields (Clax spec §6);
- the `clax://` URI forms (§10.2);
- the `meta.refs` vocabulary (§10.4);
- the actor string forms (§10.1);
- the rules that steps chain linearly and that no step carries `meta.source`.

- [ ] **Step 1: Failing test.** `path kind clax-audit` prints the schema, and `p validate` accepts the Clax golden export and rejects a copy with a step missing `meta.clax.seq`.
- [ ] **Step 2: Write the kind spec and schema.**
- [ ] **Step 3: Run** `cargo test -p path-cli kinds` and `cd site && pnpm run build` (the page count goes up by one). Expected: PASS.
- [ ] **Step 4: Bump** `toolpath` (patch), then CHANGELOG and `site/_data/crates.json`. **Commit.**

---

### Task 2: Format reference

**Files:**
- Create: `docs/agents/formats/clax.md`
- Modify: `docs/agents/formats/README.md`

The reference documents:
- the journal location;
- segment naming;
- rotation;
- the `continues` chain;
- the recovery guarantees (no partial lines; `.damaged` files);
- the export shape;
- harness ID semantics (Clax spec §9.4, including the Claude chain caveat).

- [ ] **Step 1: Write the reference.**
- [ ] **Step 2: Commit.**

---

### Task 3: `toolpath-clax`, the reader and projections

**Files:**
- Create: the crate per the file structure, with its README
- Modify: the root `Cargo.toml` (members, workspace dependency), `CLAUDE.md` (layout, dependency graph), `README.md`, `site/_data/crates.json`, `site/pages/crates.md`, `scripts/release.sh` (`ALL_CRATES`, tier 2)

**Interfaces:**
- `read_journal(dir: &Path, range: Option<SeqRange>) -> Result<Vec<Step>>`:
  - reads every `*.path.jsonl` in name order;
  - skips `*.damaged` files, with a warning;
  - checks that `meta.kind` is `PATH_KIND_CLAX_AUDIT`;
  - orders steps by `meta.clax.seq` and de-duplicates by seq;
  - tolerates a final segment without `PathClose`.
- `read_export(path) -> Result<Graph>`.
- `project(steps, &ProjectOpts) -> Graph`: the same rules as the Clax spec §8.2 (linear chains, `same-change` on moves, `clax://` bases, path ordering; no session paths).
- `harness_provider(&str) -> &str`: `claude` maps to `claude-code`.

- [ ] **Step 1: Failing tests.**
  - `journal_projection_equals_clax_export_fixture`: compare the canonical JSON of the projected Graph with Clax's golden export of the same fixture history.
  - `reads_open_final_segment`.
  - `skips_damaged_with_warning`.
  - `rejects_foreign_kind`.
  - `dedups_overlapping_seq`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** `cargo test -p toolpath-clax`. Expected: PASS.
- [ ] **Step 4: Commit.**

---

### Task 4: `path p import clax` and `p cache sync clax`

**Files:**
- Modify: `crates/path-cli/src/cmd_import.rs`, `crates/path-cli/src/derive.rs`, `crates/path-cli/src/artifact.rs` (`ArtifactType::Clax`, name `clax`; not a `Harness`), `crates/path-cli/src/sync/sources.rs`, `crates/path-cli/src/cmd_list.rs` (`p list clax`)

**Interfaces:**
```
path p import clax [--journal <dir>]          (default: $CLAX_HOME/toolpath/journal, else ~/.clax/toolpath/journal)
                   [--input <export.path.json | segment.path.jsonl>]
                   [--artifact <ID>]... [--by-session <Clax or harness session ID>]...
                   [--since <t>] [--until <t>] [--all]
```
- **Default.** With no selector on a TTY, it opens a picker over artifacts (`fuzzy`), like the harness importers.
- **`--all`.** One cache document per artifact path, plus one for the install path.
- **Cache IDs.**
  - `clax-<install8>-a-<artifact ID>`;
  - `clax-<install8>-install`.
- **Sync.** Stat-level stamps: the summed size and maximum mtime of the journal's segments. Re-derive when they change, and always overwrite, as other sources do.

- [ ] **Step 1: Failing tests.**
  - `import_clax_all_writes_one_doc_per_path`.
  - `import_clax_from_export_file`.
  - `sync_skips_unchanged_journal`.
  - `p_list_clax_tsv`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** `cargo test -p path-cli`. Expected: PASS.
- [ ] **Step 4: Commit.**

---

### Task 5: Claude chain IDs on derived paths

**Files:**
- Modify: `crates/toolpath-convo/src/derive.rs` (when `view.session_ids.len() > 1`, set `meta.extra["session_ids"]`, oldest first), with tests, and `docs/agents/formats/claude-code/` if the derived shape is documented there

**Why:** Clax records the session ID the harness reports. After Claude Code rotates a session, that ID can be a later segment's, while the derived path is keyed by the chain head (Clax spec L11). Stamping the segment IDs lets `p correlate` join on cached documents without re-reading `~/.claude`.

- [ ] **Step 1: Failing tests.**
  - `chained_view_stamps_session_ids`.
  - `single_segment_has_no_session_ids`.
  - `later_segment_id_links_chain_head`: this one lives in Task 6's suite and uses a two-segment fixture.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Bump** `toolpath-convo` (patch), then CHANGELOG and `crates.json`. **Commit.**

---

### Task 6: `path p correlate`

**Files:**
- Create: `crates/path-cli/src/cmd_correlate.rs`
- Modify: `crates/path-cli/src/cmd_p.rs` (`PCommand::Correlate { input, output }`; reads one Graph from a file or `-`)

**Algorithm** (the correlation RFC, phases 1–4, plus the Clax joins):

1. **VCS revisions** (RFC phase 2): `same-change` between steps that share `meta.source.revision`.
2. **Agent session:** for each Clax step whose `agent-session` ref is `agent://<provider>/<ID>`, find the harness path with `meta.source == provider` whose conversation ID is `<ID>`, or whose `meta.extra.session_ids` contains it. If the ID is not found, resolve the chain from the step's `transcript` ref. Give the step's ref a `toolpath:` form. Clax paths are artifact paths, so this is a step-level link and never makes a session the container.
3. **`at-revision`:** for each Clax step ref `at-revision git:<uri>@<sha>`, find a git-derived path with `base.uri == <uri>` containing a step with `meta.source.revision == <sha>`. Add `{"rel":"at-revision","href":"toolpath:<path>/<step>"}`. Never add `same-change` for it.
4. **Tool calls** (Clax spec §12.2–§12.3), for Claude Code, Codex, Pi, Gemini, and Grok once Toolpath reads Grok:
   - pair `tool.call` and `tool.call_id` records with transcript tool calls, by exact `harness_call_id` first;
   - otherwise by (bare tool name, canonical argument hash, time window), computing the hash with the spec's JCS rule and checking it against the spec's vectors.

   For each pair, add `produces` on the harness step that made the call and `produced-by` on the Clax steps the call produced.
5. **Direction** (RFC phase 3) for the remaining pairs, then mark the graph `correlates` `self` (RFC phase 4).

- [ ] **Step 1: Failing tests,** with fixture graphs built from the Clax golden export, a small Claude fixture, and a `toolpath-git` fixture repository:
  - `correlate_twice_is_noop`.
  - `head_is_at_revision_not_same_change`.
  - `clax_step_links_claude_session`.
  - `later_segment_id_links_chain_head`.
  - `exact_call_id_pairs_first`.
  - `args_hash_join_within_window_per_harness`: one test each for Claude Code, Codex, Pi and Gemini.
  - `args_hash_vectors_match_clax`.
  - `ambiguous_calls_pair_in_time_order_never_guess`.
  - `revision_same_change_still_works`: the RFC example from `docs/RFC-correlation.md` round-trips.
  - `unknown_rels_preserved`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** `cargo test -p path-cli correlate`. Expected: PASS.
- [ ] **Step 4: Commit.**

---

### Task 7: Correlation RFC amendment (owner-approved, Clax spec O5)

**Files:**
- Modify: `docs/RFC-correlation.md`. Add `at-revision` to the step-level relationships: "This step was performed against a working tree at the target revision (possibly with uncommitted changes); it is not the same change." Explain why it is distinct from `same-change`, and add `tool-use` and `agent-session` as informative conventions.

- [ ] **Step 1: Edit the RFC.** Add an example built from the Clax fixture.
- [ ] **Step 2: Commit.**

---

### Task 8: Cross-harness checks and release

**Files:**
- Modify: `crates/path-cli/tests/cross_harness_matrix.rs`. Add Clax as an import-only source: import, validate, render md, and query. Clax is not a projection target.
- Modify: `crates/path-cli/tests/schema_examples.rs` (the Clax golden export validates). Add `examples/clax-export.path.json` (a small redacted fixture, run through `--no-text --no-names --no-paths`).
- Modify: `CHANGELOG.md`; bump `path-cli` (minor) and `toolpath-clax` (initial version); update `site/_data/crates.json`.

- [ ] **Step 1: Run** `scripts/quality_gates.sh`. Expected: every gate passes.
- [ ] **Step 2: Commit.** Report to the owner. Do not publish crates or push without being asked.
