# Linked threads: a local artifact's comments in step with its clax.dev copy

Status: approved by the owner, 2026-10-15.

## Problem

A local artifact can be linked to a copy on a hosted Clax: `clax remote
add`, `clax push`, and linked publishes (`crates/clax-core/src/store/remotes.rs`).
Today the link only sends versions up. Threads on the hosted copy never reach the local
daemon, so the Clax plugin's `watch` and `wait_for_feedback` (local only)
can't hear them. The workaround is a second MCP server pointed at the
hosted `/mcp` with an agent token, polled by hand. It splits where fixes are
published and loses live notifications.

## Decided

- **Fully two-way:**
  - Threads started on the hosted copy come down to the local artifact.
  - Threads started locally on a linked artifact go up and appear on the
    hosted copy under the owner's name.
  - Replies and resolves go both ways.
- **Same rules as local:**
  - Every hosted thread mirrors down and shows locally.
  - Local agents get the ones a `watch` covers, or that someone sent to
    the owner's agents, exactly as for local comments.
  - The hosted daemon's own rules (rights, approvals, the visibility
    filter) decide what reaches the link at all. The link sees what the
    owner's device token sees.
- **A per-link switch:** each link is two-way by default. The owner can
  set it to download only (`clax remote` and the link's row in the local
  shell). Then local threads stay local, while replies and resolves on
  threads that came from the remote still go up.
- **Shown inline:**
  - Mirrored threads sit in the same Threads list and inbox as local ones.
  - Each carries a small tag naming the remote ("clax.dev") and the
    commenter's name there.
  - Replying or resolving from local acts on the hosted copy.

## Design

### Transport

- **The credential:** the local daemon already holds the device
  sign-in's agent token for each remote (`remotes.rs`, "the agent token
  it holds as that person"). The sync acts with that token only, over
  the hosted daemon's existing routes, and adds no new credential.
- **One sync task per remote** with at least one linked artifact. It
  keeps an MCP session on the hosted `/mcp`, `watch`es the linked
  artifacts, and long-polls `wait_for_feedback` for liveness.
- **Each wake-up, and every 60 s as a backstop,** it reads the linked
  artifacts' threads since a stored cursor.
- **Offline:** when the remote is unreachable, the task backs off
  (capped at 5 min) and the shell's link row says "clax.dev unreachable
  since …". Local work never waits on it.

### Storage (next free migration on main)

- **`linked_threads`:** remote, local thread ID, remote thread ID, and
  which side the thread started on.
- **`linked_comments`:** remote, local comment ID, remote comment ID,
  and an idempotency key. The key is made locally before the up-send,
  so a resend after a lost answer, or a comment's echo coming back
  down, maps to the same row and never duplicates.
- **`linked_outbox`:** the local replies, resolves and new threads not
  yet acknowledged by the remote, retried in order per thread. Each row
  is deleted when acknowledged.
- **The per-remote cursor** lives beside the link.

### Authors

- **Remote authors are display-only.** A mirrored comment from someone
  on the remote is stored with author kind `remote`: the remote's name
  plus that person's or agent's public ID and display name there. It is
  never mapped to a local person, agent or rights.
- **Local visibility of mirrored threads:** the owner and the owner's
  agents see them, as the owner's own artifact's threads. They are
  shown to no other local person unless that person can read the
  artifact locally.
- **Origin is kept:**
  - Comments the owner or their agents make locally go up as the device
    token's person, marked with the local agent's name.
  - Comments that came down from the remote never go back up.

### Anchors and versions

- **Translating pins:** each push records the local version and the
  remote version it made (`pushed_n` / `remote_n`). A mirrored anchor's
  version is translated through that record.
- **A remote version with no local twin** (published on the remote
  directly) keeps its remote version number, and the local view shows
  the thread as "on clax.dev v<n>" with an open-there link.
- **Selectors** (element, area, range) are carried as they are; both
  sides render the same HTML for twinned versions.
- **Local threads going up:** a local thread on a version never pushed
  is held until that version is pushed, or goes up as a whole-artifact
  comment if it was on the whole artifact.

### Clips and snapshots

- **Images:** a mirrored comment's clip or snapshot image is fetched over
  the remote's content route with the token and cached under
  `<home>/remote-cache/clips/`.
- **Paths:** a path a remote daemon sends is never passed to an agent or
  opened. This also closes the open record on `crates/clax-mcp/src/tools.rs`
  ("In remote mode the agent is shown whatever clip path the remote
  daemon sends").

### Ending a link

- **Unlinking or removing the remote** stops the task.
- **Mirrored threads stay locally** as read-only history, tagged
  "unlinked".
- **The outbox:** unsent outbox rows are shown to the owner and dropped
  only on the owner's confirmation.

### UI

- **The tag:** thread cards and inbox rows carry a remote tag ("clax.dev")
  and the commenter's remote name.
- **Pending items** show "Sending to clax.dev…" until acknowledged, and
  "Not sent: clax.dev unreachable" with Retry after the backoff cap.
- **Phone width and dark mode** are covered like every card.

## Tests

- **Store:**
  - idempotent mapping;
  - an echo of an up-sent comment maps to the same row;
  - outbox order per thread;
  - anchor translation through `pushed_n` / `remote_n`;
  - a version with no twin.
- **Two daemons** (a local daemon and a hosted test daemon in-process, as
  `crates/clax-server/tests/api_hosted.rs` sets up hosted):
  - a hosted comment reaches a local `wait_for_feedback`;
  - a local reply appears on the hosted thread exactly once, also after a
    simulated lost answer;
  - a local thread appears on the hosted copy;
  - a resolve propagates both ways;
  - a remote outage queues, then drains in order;
  - a remote's clip path is never handed to an agent.
- **Visibility:** another local person who can't read the artifact sees
  none of the mirrored threads.
- **Browser:** the tag on a mirrored thread, the pending and failed
  states, and replying from local shown on the hosted page; at 1280 and
  390 wide, light and dark.
