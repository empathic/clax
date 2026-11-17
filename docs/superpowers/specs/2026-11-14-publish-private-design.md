# Publish private: an agent publishes to clax.dev in one step, approved once

Status: draft for the owner's review, 2026-11-14.

Lines marked **(owner)** are what the owner said. Lines marked **(call)**
are choices this document makes; the owner has not agreed to them yet.

## 1. Why

Some people think of Clax as clax.dev. They installed the plugin, they
have an account, and they want their agent to make pages there and work
on pages there. The localhost daemon confuses them and they should never
have to think about it. **(owner)**

The owner's own case: every pull request gets a page that explains it,
published to clax.dev. Today each of those pages asks for an approval,
and the approval is the same decision every time. **(owner)**

What exists today (paths as of main `f6a6563d5`):

- An agent publishes locally with `publish`, then calls `publish_public`
  (`crates/clax-mcp/src/tools.rs`), which pushes the local artifact to a
  connected public Clax. Two calls.
- Each new page asks the person a question in the localhost inbox
  (`crates/clax-server/src/routes/remotes.rs`, the push approval). The
  only standing approval is per page: `remote_links.auto`, set by the
  answer "Publish and push new versions". Nothing covers every page.
- A pushed copy is already restricted to its owner until shared. The
  question's text says "whoever its link reaches can read it", which is
  not true of a new copy.
- Results that wait for the person carry a localhost inbox URL.
- With a clax.dev URL an agent can already read a page, read and answer
  its comments, and watch it. Tools that would change it answer
  `not_on_remote`.
- `docs/superpowers/specs/2026-10-26-quickstart-onboarding-design.md`
  §10 proposed "always publish to clax.dev" and was not built. This
  design replaces that section.

## 2. Goals

1. One tool call puts a page on clax.dev and returns its clax.dev link.
   **(owner)**
2. The tool the agent and the person see is named `publish_private`: the
   name says nobody else can see the page, and an agent in an automatic
   permission mode is not asked to do something called "public".
   **(owner)**
3. The person approves once, on a clax.dev page, signed in there, that
   this machine's agents may publish private pages to their account.
   After that no page asks. **(owner)**
4. Nothing in this flow shows the person a localhost URL or sends them
   to the localhost inbox. **(owner)**
