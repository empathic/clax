# Asking where the person is paying attention

Status: draft for the owner's review, 2026-10-16; the four proposals settled the same day.

## Problem

An agent asks its person a question in one of two places. Clax shows it as a
question card; the terminal shows it in chat or in the harness's own dialog
(Claude Code's AskUserQuestion). Today the place depends on the mechanism,
not on the person:

- The MCP server instructions say "ask with `ask` rather than in chat"
  (`crates/clax-mcp/src/tools.rs:35-37`), and every plugin's skill prefers
  `ask` (`plugins/claude-code/skills/clax/SKILL.md:354-386`).
- Claude Code's AskUserQuestion hook waits in Clax whenever the person holds
  any Clax stream with the `questions` or `inbox` topic
  (`Hub::holds_surface`, `crates/clax-server/src/stream.rs:1325-1343`), and
  a hidden tab holds one (`web/shell/src/stream.ts:23-27`). So a person
  typing in the terminal with a Clax tab open in the background gets the
  question in Clax, where they aren't looking (open record `fabba150`).
- A question has no thread (`questions` table, migration at
  `crates/clax-core/src/store/migrations.rs:380-401`). A question about a
  comment shows in the sidebar's "Questions for you", away from the
  comment.

## Decided

- **The agent picks the surface.** The goal is the agent's best estimate of
  where the person is paying attention and wants to engage. Clax gives it
  the signals and a suggestion. It decides nothing for the agent.
- **Best effort:** the estimate uses every signal that is available and
  says when it is unsure.
- **One surface asks; the other is notified.** A question is asked on one
  surface only and is never mirrored. The other surface gets a notice that
  a question waits there. Where the mechanism allows it, the notice lets
  the person pull the question over.
- **The narrowest context:** a question shows in the most local place it
  belongs: the thread it arose from, else the artifact, else the inbox.
- **Every agent follows the same rules:** Claude Code, Codex, Grok and Pi.
- **Discussion stays where it started.** This applies to answers as well as
  questions. Work instigated by a Clax thread is answered in that thread.
  Work instigated in chat is answered in chat.

## Design

### 1. The attention estimate

The daemon keeps, for each agent session, what it knows of where that
session's person is engaged:

- **Instigation:** what started the agent's current work.
  - A Clax thread or question answer delivered to it (`working::mark_items`
    already records the artifact and thread; `crates/clax-server/src/working.rs:62`)
    means Clax, at that thread.
  - A prompt the person typed means the terminal.
- **Terminal activity:** when the person last typed to this session.
  - **Claude Code:** the `UserPromptSubmit` hook (`events::prompt`,
    `crates/clax-hooks/src/events.rs:291-299`) records it. Today the hook
    only reads feedback.
  - **Pi:** the plugin's input event records it, if Pi has one. Otherwise
    the plugin infers it from an agent turn that has no Clax delivery
    behind it.
  - **Codex and Grok:** these have no prompt hook (`plugins/clax/hooks/hooks.json`;
    Grok's is a no-op, `crates/clax-cli/src/commands/hook.rs:889-894`). A
    `Stop` or turn start with no Clax delivery since the last one counts as
    a terminal turn.
- **Clax activity:** whether the person is engaged on the session's page.
  - Presence `here` on the artifact (visible tab, input within 5 min;
    `web/shell/src/view/presence-model.ts:9-10`).
  - New: the tab's focus, reported with presence. Today focus is known
    only inside the shell's SharedWorker (`web/shell/src/stream-hub.ts:26-30`).
    Focus is kept for the person's own agents' suggestion alone: other
    viewers' presence never shows it.
  - New: the person's last input on the page.
  - The extension side panel being open.

From these the daemon derives a **suggestion**:
`{surface: "clax" | "terminal", where: {thread_id?, artifact_id?}, confidence: "clear" | "unsure", why}`.

- The most recent engagement wins:
  - a terminal prompt newer than any Clax input on the page suggests the
    terminal;
  - Clax input or focus newer than the last prompt suggests Clax, at the
    instigating thread when there is one.
- **Unsure** when:
  - the newest signals on the two sides are within a minute of each other;
  - neither side has a signal from the last 5 minutes;
  - or the only signal is an inferred terminal turn (Codex, Grok).

  These windows are a starting default, to tune with use.
- **An unsure suggestion leans to Clax:** it names Clax, at the
  instigating thread or else the session's artifact, with `confidence:
  "unsure"`.
- A hidden or unfocused tab never counts as Clax attention. This replaces
  `holds_surface`'s rule and settles record `fabba150`.

**Where agents read it:**
- A new MCP tool `attention`, and a field on every `ask` result.
- The context the Claude Code and Pi plugins add at prompt and delivery
  time.

### 2. Asking in Clax

- **`ask` takes a place:** `thread_id` (a thread on `url_or_id`'s
  artifact), else the artifact, else nothing (the inbox).
  - Migration 36 (the next free one on this branch) adds `questions.thread_id`.
  - The daemon checks that the thread is on the artifact and that the
    person can read it.
- **Shown in the narrowest place:**
  - A question on a thread renders inside that thread's card, beneath its
    last comment, with the same answer controls as the question card. It
    is still counted in the sidebar's questions, the top bar's Inbox and
    `/inbox`, each linking to the thread.
  - A question on an artifact shows as today, in the sidebar's "Questions
    for you".
  - A question on nothing shows in the inbox.
- **The terminal side is told:** the `ask` result carries
  `tell_terminal: "Asked in Clax: <url>"`, and the instructions tell the
  agent to write that line in chat. No plugin can write to the terminal
  except through the agent or a hook's status line.

### 3. Asking in the terminal

- **The notice:** a question asked in the terminal is not copied to Clax.
  Clax shows a notice in the narrowest place (§2): "<agent> is asking you
  in the terminal", with the time and, for a thread, the thread. Clax also
  notifies as for a question (the inbox count, the tab's notification).
  The notice closes when the question is answered or withdrawn.
- **Claude Code's AskUserQuestion:**
  - The hook asks the daemon for the suggestion.
  - When it names the terminal, the hook releases at once, so the
    terminal dialog shows. It posts the notice, and the PostToolUse
    `asked` hook (`crates/clax-hooks/src/ask.rs:207-228`) closes it.
  - When it names Clax (unsure included), the hook asks in Clax as today,
    and its status line is the terminal's notice. The card keeps
    **Answer in the terminal**, the pull toward the terminal (`release_owner`,
    `crates/clax-server/src/routes/questions.rs:735-750`).
- **Chat questions** in any harness, which the agent writes as text: the
  agent posts the notice with `ask` `{notice: true, thread_id?}` (the
  questions' headers only), and cancels it when the person answers in
  chat.
- **Pulling to Clax:** a notice offers **Ask me here**. It sends the agent
  a delivery (piggybacked feedback, `wait_for_feedback`, channel notice)
  saying the person wants the question in Clax, and the agent asks it
  there with `ask`.
  - **The limit:** Claude Code's terminal dialog can't be closed from
    outside. After Ask me here, the dialog stays until the person presses
    Esc, and the notice says so. Esc withdraws the hook's question
    (`question_grace`); the Clax question is the agent's new one.

### 4. Answers and replies stay with their thread

- **The instructions** (MCP server instructions and every plugin's skill)
  change from "prefer `ask`" to:
  1. read the suggestion;
  2. ask on the surface it names, at the place it names, and only there;
  3. notify the other surface: in chat, "Asked in Clax: <url>"; in Clax,
     the notice of §3;
  4. answer Clax-instigated work in its thread (`comments_reply`), and
     chat-instigated work in chat.
- **The suggestion's `why`** is shown to the agent, so it can override
  with its own context (for example, the person just said "I'm on my
  phone").

### 5. Each harness

| Harness | Terminal activity | Native question | Notice in Clax |
|---|---|---|---|
| Claude Code | `UserPromptSubmit` | AskUserQuestion | the hook posts the notice (§3) |
| Pi | input event if offered, else inferred | none (`clax_ask` is Clax's) | n/a |
| Codex | inferred from turns | `request_user_input` (plan mode only) | no; the agent posts the notice |
| Grok | inferred from turns | `ask_user_question` | no; the agent posts the notice |

## Tests

- **Store:**
  - the suggestion for each combination of instigation, prompt time,
    presence, focus and input, including both unsure cases;
  - a thread question refused off its artifact.
- **Server:**
  - a hook question with a newer terminal prompt is released at once and
    leaves a notice;
  - one with newer focused Clax input waits in Clax;
  - **Ask me here** reaches the agent as a delivery;
  - the `asked` hook closes a notice;
  - a hidden tab is not attention.
- **Plugins:** the Claude prompt hook records terminal activity; Pi's turn
  inference; the skills' text in all four plugins.
- **Browser:**
  - a thread question renders in its thread card and is counted in the
    Inbox;
  - a notice shows "asking you in the terminal" and its Ask me here;
  - both at 1280 and 390 wide, light and dark.

## Settled by the owner (2026-10-16)

1. The unsure windows (one minute apart, none in 5 minutes) are a default
   to tune with use.
2. An unsure suggestion leans to Clax.
3. No mirroring: a question is asked on one surface, and the other gets a
   notice.
4. Tab focus goes to the daemon, for the person's own agents.
