# `clax queue`: owner verbs for report queues, a plan

Status: plan only, nothing built. Asked for through claude-break-glass,
attributed to Bobby ("could clax have a local CLI that helps with this?");
that attribution is unconfirmed, and the verbs write secrets, so nothing is
built until Bobby's own word comes through work-assistant.

## Why

A queue and its service grant are made in a browser today (spec
2026-10-27-clax-sdk-design §5.3). The grant's answer holds a token and a
private key shown once (web/sdk/src/types.ts `ServiceGrantCreated`), which
a person then copies into their backend's secret store. Break-glass did
this by hand: POST /api/queues/{qid}/service-grants piped into
`op item create`. These verbs do it with no secret on screen.

## Verbs

```
clax queue create <name> --origin <url> [--origin <url>…] [--reporters-see own|all]
clax queue list
clax queue origins <queue> --add <url> | --remove <url>
clax queue grant <queue> --name <grant name> (--to-1password <vault>/<item> | --json-template)
clax queue grants <queue>
clax queue revoke <queue> <grant ID>
```

- `create`: POST /api/queues (crates/clax-server/src/router.rs:156). Prints
  the queue's ID and origins. The body's fields are those `queues::create`
  takes, to be read before building; `--origin` is normalised by the
  daemon (clax-core queue.rs `normalize_origin`).
- `list`, `grants`: GET /api/queues and GET …/service-grants (router.rs
  156, 167): IDs, names, origins, created_at. Never a token or key: the
  list routes return none.
- `origins`: PATCH /api/queues/{qid} (router.rs:158).
- `grant`: POST /api/queues/{qid}/service-grants (router.rs:167). Its answer
  goes to one sink and is never printed:
  - `--to-1password <vault>/<item>`: runs `op item create` (or
    `op item edit` when the item exists, with `--force` to overwrite) with
    the item's JSON on `op`'s **stdin** (`op item create --template -`),
    never in argv, so no process listing or shell history holds it. Fields:
    `instance`, `queue`, `token` (concealed), `private_key` (concealed),
    `grant_id`. Fails before calling the daemon if `op` is missing or not
    signed in (`op whoami`), so no grant is made that nothing can keep.
  - `--json-template`: writes that same item template to stdout, for the
    person's own `op item create --template -`. Refused when stdout is a
    terminal (`isatty`), so it is never drawn on screen.
  - Neither: refused, naming the two flags. There is no flag that prints
    the secret.
  - If the sink fails after the grant was made, the verb revokes that grant
    (DELETE …/service-grants/{gid}, router.rs:171) and says so: a grant whose
    secret nobody holds is no use and should not stay live.
- `revoke`: DELETE …/service-grants/{gid}.

Errors print the daemon's `error.code` and message (agents are refused
these routes: 403 `not_for_agents`, docs/contract.md 5555-5563, so the verbs
run as the owner's own CLI session, not an agent's).

## Where it lands

- `crates/clax-commands/src/commands/queue.rs` (the verbs), wired in
  `commands/mod.rs` and `crates/clax-cli/src/main.rs` as the other verbs are.
  Licence: GPL-2.0-only (LICENSE 23-31), as the rest of the CLI.
- How a CLI verb reaches the daemon as the owner (its local credential and
  client) follows the existing owner verbs (people.rs, tags.rs); to be read
  before building.
- No new dependency: `op` is run as a child process
  (`std::process::Command`, stdin piped), and isatty comes from the
  standard library (`std::io::IsTerminal`).

## Tests

- Unit (Rust, `commands/queue.rs`): argument parsing; the template's JSON
  (field names, concealed types); `--json-template` refused on a TTY (a
  test seam for IsTerminal); no flag combination prints a token or key
  (assert on captured stdout and stderr with a fake daemon answer); the
  revoke-on-sink-failure path.
- With the daemon (crates/clax-server tests' harness, as api_queue_reporter
  does): create, list, origins, grant, revoke against a scratch store; the
  grant's token then mints a reporter token (POST …/reporter-tokens), so the
  stored secret is shown to work.
- `op` faked by a script on PATH in tests: it records its argv (asserted to
  hold no secret) and its stdin (asserted to be the template).

## What it cannot test

- A real 1Password vault: tests fake `op`. A first real run needs a person
  signed in to `op`, by hand, once.
- `op`'s own behaviour across versions (template flags have changed before).
- Whether the person's terminal or tooling logs stdout when
  `--json-template` is piped somewhere unexpected.

## Open questions for Bobby

- Is 1Password the only store to support, or also a generic
  `--to-command <cmd>` (secret on that command's stdin)?
- Should `grant` be able to replace an existing item's secret (rotate),
  revoking the old grant after the new one is stored?
