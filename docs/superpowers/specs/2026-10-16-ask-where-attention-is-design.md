# Asking where the person is paying attention

Status: draft for the owner's review, 2026-10-16.

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
- **Both surfaces say what is happening.** A question asked in the terminal
  shows in Clax as being asked there, and the reverse. Where the mechanism
  allows it, the person can pull the question to the other surface.
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

- **Claude Code's AskUserQuestion:**
  - The hook asks the daemon for the suggestion.
  - When it says the terminal, or is unsure with the terminal newest, the
    hook releases at once, so the terminal dialog shows.
  - It then posts the questions to Clax as a **notice**: a read-only card,
    "Claude is asking in the terminal". The card shows the questions and
    follows their state. The PostToolUse `asked` hook
    (`crates/clax-hooks/src/ask.rs:207-228`) closes it as "Answered in the
    terminal".
  - When the suggestion says Clax, the hook waits in Clax as today. The
    card keeps **Answer in the terminal**, which is the pull toward the
    terminal (`release_owner`, `crates/clax-server/src/routes/questions.rs:735-750`).
- **Chat questions** in any harness, written by the agent as text: the
  agent may post the same questions as a notice with `ask`
  `{notice: true}`. It shows in the narrowest place, as in §2, marked
  "asked in the terminal", and closes when the agent cancels it or the
  person answers.
- **Pulling to Clax:** on a notice, **Answer here** turns it into an open
  question in Clax. The agent learns of that at its next delivery (piggy-
  backed feedback, `wait_for_feedback`, channel notice). It then stops
  waiting on the terminal and takes the Clax answer.
  - **The limit:** Claude Code's terminal dialog can't be closed from
    outside. A pulled AskUserQuestion's dialog stays until the person
    dismisses it. The notice says so ("Answer here, then press Esc in the
    terminal"). Pressing Esc withdraws the hook question
    (`question_grace`), so the pulled question must be a new row, not the
    hook's.

### 4. Answers and replies stay with their thread

- **The instructions** (MCP server instructions and every plugin's skill)
  change from "prefer `ask`" to:
  1. read the suggestion;
  2. ask on the surface it names, at the place it names;
  3. say on the other surface what you did;
  4. answer Clax-instigated work in its thread (`comments_reply`), and
     chat-instigated work in chat.
- **The suggestion's `why`** is shown to the agent, so it can override
  with its own context (for example, the person just said "I'm on my
  phone").

### 5. Each harness

| Harness | Terminal activity | Native question | Mirrored |
|---|---|---|---|
| Claude Code | `UserPromptSubmit` | AskUserQuestion | yes (hook, §3) |
| Pi | input event if offered, else inferred | none (`clax_ask` is Clax's) | n/a |
| Codex | inferred from turns | `request_user_input` (plan mode only) | no; notice by the agent |
| Grok | inferred from turns | `ask_user_question` | no; notice by the agent |

## Tests

- **Store:**
  - the suggestion for each combination of instigation, prompt time,
    presence, focus and input, including both unsure cases;
  - a thread question refused off its artifact.
- **Server:**
  - a hook question with a newer terminal prompt is released at once and
    leaves a notice;
  - one with newer focused Clax input waits in Clax;
  - **Answer here** opens a new question and closes the notice;
  - the `asked` hook closes a notice;
  - a hidden tab is not attention.
- **Plugins:** the Claude prompt hook records terminal activity; Pi's turn
  inference; the skills' text in all four plugins.
- **Browser:**
  - a thread question renders in its thread card and is counted in the
    Inbox;
  - a notice shows "asking in the terminal" and its Answer here;
  - both at 1280 and 390 wide, light and dark.

## Proposals for the owner

1. The unsure window (signals within a minute of each other, or none in 5
   minutes).
2. Whether an unsure suggestion should lean to the terminal (the agent's
   own surface) or to Clax.
3. Whether a notice for chat questions is the agent's choice (as above) or
   always posted when the person has Clax open on the page.
4. Whether to send tab focus to the daemon. It is new presence data that
   other people in the room never see; only the person's own agents see
   it.
