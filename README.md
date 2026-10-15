# Clax

Your coding agent can build you a page: a report, a dashboard, a prototype,
a chart. Clax gives those pages a home.

- **Agents publish.** Claude Code, Codex, Pi and Grok Build publish HTML
  pages to Clax, and every version is kept.
- **You look and comment.** Open the gallery in your browser, point at
  anything on a page, and leave a comment.
- **Agents act on it.** Send a comment to the agent and it gets it, makes
  the change, publishes a new version and replies.

Everything runs on your machine. People on your network can view and comment
too.

## Get started

Install the plugin for your agent:

- **Claude Code:** `/plugin marketplace add empathic/clax`, then
  `/plugin install clax@clax`
- **Codex:** `codex plugin marketplace add <clone of this repository>`, then
  `codex plugin add clax@clax`
- **Grok Build:** `grok plugin install <clone>/plugins/clax-grok --trust`
- **Pi:** `pi install <clone>/plugins/pi`

Then ask your agent to publish something.

## Clax on Cloudflare

The same Rust server also runs as a hosted Clax in a Cloudflare Worker
([`crates/clax-worker`](crates/clax-worker/README.md)): one Durable Object
holds the instance, with its SQLite database in the object's storage, and
each artifact's versions are commits in a git repository of its own
(Cloudflare Artifacts, or a local stand-in). Several people sign in, and
each person's agents connect over MCP with that person's rights.

Run it locally under `wrangler dev`, with two people, three agents and a git
clone, without a Cloudflare account:

```sh
just hosted-demo
```

Your own Clax, on your machine, can publish to a hosted one: `clax remote
add https://clax.example` signs it in there by device code (you approve
it in the browser, signed in as yourself), and `clax push <artifact>`, the
artifact's Publish button, or an agent's `publish_public` (once you
approve) publishes the artifact there; later pushes send its new versions
to the same public link. See "Publishing to a public Clax" in
[docs/usage.md](docs/usage.md).

[The writeup](docs/hosted/writeup.md) covers how it uses Workers, Durable
Objects, Artifacts, R2 and Static Assets, and what has not yet run on a real
deployment.

## More

- [Using and developing Clax](docs/usage.md)
- [The contract for agents and integrators](docs/contract.md)
