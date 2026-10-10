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

## More

- [Using and developing Clax](docs/usage.md)
- [The contract for agents and integrators](docs/contract.md)
