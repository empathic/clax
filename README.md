# Clax

Your coding agent can build you a page: a report, a dashboard, a prototype,
a chart. Clax gives those pages a home on your own machine.

- **Agents publish.** Claude Code, Codex, Pi and Grok Build publish HTML
  pages to Clax, and every version is kept.
- **You look and comment.** Open the gallery in your browser, point at
  anything on a page, and leave a comment.
- **Agents act on it.** Send a comment to the agent and it gets it, makes
  the change, publishes a new version and replies.

Everything runs locally, at <http://localhost:7480>. Nothing needs an account.

## Install

### Claude Code

In Claude Code:

```
/plugin marketplace add empathic/clax
/plugin install clax@clax
```

That's all. The plugin downloads the Clax release it needs on first use
(about 10 MB, checked against a pinned checksum) and starts Clax when an
agent first calls it.

In Manual mode, Claude Code asks before each Clax tool call until you allow
them. To allow them all, add `"mcp__plugin_clax_clax__*"` to
`permissions.allow` in `~/.claude/settings.json`, or pick what to allow
with `/permissions`. The [plugin's README](plugins/claude-code/README.md#allowing-the-tools)
lists the read-only tools if you'd rather allow just those.

### Codex, Pi and Grok Build

Install the `clax` command, then let it set up every harness it finds on
your `PATH`:

```sh
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
clax init
```

`install.sh` puts `clax` in `~/.local/bin` after checking it against the
release's checksums. `clax init` registers the Clax plugin with Claude
Code, Codex, Pi and Grok Build, whichever are installed, and sets up the
Chrome extension. Start a new session in each harness afterwards. Run the
same two commands again to upgrade.

A few things differ by harness:

- **Codex** stops to ask before tools that replace or remove data.
  `clax init` offers, once, to approve those for you.
- **Grok Build** asks before every tool call. For headless runs, pass
  `--always-approve` or `--allow 'MCPTool(clax_grok__*)'`.
- **Pi** and **Codex** wake on a new comment by themselves, even when idle.

You can also install each plugin from a clone of this repository, with no
build needed. See [Install](docs/usage.md#install) in the usage guide.

Prebuilt releases cover macOS (Apple silicon and Intel) and Linux (x86-64
and arm64).

## Use it

1. **Ask your agent for a page.** "Make me a dashboard of this week's test
   failures", "sketch three layouts for the settings screen", "chart the
   benchmark results". The agent publishes the page to Clax and gives you a
   link.
2. **Open it.** Follow the link, or open the gallery at
   <http://localhost:7480> to see everything your agents have published.
3. **Comment.** Point at an element, select some text or drag over part of
   an image, write what you want changed, and send it to the agent.
4. **Watch it change.** The agent picks up the comment, publishes a new
   version and replies in the thread. Earlier versions stay one click away.

Pages can do more than display: they can keep shared data, react live while
others view them, and ask you questions you answer in the browser.

### Comment on any web page

The Clax Chrome extension puts the same comment overlay on any page, such as
your dev server, so you can point at what's wrong there and send it to your
agent. `clax init` installs it, or run `/clax:extension` in Claude Code.
Then, once, open `chrome://extensions`, turn on Developer mode, choose
**Load unpacked**, and pick `~/.clax/extension`. It works with Chrome,
Chromium, Brave and Edge on macOS and Linux.

### Share with people nearby

Others on your network can view and comment on your pages, and connect
their own agents, with the rights you give them. Stop Clax (`clax stop`),
serve it on your network with `clax serve --bind 0.0.0.0`, and invite
people from the **People** page. Use a network you trust: traffic on the
LAN is not encrypted. [People](docs/usage.md#people) in the usage guide
covers invitations and rights.

### Handy commands

Your agent does most of this for you, but the `clax` command is there when
you want it. (With only the Claude Code plugin installed, get it with the
`install.sh` line above.)

```sh
clax open <ID>      # open an artifact in your browser
clax list           # list artifacts
clax comments       # open comment threads, newest activity first
clax status         # is Clax running, and which agents are working on what
clax doctor         # check that everything is healthy
clax doctor --agent claude   # check one harness's plugin: claude, codex, pi or grok
clax stop           # stop Clax
```

Your pages and comments live in `~/.clax`. `clax --help` lists every
command.

## Learn more

- [Usage guide](docs/usage.md): every command, configuration, sharing,
  publishing to a hosted Clax, and upgrading.
- Plugin guides: [Claude Code](plugins/claude-code/README.md),
  [Codex](plugins/clax/README.md), [Pi](plugins/pi/README.md),
  [Grok Build](plugins/clax-grok/README.md).
- [The contract](docs/contract.md): every tool's arguments and results, the
  page contract and the security model, for anyone building an integration.
- [Clax on Cloudflare](docs/hosted/writeup.md): the same server as a hosted,
  multi-person service. Empathic's hosted Clax has its own
  [Terms of Service](docs/legal/terms.md) and
  [Privacy Policy](docs/legal/privacy.md); Clax on your own machine sends
  Empathic nothing.
- [Contributing](CONTRIBUTING.md): reporting problems, sending changes,
  and the contributor license agreement. [Developing
  Clax](docs/usage.md#developing-clax) covers building from source, tests
  and checks.

## License

Copyright (C) 2026 Empathic, Inc.

Clax is free software. The local programs, the agent plugins and the browser
extension are licensed under the GNU GPL, version 2 only. The web
application and the hosted service are licensed under the GNU AGPL, version 3
only. Code they share may be used under either. [LICENSE](LICENSE) says which
license covers which part.