5. The local daemon may stay in the path as plumbing. **(owner)**
6. An agent given a clax.dev link to a page the person can edit can
   publish a new version of it. **(owner: "connect directly to a
   clax.dev artifact for collaboration"; the extent is a call, §6.)**

## 3. Non-goals

- Sharing. Letting a reviewer see a page uses the existing mechanisms,
  unchanged. **(owner)**
- Removing the local daemon from the path. The plugin still starts
  `clax mcp` against the local daemon. **(call)**
- Local-first use. `publish`, `publish_public` and `clax push` keep
  working as they do. **(call)**
- A page's database (`db_*`) and assets on a page that lives only on
  clax.dev. **(call)**

## 4. Anti-goals

The change has failed, even if it works, when any of these is true:

- a `publish_private` result, or a message it tells the agent to show,
  contains a localhost URL;
- a person who approved once is asked again for a later page;
- a page published by `publish_private` can be opened by anyone but its
  owner without a separate decision of the owner's;
- an agent on the machine can grant the standing approval itself.

## 5. The standing approval

### 5.1 What it is **(call)**

A permission on the agent token a public Clax hands a machine at device
sign-in: `agent_publish`. In words, on the approval page: "Agents on
this machine may publish pages to your account. Only you can open them
until you share them."

It is kept by the public Clax, on the token's grant, listed with the
machine on the person's People page, and turned on and off there. It has
three values: `unasked` (the person was never asked), `on`, and `off`
(the person chose "Connect without this", denied a request for it, or
turned it off on People). `off` is the person's answer and stays until
they turn it on, on People: no agent's call asks again (§6.1). The local
daemon keeps a copy of the last value it was told, for the tool's first
check only; the public Clax's answer decides.

It covers a new page, which starts restricted to its owner, and new
versions of a page published this way. Whether that includes a page the
owner has since shared is **open (§11)**. It never covers a decision that widens who may open a page (link
access, listing, a share, comments on, a site opened to the world):
those stay the owner's, as "Browser confirmations" in `docs/contract.md`
has them.

### 5.2 How it is given **(call, on the owner's "a clax.dev page where
they log in and approve")**

Through the existing device sign-in (`docs/contract.md`, "Publishing to
a public Clax"), which is already a page on the public Clax where the
person signs in and approves this machine:

- `POST /api/hosted/device/code` takes `wants: ["agent_publish"]`.
- The approval page (`/device`) names what is asked, as §5.1 words it,
  next to the machine and the account. Approving approves both the
  machine and the permission. The page offers "Connect without this"
  for a sign-in that asks for it: the machine connects, the permission
  stays off.
- The token answer (`POST /api/hosted/device/token`) carries
  `agent_publish: true | false`.
- `GET /.well-known/clax` lists `agent_publish` in `capabilities`, so a
  local daemon knows the public Clax offers it.

A machine already connected asks for the permission on the token it
has; it gets no new token. It sends the same `POST
/api/hosted/device/code` with `wants`, this time with its agent token as
the bearer. The public Clax ties that request to the token's grant
(`device_codes.grant_id` exists already, `crates/clax-store/src/device.rs`),
and the approval page says "Allow <machine>, already connected, to
publish pages to your account" in place of the connect words. Only the
grant's own person may decide it. Approving sets the grant's permission
to `on`; denying sets it to `off`; the machine's poll answers the value
and issues nothing. The local remote's row, token, links, linked
threads, mirrors and per-page approvals are untouched throughout, and
the local daemon keeps only the request's device code while it waits. A
request that expires leaves the value as it was.

The approval link is the sign-in's `verification_uri_complete`
(`crates/clax-server/src/hosted.rs`), which carries the user code: the
person clicks and approves, and types nothing. The user code is shown
beside it only so the person can see the page is the one their agent
asked for.

A public Clax that refuses `wants` as an unknown field does not offer the
permission (its `capabilities` say so first); §6.1 says what the tool
answers then.

A person who used the shell's Publish button or `clax remote add` is
offered the permission the same way: those sign-ins ask for it too.

### 5.3 Who may start that sign-in **(call)**

Today only the owner starts a device sign-in; an agent asks through
`connect_instance`, and the owner answers in the localhost inbox
(`docs/superpowers/specs/2026-10-29-remote-instances-design.md`: an
agent's call alone must never connect).

`publish_private` may start a device sign-in itself, without that
localhost answer, for one instance only: the default one, `[publish]
remote` in `config.toml`, which is `https://clax.dev` unless the owner
set another. The agent's call still does not connect anything: the
person's approval on the public Clax does, signed in there, on a page
that names the machine and its address and warns when the sign-in was
asked from another address.

This is a new path an agent may call, beside the owner's `POST
/api/remotes`: it accepts no origin, only the default instance, and asks
for no localhost confirmation ("Browser confirmations" decision 2 keeps
covering `POST /api/remotes` and every other origin). It is limited per
session as connection requests are.

Any other instance keeps today's rule: `connect_instance`, answered in
the owner's browser.

### 5.4 Where it is enforced **(call)**

- The public Clax refuses a publish made under a token without the
  permission when the request says it is an agent's and carries no
  per-page approval: 403 `agent_publish_not_allowed`. The local daemon
  marks such requests (it sends no such mark today:
  `crates/clax-server/src/public.rs`). This makes turning the permission
  off on People take effect at the next publish, with no polling. It is
  a record of the person's choice that an honest daemon obeys, not a
  barrier: the token sits in the daemon's database, which anything
  running as the person's OS user can read.
- The local daemon skips its push question for an agent's push when the
  remote's token has the permission and the push does not widen access.
- A push made with the bare daemon token and no session (`clax push`,
  "Browser confirmations" decision 1) is unchanged: it waits for its
  confirmation as today.

An agent that holds the local daemon token can already push as the
owner's CLI (the open concern on
`crates/clax-server/src/routes/remotes.rs` about `CLAX_CHANNEL=owner`).
This design does not change that and does not depend on it.

## 6. The tool

### 6.1 `publish_private`

Arguments, as `publish` takes them (an HTML string or a file, supporting
files, a title), plus:

- `url_or_id` and `if_version`, to publish a new version of a page;
- `timeout_s`, how long to wait for the person's first approval.

It takes no instance argument: it publishes to the default instance
(§5.3). **(call)**

What it does:

1. Asks the local daemon whether the default instance is connected with
   the permission. This comes first: nothing is made before the person
   approved.
2. If not, starts the sign-in (§5.2) and returns at once when it cannot
   wait, or waits up to `timeout_s`:
   `{status: "approval_needed", approve_url, user_code, tell_person,
   call_again: true}`. `approve_url` is the public Clax's approval page
   with the code in it (§5.2): one click, nothing to type. `tell_person`
   is one sentence for the agent to show, naming clax.dev and what
   approving allows. The same call made again waits on the same sign-in.
3. Once approved, publishes: a local artifact and version as `publish`
   makes them, pushed to the default instance with new versions
   following, in the one call. The local copy is the daemon's own
   business: no result names it.
4. Answers `{status: "published", url, artifact_id, version, if_version,
   visibility: "private", who_can_open: "only you"}`. `url` is the
   page's address on the public Clax. `artifact_id` is the public copy's.

The states the tool can find, and what it answers **(call)**:

| The default instance is | The tool |
|---|---|
| not connected | starts the sign-in, asking for the permission (step 2) |
| connected, permission `unasked` | asks for it on the token it has (§5.2, step 2) |
| connected, permission `off` | `{status: "turned_off", settings_url, tell_person}`: the person's People page on the public Clax, where they turn it on. It starts no request, in this session or any other |
| too old to offer the permission | `{status: "not_supported"}` and a sentence; it never falls back to the localhost question |

**Versions.** `version` and `if_version` are the public copy's numbers,
and `read` of such a page gives the public head. The tool keeps the
local numbers to itself.

**A failed push.** The tool answers an error, `remote_unreachable` (or
the public Clax's own refusal), and the agent makes the same call again.
There is no half-published state for it to carry: a new page whose push
failed leaves no local artifact behind, and a new version whose push
failed stays as the local head, which the next call pushes when its
content is the same and replaces when it is not. Nothing is published
twice, and no result carries a local ID.

**The public copy moved** (someone else published a version there):
`remote_moved` with a sentence, until step 3 (§6.4), which brings the
head into the local copy first.

**A page that declares capabilities.** Its approval lives in the local
shell today. `publish_private` refuses such a page with a sentence that
names `publish`; moving that approval is later work.

### 6.2 The tool's words **(call; exact wording is the builder's)**

The description says, in this order: it publishes a page to the person's
Clax account; only the person can open it until they share it; the first
use asks them once, on clax.dev; it returns the page's link. It does not
use the word "public".

`publish_public` keeps its name and behaviour. Its question for a new
page stops saying "whoever its link reaches can read it" and says who
can open a new copy: only its owner.

### 6.3 Later calls on a page published this way **(call)**

A page made by `publish_private` is marked on its link as living on the
public Clax. For such a page every MCP tool result gives the public
Clax's `url` and `artifact_id` where it gives a localhost one today
(`watch`, `read`, `list`, `comments_*`, `open`, `status`), and `open`
opens the public page. A page pushed with `publish_public` or `clax
push` keeps today's results.

Comments made on the public page already reach the local daemon and wake
the agent (`crates/clax-server/src/linked.rs`); that is unchanged.

### 6.4 A page that lives only on clax.dev **(call)**

`publish_private` with a clax.dev URL of a page this machine did not
push, which the person may edit: the local daemon first takes the page's
head version into a local artifact linked to it (marked as §6.3), then
publishes the new version and pushes it as an update. `if_version` is
checked against the public copy's head; a head that moved answers
`remote_moved`, as a push does today.

A page the person may not edit answers `not_permitted`. `db_*` and
assets on such a page still answer `not_on_remote`.

This part can ship after the rest (§9).

### 6.5 How an agent knows to use it **(call)**

When the default instance is connected with the permission, the
session-start context the plugin prints and the `status` tool say so in
one line: pages are published with `publish_private`. The clax skill
says the same and tells an agent whose person asks for a page "on
clax.dev" to use it. Without the permission nothing changes for agents
that publish locally.

## 7. Settings **(call)**

`config.toml`:

```toml
[publish]
remote = "https://clax.dev"   # the default instance; the owner's to change
```

No `always` switch: an agent chooses `publish` or `publish_private` per
page, and §6.5 steers it. Section 10 of the quickstart design is marked
superseded by this document.

## 8. Constraints

- The Worker serves the public side from the same code
  (`crates/clax-worker`); every public-side change here applies to both.
- New columns take the next free migration numbers on main, with their
  marks, as `AGENTS.md` and `crates/clax-store/src/migrations.rs`
  require: the permission on the public side's grants, what a device
  code asked for, the local remote's last known permission and its
  waiting request's device code (§5.2), and the link's mark (§6.3).
- `docs/contract.md` and `docs/usage.md` change with the code: the
  device sign-in, the agent's sign-in path (§5.3), the push routes, the
  MCP tool table, "Browser confirmations" decision 2, and
  `not_on_remote`.
- The plugins that list tools (`plugins/clax`, `plugins/pi`, and the
  others under `plugins/`) carry the new tool and the skill's text.
- Tests follow the repository's rules: fake public Clax from
  `clax-testkit`, injected clocks, no sleeps.

## 9. Order

1. The permission: public side, device sign-in, People page, the local
   daemon's copy, enforcement (§5).
2. `publish_private` for new pages and their new versions, with the
   remote-first results (§6.1 to §6.3, §6.5, §7).
3. Publishing to a page that lives only on clax.dev (§6.4).

Each step is useful alone and can be its own pull request.

## 10. Left to the builder

- Names of columns, routes' exact shapes beyond what §5.2 fixes, and the
  wording of every sentence shown.
- How the tool waits on the sign-in (reusing the remote poll route).
- Whether step 3's local copy reuses the Clax app's pull.
- The People page's layout for the permission.

## 11. Open, for the owner

1. A page the owner shared after it was published (a pull request's page
   sent to its reviewers): does the standing approval cover its new
   versions? Covered, an agent's update reaches the people it was shared
   with unasked, as a page set to follow new versions does today. Not
   covered, each update of a shared page asks again.
2. A page that declares capabilities is refused by `publish_private`
   (§6.1), because that approval lives in the local shell. For a person
   who uses only clax.dev that is a dead end. Either it stays refused
   until that approval has a home on the public Clax (this document's
   choice), or that move joins this work as a fourth step.
