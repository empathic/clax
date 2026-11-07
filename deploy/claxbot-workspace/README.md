# ClaxBot workspace profiles

`clax.toml`, `changelist.toml` and `pathbase.toml` are `workspace.toml` files
for ClaxBot, filled in from how each repository really deploys its previews.
Each is a whole file, one target: `image` and `domain_allow_list` belong to
the file, not the target, so a daemon serving several of these repositories
needs one image with every toolchain, and the union of the hosts. The format
is `crates/clax-bot-workspace/src/config.rs`; the design is
`docs/superpowers/specs/2026-11-06-claxbot-workspace-design.md`.

These are the operator's route, where a file names the repository. A queue's
admin can connect one from Clax instead (`docs/usage.md`, "Connecting a private
repository"): the file then needs only `[runtime]`, `image` and
`domain_allow_list`, the owner sets the App once, and each link is looked up per
run:

```toml
image = "ghcr.io/you/build-image:latest"
domain_allow_list = ["github.com", "codeload.github.com"]

[runtime]
kind = "daytona"
```

A daemon reads the file when it starts, so restart it after adding or changing
one. For a queue with both, the file's target is what runs. A private repository's
link waits for a queue admin to accept that its CI may run ClaxBot's branches; a
target here has no such step, so naming a repository here is that acceptance,
and its operator reads what its CI runs on a pull request first.

**Changelist and Pathbase are not safe to turn on yet:** their previews run
any branch of the repository with no approval. Each file's header says what
that reaches and what comes first.

| | Preview | Ready | Names the commit | Publishes a build image |
|---|---|---|---|---|
| Clax | `https://pr-{pr}.clax-preview.dev` | `/healthz` | yes | no |
| Changelist | `https://changelist-pr-{pr}.fly.dev` | `/api/health` | **no** | no |
| Pathbase | `https://pathbase-pr-{pr}.fly.dev` | `/api/v1/health` | **no** | no |

## To use one

1. Replace `queue` with the queue's ID. A target whose queue matches no run
   does nothing.
2. Build an image with what the file's comment lists, and set `image`. For
   Clax that is `clax-image.Dockerfile`, here. On Daytona `image` names a snapshot
   ("A snapshot on Daytona"), and a home with none fails at startup.
3. Make the GitHub App below and put its tokens in the variables the file
   names. For Daytona, also `DAYTONA_API_KEY`, `DAYTONA_API_URL` and
   `DAYTONA_ORGANIZATION_ID`.
4. Add the rulesets below.
5. On Clax, set the repository variable `CLAXBOT_LOGIN` to the App's login,
   `<app-slug>[bot]`. Without it a renamed ClaxBot branch drops the preview
   gate (`docs/environments.md`, "Credentials").

## A snapshot on Daytona

Daytona starts a sandbox from a snapshot, so `image` is a snapshot's name and not
an image's. With none a sandbox is Daytona's default of 1 vCPU, 1 GiB and 3 GiB,
which builds nothing, so a home on Daytona with no `image`, or a blank one, gets no
containers: the service will not start, and a daemon starts with ClaxBot's panel
saying its workspace failed. That holds with no target too, as a linked repository
runs on the file's `image`. As Daytona's documentation reads on 2026-11-06:

```
docker build --platform linux/amd64 -t clax-workspace:1 \
  -f deploy/claxbot-workspace/clax-image.Dockerfile deploy/claxbot-workspace
daytona login --api-key "$DAYTONA_API_KEY"
daytona snapshot push clax-workspace:1 --name clax-workspace-1 \
  --cpu 4 --memory 8 --disk 10
```

- **Size is the snapshot's.** `--cpu`, `--memory` and `--disk`, in cores and GB, are
  "allocated to the underlying sandboxes", and default to 1, 1 and 3. A cold build
  of Clax peaked near 7 GB of memory and 6.7 GB of disk, and 4, 8 and 10 is also
  the most an organization gets unless Daytona raises it. Daytona's API lists the
  same fields on `POST /sandbox`, but its SDKs take them only with an image and
  never with a snapshot, so nothing here sends them: to change a size, push another
  snapshot.
- **x86-64.** A local image must be AMD64, so build with `--platform linux/amd64`
  on any other host. On arm64 a Clax build fails on the Android NDK
  (`MODULE.bazel`'s toolchains).
- **A tag.** `latest`, `lts` and `stable` are refused. Bump the tag and the name for
  each image, and set `image = "clax-workspace-1"`.
- **It goes idle.** A snapshot unused for two weeks turns inactive and is activated
  by hand. A sandbox that will not start after a quiet spell is this.

Record `8ab1456e` (`crates/clax-bot-workspace/src/daytona.rs`) holds what only a real
sandbox shows of this.

## The workspace service

A Worker runs no container, so hosted Clax reaches ClaxBot's containers through
`clax-bot-workspace-service`, on a host of your own. This section is that host;
`docs/prod.md`, "ClaxBot's workspace service", is the Worker's side and the order
of the steps. The service holds the runtime's key and the GitHub App's, and no
model key, no store and no credential into Clax. It serves the `workspace.toml`
in its home alone: a repository a queue's admin connects in Clax does not run here.

**Build it** at the commit the Worker was built from. The two speak a versioned
wire, and a skew is refused with `wire_skew` (426), so rebuild when a release
changes it. Nothing publishes this image; `service.Dockerfile` has not yet been
built by anyone, so its first build is also its test:

```
git checkout v<release>
docker build -f deploy/claxbot-workspace/service.Dockerfile -t clax-workspace-service .
```

On a Linux host with no Docker, `bazelisk build
//crates/clax-bot-workspace-service:clax-bot-workspace-service` makes the same
binary at `bazel-bin/crates/clax-bot-workspace-service/`; it needs `git` on PATH.

**Its home** is a directory that outlives the container, owned by UID 10001 for
the image. Copy `clax.toml` there as `workspace.toml` and fill it in ("To use
one"). It also holds:

- `workspace-id`, this home's label on every container: a new home neither finds
  nor ends what an old one left, and those run until `auto_stop_minutes`;
- `workspace-runs/`, the run ledger, with the daily push count: a lost home resets
  `max_pushes_per_day`;
- `workspace-jobs.log`, the commands begun, so that a command is not run twice
  across a restart;
- `workspace-previews.json`, what became of a fix's preview, until the Worker has
  told it.

Clones are in the temporary directory, and are not kept.

**Its environment:**

| Variable | |
|---|---|
| `CLAX_WORKSPACE_SERVICE_TOKEN` | Required. The bearer secret, at least 32 bytes: `openssl rand -hex 32`. The Worker's secret of the same name holds the same value |
| `CLAX_WORKSPACE_SERVICE_HOME` | Required; the image sets `/var/lib/clax-workspace` |
| `CLAX_WORKSPACE_SERVICE_INSTANCE` | Required: the one instance it serves, of letters, digits, `_` and `-`. It must be what the Worker names, and a request naming another is refused with `wrong_instance` (403). `clax-prod` here and in `docs/prod.md` |
| `CLAX_WORKSPACE_SERVICE_ADDR` | Optional; `127.0.0.1:7490` |
| `DAYTONA_API_KEY` | Required with `kind = "daytona"`. `DAYTONA_API_URL` and `DAYTONA_ORGANIZATION_ID` where you need them |
| `CLAXBOT_CLAX_APP_KEY` | The GitHub App's private key, as `clax.toml`'s `private_key_env` names it: the PEM GitHub gave, `BEGIN RSA PRIVATE KEY`, whole |

**Run it** on the host's network, so that it stays on loopback with no port
published and `cloudflared` reaches it there (Linux; Docker Desktop has no such
network). The PEM has several lines, which an `--env-file` cannot hold, so it
goes by name:

```
export CLAXBOT_CLAX_APP_KEY="$(cat /etc/clax-workspace/app.pem)"
docker run -d --name clax-workspace --restart unless-stopped --network host \
  -v /var/lib/clax-workspace:/var/lib/clax-workspace \
  --env-file /etc/clax-workspace/env -e CLAXBOT_CLAX_APP_KEY \
  clax-workspace-service
```

`/etc/clax-workspace/env`, mode 0600, holds the rest, one `NAME=value` a line. It
reads `workspace.toml` when it starts, so restart it after a change. A restart
forgets every job, and a run reads a command it was waiting on as lost: do it with
no run going.

**Check it** before anything fronts it. Keep the secret out of argv by putting
the headers in a file, mode 0600:

```
Authorization: Bearer <the secret>
clax-workspace-wire: 1
clax-workspace-instance: clax-prod
```

```
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7490/v1/previews   # 401
curl -s -H @/etc/clax-workspace/headers http://127.0.0.1:7490/v1/previews | jq .snapshot.queues
```

The second names each queue the file serves: if `clax-feedback`'s ID is not
there, no run on it gets a shell. Both answers were read from the binary with
fake keys. It lists nothing of Daytona or GitHub, so this does not show either key
works; "First live run" does.

**The tunnel** is Cloudflare Tunnel: `cloudflared` dials out, on port 7844, and
the host needs no inbound port. As Cloudflare's documentation reads on 2026-11-06:

1. Under **Networking**, **Tunnels**, **Create a tunnel**, and name it.
2. Install `cloudflared` as the dashboard says, and run it with the token in a
   file rather than in argv, which needs `cloudflared` 2025.4.0 or later:
   `cloudflared tunnel run --token-file /etc/clax-workspace/tunnel-token`.
   `TUNNEL_TOKEN` in the environment does the same.
3. On the tunnel's **Routes** tab, **Add route**, **Published application**: a
   hostname on a zone of the Cloudflare account, with the service URL
   `http://localhost:7490`. Not under the site domain, whose `*` route is the
   Worker's.
4. Wait for **Healthy** under **Tunnels**.

`https://<that hostname>` is `CLAX_WORKSPACE_SERVICE_URL`.

**Put Access in front of it.** Cloudflare says a published application with no
Access application "will be available to anyone on the Internet", so the default
is Access with a service token, and the bearer secret behind it as a second
factor:

1. Under **Access**, **Service credentials**, **Service Tokens**, create a token
   for this instance. Its client ID and client secret are shown once.
2. Add a self-hosted Access application on the hostname, with one policy of action
   **Service Auth** that includes that token and nothing else.
3. On the tunnel's route, turn on **Protect with Access**. The service does not
   read Access's own token, so this is what makes the tunnel refuse a request
   Access did not pass.
4. Give the Worker the two as the secrets `CLAX_WORKSPACE_ACCESS_CLIENT_ID` and
   `CLAX_WORKSPACE_ACCESS_CLIENT_SECRET` (`docs/prod.md`, "ClaxBot's workspace
   service"). It sends them on every request, as `CF-Access-Client-Id` and
   `CF-Access-Client-Secret`, beside `Authorization: Bearer`. Both or neither: one
   alone is a workspace that did not start.

Ask it, at `https://<that hostname>`, with a file like the one above holding the
two headers as well:

```
curl -s -o /dev/null -w '%{http_code}\n' https://<hostname>/v1/previews                  # 403, Access's
curl -s -H @/etc/clax-workspace/headers -H @/etc/clax-workspace/access \
  https://<hostname>/v1/previews | jq .snapshot.queues
```

The first answering 401 means the request got past Access to the service, and the
hostname is open. A refusal by Access is a page of its own and not the service's
word, so a Worker with the headers wrong reads it as the service being out, and
waits; the service's own 401 ends a call at once. To rotate, add a second token to
the policy, put the two secrets, then remove the first. Nothing of this has run
against a real tunnel or a real Access application.

## The GitHub App

One App, installed on the target repositories alone.

| Permission | Level | For |
|---|---|---|
| Contents | Read and write | pushing branches under `claxbot/` |
| Pull requests | Read and write | finding and opening its pull request |
| Metadata | Read | required of every App |

Nothing else. In particular:

- **The list slows a change; only a person's review binds it.** Every path
  a model may write compiles into the same Worker, so no list of files can say
  what a change does: writable code can call protected code, and a model can
  say so in any words. Read the diff before `/preview`, and before a merge.
- **`clax.toml` limits what a change may touch.** `writable_paths` is an
  allow-list and `protected_paths` subtracts from it, on both sides of a rename.
  `preview.yml`'s `approve` reads the same list from the default branch and
  checks the head again through GitHub's compare API, so a `/preview` does not
  rest on the daemon's word. `path-cases.txt` holds the two matchers to one
  answer, and `scripts/check-claxbot-paths.py tree` fails on an entry the tree no
  longer has. A person who needs a refused path moves the change to a branch of
  their own.
- **`refused_in_rust` is a tripwire, and a person's review is the control.** It
  catches the plain `#[path]` pointing a `mod` at a protected file, and only the
  daemon checks it. A macro, a raw string or an `include!` of another extension
  gets past it, and a writable `lib.rs` can swap a module with no attribute at
  all. Its patterns are not meant to grow: read the diff.
- **No `workflows` permission.** GitHub then refuses a push that changes
  `.github/workflows/`, whatever `protected_paths` says. On a pull request's
  own events and on the deploy's dispatch, a run takes `preview.yml` from the
  branch, so Clax's preview gate depends on a branch not being able to edit it.
- **No Actions permission.** Clax's deploy starts from a dispatch, and only
  its `approve` job may make one.
- **No bypass of a ruleset.** Contents: write reaches every branch and every
  tag, and ClaxBot should be able to push neither its target's main line nor
  a release. A tag `v*` starts `release.yml` and `fleet-image.yml` on Clax, so
  a tag is a release build. `scripts/setup-prod-github.sh` makes rulesets for
  `prod` and `release/*` and for nothing else, so a deployment has to add one
  for `main` and one for the tags that start a workflow (on Clax `v*`, and
  `sdk-v*`, which `publish-sdk.yml` trusts), with the App in neither's bypass
  list, and the same on any other target.

Pull requests: write also lets the token comment. On Clax `preview.yml`
refuses a `/preview` whose sender is not a person with write access, so ClaxBot
cannot approve its own commit (`docs/environments.md`, "Credentials").

The daemon mints the App's tokens itself, with `[target.github_app]` or a link:
a write token that stays in the daemon, and for each container's clone a read
token, `permissions: {"contents": "read"}` on the one repository, revoked when
the clone returns.

A fine-grained personal token with the permissions above is the other route. Its
owner is then the pull request's author, whom `preview.yml` refuses, so someone
else has to approve; `CLAXBOT_LOGIN` is then that owner's login, and all their
own pull requests are gated with ClaxBot's. A read token for that route is a
second token, in `read_token_env`: it goes into a container, so it is never the
write token (`Config::parse` refuses the same variable for both).

## Workflows have to start

A pull request made with a workflow's `GITHUB_TOKEN` starts its workflows only
once someone approves them, and other events it makes start none, so it would
get no CI and no preview. These profiles name a `[target.github_app]` instead:
the daemon mints the App's own short-lived tokens, which GitHub documents as
the way to have them run.

## Signed commits

With `[target.github_app]`, GitHub makes each commit through its API and signs
it, and the daemon publishes it only once GitHub says it is verified. The App
needs Contents and Pull requests, read and write, on the repository, and no
Workflows permission; its private key stays in the daemon. `write_token_env` is
the other route, and excludes the App. A commit made with a token is pushed
unsigned, and a target proposes nothing that way unless it says
`allow_unsigned_commits = true`, which is for a demo's or a local forge's
repository. A token link connected in Clax pushes unsigned commits too, and the
panel says so: a repository that requires signatures refuses them.

## First live run

Everything above has run against fakes (`node demo/claxbot-workspace/run.ts`).
Nothing has yet run against Daytona or GitHub. In this order, each deciding
something a fake cannot:

1. Daytona, with its key in the environment, keeps the runtime's contract:

   ```
   DAYTONA_API_KEY=… bazelisk test //crates/clax-bot-workspace:conformance \
     --test_env=DAYTONA_API_KEY --test_arg=live_daytona --test_output=all
   ```

   Add `--test_env=DAYTONA_API_URL --test_env=DAYTONA_ORGANIZATION_ID` where
   those are set. What only this run settles is on record `8ab1456e`
   (`crates/clax-bot-workspace/src/daytona.rs`): an absent `exitCode` meaning 0,
   repeating `start` on a 5xx, label keys with dots, how `labels` filters, how its
   rate limits show, and a snapshot's size reaching its sandbox.
2. On a daemon, make the GitHub App as above, on a throwaway private repository
   with a preview, and set it under **Admin**, **Repositories**. Connect the
   repository from the queue's panel, approve it, and check the approval line shows
   the account you expect. A Worker refuses a queue's own link: on hosted Clax
   `docs/prod.md`, "ClaxBot's workspace service", steps 2 to 14 replace this step
   and the next, and what they show settles the same records.
3. Send one report to ClaxBot and watch one proposal through. Record `358e1d23`
   (`crates/clax-bot-workspace/src/forge.rs`) is what it decides:
   - GitHub marks the App's API commit `verified`, within the three reads two
     seconds apart. If it settles later, every commit is refused and stays on no
     branch: raise `VERIFY_ATTEMPTS` or `VERIFY_DELAY`.
   - An installation token narrowed by `repositories` and `permissions` still
     counts for that, and `DELETE /installation/token` answers 204 for one.
   - The status a non-fast-forward `PATCH` and an existing ref's `POST` answer.
   - `git fetch` sees a ref straight after the API moved it.
   - A file replaced by a directory of the same name goes through `base_tree`;
     a wrong answer is refused, and nothing lands.
   - The pull request starts `preview.yml`, and a ref moved by `PATCH` fires
     `synchronize` (record `bf8b6db0`).
4. Close those records with what the run showed.
