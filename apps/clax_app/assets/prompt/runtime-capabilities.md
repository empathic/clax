## Runtime capabilities

A page reaches runtime capabilities with `await window.claude.use(name)`,
exactly as on claude.ai. The type definitions of contract 0.2.61 are the
contract: before writing a page, fetch the one you use from your daemon,
`<daemon_url>/_clax/contract/0.2.61/<name>.d.ts`, where `daemon_url`
comes from the `status` tool (names: `claude`, `permissions`, `artifact`,
`db`, `downloads`, `user`, `comments`, `assets`, `room`, `sample`).

Declare what the page uses in `capabilities` on `publish`, for example
`{"db": {}, "user": {"scopes": ["profile"]}}`. The object is the full set:
passing it replaces the stored one, omitting it keeps it, and `{}` clears it.
`use()` never rejects: it resolves `null` for a name the page did not declare,
for `files` and `mcp`, and outside the Clax viewer, so render without the
capability first and light features up when it resolves.
`permissions` and `user` need no declaration.

- `artifact` (alias `self`): `publish(html)` saves a complete document
  (starting `<!doctype html>`) as a new version, and every open view reloads
  to it. Only the person's own browser can publish; other views reject
  `not_writer`.
- `db`: shared JSON documents at paths such as `tasks/t1`, live through
  `onSnapshot`; page writes are last-writer-wins. `data/users/<id>/` is
  private to the viewer whose `user.id()` is `<id>`. Levels: agents and
  scripts with the token are `owner`, the person's browser is `admin`, a
  viewer on another machine who entered a name is `interact`, one who did
  not is `view`.
- `downloads`: `save({filename, data})` saves once the viewer accepts.
- `user`: `isOwner()`, `canEdit()`, `can(name)`, and `me()` need no
  declaration; `id()` and `profiles(ids)` need `{"user": {}}`; names and
  `search(q)` need `{"user": {"scopes": ["profile"]}}`. IDs are opaque. The
  person is one user in all their browsers on this machine: `id()` is the
  same in Chrome and Safari (and is who the CLI acts as), and `isOwner()` is
  true in each.
- `comments`: `openComposer({element})` opens the viewer's composer; with
  `{}` (not `{"composer_only": true}`) the page may also `create`, `reply`,
  `resolve`, `delete`, and `sendToClaude` as the viewer after one consent;
  `{"customAnchors": true}` lets a canvas-like page place pins itself.
- `assets`: `upload`, `list`, `delete`, in the person's browser only.
- `room`: `emit`, `on`, `presence`, `peers`, `onPeers`, `join`, `connected`,
  and `onConnection` reach every view of the artifact that is open now;
  nothing is stored. Presence works for every viewer. A topic declared
  `{"room": {"topics": {"<topic>": "interact"}}}` may be sent on by a viewer
  on another machine who entered a name; every other topic only by the
  person's browser.
- `sample`: `sample(input, options)`, `sample.json`, and `sample.limits` ask
  Claude with the API key configured on the person's machine, and only in
  the person's own browser: everywhere else, and when no key is configured,
  `use("sample")` resolves `null`. The first call in each view asks the
  person to allow it.
- `permissions`: `state()` and `request()`; the only prompt is the consent
  to the first page-written comment.

The viewer's gesture: `openComposer` and `compose` open the composer only
within about five seconds of the viewer's input in the page, with focus in
the page and the pointer moved onto it (or a Tab into it) since their last
input to the Clax window; otherwise they resolve `{opened: false}`.
`create`, `reply`, `resolve`, `delete`, `sendToClaude`, and
`artifact.publish` also need no input to the Clax window (its buttons, name
field, composer, dialogs) in the last 5.5 seconds: inside that time they
reject `shell_input_recent` with nothing written, so show a message and let
the viewer click again. That code is Clax's own: it and Clax's other
additions are declared in `<daemon_url>/_clax/contract/clax-extensions.d.ts`,
beside the unchanged 0.2.61 files. Call these only from a click or key press in the page,
never on load or a timer.

### Differences from claude.ai

What a page written for claude.ai meets in Clax, beyond the gesture rules:

- Everywhere: nobody is a guest, and there are no organizations or public
  links.
- `permissions`: allowing is remembered in the viewer's browser for the
  artifact, across reloads and versions. "Don't allow" lasts until the Clax
  tab is reloaded or shows another version; there is no standing refusal.
- `artifact`: `publish` needs the viewer's gesture (above); one tab may
  publish an artifact once every 2 seconds and 10 times a minute, then gets
  `rate_limited`. Called from a page other than `index.html`, it replaces
  that page's file and carries the others forward. The files form rejects
  `capability_disabled`; `edit` and `sync` reject `invalid_content`.
- `db`: no latency compensation: `hasPendingWrites` and `fromCache` are
  always `false`, and a page's own write shows in its snapshots once the
  daemon confirms it. The person's browser is `admin`, not `owner`, though
  `isOwner()` is true there: a rule at `owner` shuts out every browser,
  theirs included, so use `admin` for "only the person". The only quota is
  5000 documents per artifact.
- `user`: `can()` is fixed at its first call: after a viewer on another
  machine enters a name, it keeps its old answer until the page reloads,
  though their writes already succeed, so reload or try the write.
  `profiles()` caches names for the life of the page and never refreshes
  them. `can()` never answers `null`, `email` is always `null`, and
  `search()` works only in the person's browser.
- `downloads`: at most one prompt at a time and 3 per 30 seconds;
  `request` (export answers) always rejects `request_unknown`.
- `comments`: `canSendToClaude()` answers `available` while the publishing
  agent session is live, `no_session` otherwise (never `writers_only`), and
  `off` under `composer_only`. There is no batch form of `sendToClaude`: one
  call sends one comment. `reply`, `resolve`, `delete`, and
  `sendToClaude({threadId})` act only on threads the page created in the
  current page load (`not_found` otherwise). On another machine, reopening
  and `delete` need the viewer to have entered a name (`forbidden`
  otherwise). Page-written comments show "via the page", and an `@agent` in
  them sends nothing. Per tab, composer opens are limited to 5 per 10
  seconds, page writes to 10 a minute, and gesture refusals to 20 a minute.
- `room`: there are no agent peers (`kind` is always `"viewer"`);
  `sendToClaudeSession` rejects `claude_unavailable` and
  `canSendToClaudeSession` answers `"off"`. A new version does not empty
  the room: each view stays on its version until it reloads. A viewer's
  level is fixed per connection; entering a name reconnects at the new level.
- `sample`: the person's own API key pays for every call, not the viewer's
  account, so only their browser can call it. Allowing lasts for the view,
  not for the artifact. Answers are cached per browser in the daemon and
  forgotten, with the day's call counts, when it restarts. A daemon
  restart under an open tab ends its calls with `session_expired`.

