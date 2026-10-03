# Echo: the shell's design, the agent working signal, the version changelog and batch send

This record holds the design decisions behind Echo, the look and interaction
model of the Clax shell, and behind three features built in it: the agent
working signal, the version changelog, and batch send. The main spec
(`2026-09-28-clax-design.md`) states the resulting contract, row D-Echo in
its §2 points here, and §8 to §16 carry the details. This record states what
was decided and why; where the two differ in wording, the main spec is the
contract.

## 1. The model: comment threads

- People and agents interact through comment threads anchored to the page,
  as in pull request review comments or document comments. The vocabulary is
  comment, thread, reply, resolve, addressed and outdated.
- There are no turns. A person comments whenever they like; an agent works
  and publishes when it is ready. Nothing in the shell says whose move it is,
  and there is no status for the artifact as a whole.
- Every screen shows the threads from the viewer's point of view: what is
  new for this viewer, what agents are working on, what is open.
- On top of the threads, Clax adds three layers: a version-tagged history
  line on each thread (`v3 alex commented · v4 Mia replied · claude worked on
  it · v5 claude addressed it · alex resolved`), a working marker, and
  "addressed in vN", the agent's claim that a version handled a thread.
  Addressing never resolves; resolving stays a separate act.
- Several people and several agents may share an artifact. People and agents
  are shown as two groups, never as one person against one agent.

## 2. Echo: the look

### The idea

Clax is a call and its echo: comments go out, versions come back. The Echo
mark is two half-discs facing a brown dot. The left arc is people
(red-orange), the right arc is agents (green), and the dot is the page
between them. The same layout recurs in the top bar's roster and in each
gallery card's roster: people on the left, agents on the right.

### Principles

- The artifact is the star. Nothing Clax draws covers or moves it, except
  pins and the comment-mode outline.
- Two voices with fixed jobs. IBM Plex Sans Condensed 600 sets Clax's
  structure: titles, numerals, labels, buttons and group heads. IBM Plex Mono
  sets everything people and agents write, and all meta.
- Two colours with fixed meanings. Red-orange (`#ED5439`) is people: their
  comments, pins and comment mode. Green (`#457D26`, lighter in dark mode) is
  agents: their notes, working, and primary actions. Brown (`#2F0B04`) is
  ink. Pink (`#F9C8BF`) is an accent only.

### Type and labels

- Plex Sans Condensed is one self-hosted WOFF2 of about 20 KB (SIL OFL 1.1),
  `font-display: swap`, never preloaded, with `font-synthesis: none` and a
  metric-adjusted fallback, so it never blocks first paint and its swap
  barely moves the top bar. At most three font files load: Plex Mono 400 and
  600, and Plex Sans Condensed 600.
- Labels are sentence case. Nothing uses tracked capitals.

### Voice

- Buttons are plain verbs: Comment, Reply, Resolve, Send to claude, Clear.
- Playful words appear only in status lines, hints and empty states.
- Haiku appear in two places: the gallery footer (a new one each visit) and
  the line under an agent's working status (the sidebar strip and the people
  panel). Never in comment mode, never animated.

### Theme

The shell follows the system's colour scheme. A switch in the top bar flips
between light and dark; a flip that lands on the system's own scheme clears
the stored choice, so the shell follows the system again. There is no third
"auto" state.

### Comment mode

The Comment button turns red-orange and a 3px red-orange rule runs under the
top bar. The C keycap shows on the Comment button only.

### Keys

The shell has three keys: C toggles comment mode, `?` opens a sheet listing
the keys, and Esc leaves comment mode or closes a menu or the sheet. No key
acts on a thread; threads, versions and people are reached by pointer or by
Tab. The keys act only while focus is in the shell's own chrome and not in a
text field or a dialog. While focus is in the artifact, keys belong to the
page, and C and `?` stay held while the viewer may still be typing for it:
after focus leaves the shell window, after a prompt or composer the page
raised opens or closes, and after a reload the page's publish caused, until
the viewer presses in the shell or focus lands on one of its controls.
Escape keeps its existing behaviour. Once focus may have reached the shell
from the page, Send, Resolve, Reply, the batch sends, Post in a composer the
page opened, and Allow take only a pointer's click; no key lifts that, only
a press on a shell control (main spec §8, "Consequential actions").

### Motion

The mark's halves meet over 300 ms; a working agent's dot breathes; a 2px
green sweep runs under the top bar while an agent works. Under reduced
motion nothing moves.

### Easter eggs

- **Rally of 10.** At an artifact's tenth version only (not every tenth): a
  muted chip on its gallery card, and once per browser in the top bar's
  summary.
- **The mark.** Clicking the gallery's mark makes its halves meet; a second
  click parts them. In the artifact view the mark is a plain link to the
  gallery.

## 3. Echo: the screens

### Participants

- A person's token is their initials in a half-disc with a red-orange
  outline: underlined for the viewer, a green dot when here, half opacity when
  away.
- An agent's token is a half-disc with a green outline: solid green with a
  breathing dot while working (a still dot under reduced motion), outline
  only when idle.
- An agent is named by its harness: `claude`, `codex`, `pi`. When two agents
  on one artifact share a harness, each name gains a short suffix from its
  handle (`claude 7f3a`). A handle identifies an agent; the harness names it.

### Top bar

The mark, the title over the "published by" line, the roster with a
two-line summary, Comment, Threads with the open count, the version button,
a menu (open raw, copy link) and the theme switch.

- Summary line 1 is the first that applies: `<agent> working on N` (several
  working agents are listed: `claude, codex working on 5`; `<agent>: <message>`
  when the record has a message; `<agent> working` with no named threads);
  `v5 published` with a Reload button beside it; `v5 addressed 3`, or `3 new
  versions · 7 addressed` for a viewer returning after several versions;
  otherwise nothing working.
- Summary line 2 is about the viewer: how many of the worked-on threads are
  theirs and the elapsed time, or `yours, not looked at yet`, or the count of
  open threads, plus idle agents when there is room.
- The roster and its summary open the people panel.
- At phone width the bar holds the mark, the title, the roster (one token per
  side), Comment and the menu (open raw, copy link), so both stay reachable on
  a phone; a Page | Threads switch sits at the foot.

### The version moment

A new version never puts a band over the page. It shows as a green dot on the
version button, the summary line, the Addressed in vN group, and the version
menu. Viewing an older version shows a Latest link beside the version
button. Error notices keep their existing form.

### Version menu

Every version, newest first: who published it and when, the threads it
addressed as numbered chips, what this viewer did about them (`you resolved
it`, `you replied on 9, still open`), and its note. A full sheet at phone
width.

### Thread cards and the sidebar

- People's messages carry a red-orange rule on the left; an agent's a green
  rule on the right, labelled `<agent> · addressed in vN` when its version
  addressed the thread.
- An `outdated` tag marks a thread whose element changed in a later version
  but still exists. Outdated threads stay in Open with the tag. Detached
  keeps its meaning: the anchor is gone.
- Actions: Reply, Resolve, and `Send to <agent> ▾`, whose caret picks the
  agent.
- Groups: Addressed in vN, Open, Detached, Resolved. Addressed in vN holds
  the open threads the viewer is in that the newest version addressed and
  that they had not looked at when the view was decided. Threads addressed in
  earlier versions are in Open, their history line naming the version.
- Pins: red-orange when open; split red-orange and green while an agent works
  on the thread; white with a green ring and a `vN` flag when addressed and
  not looked at; a green ring when selected; dashed while being written.

### People panel

One row per person (the threads they are in, here or away and where they are
looking, the last version they viewed) and per agent (the threads it is
working on and whose, the elapsed time with a haiku, or idle). The viewer's
own name is edited here, with a "Share where I'm looking" switch.

### Gallery

- **Needs your eyes** first, then **Everything else** (pinned first, then by
  the latest version or reply). There is no other grouping.
- An artifact needs this viewer's eyes when a thread they are in was
  addressed after they last looked at it, a version newer than the last one
  they viewed exists, or a thread they are in has a reply from someone else
  they have not seen.
- Each card leads with its version numeral, then the title, the publishing
  agent and time, the markers (`N addressed in vK`, `vK new`, `N new
  replies`, `<agent> working on N`, `N open`, `rally of 10`), and a footer
  with the roster and `seen vK`.
- The gallery footer holds one haiku.

## 4. The agent working signal

- An agent marks an artifact as being worked on, optionally naming the
  threads it acts on, and optionally with a short message.
- It is set automatically when comment feedback reaches the agent, and
  explicitly with a `working` tool.
- It clears when the agent replies to or resolves the last thread it names,
  publishes the artifact, ends its turn, or its session ends, and it lapses
  two minutes after its last renewal.
- Tool calls renew it. A `PostToolUse` hook does so at most once a minute
  per session, through a shell check of a stamp file's age, so most tool
  calls start no process.
- It shows in the top bar's roster and summary, on gallery cards, on thread
  cards and pins, and to the page through a Clax extension of the `comments`
  capability (`working()`, `onWorking(fn)`), read-only and without consent,
  under either declaration form.

## 5. The version changelog

- Each version may carry a short note from the agent and the set of threads
  it addressed. There are no markup or visual diffs.
- Threads the publishing session was working on are linked to the new
  version automatically; `addresses` on a publish names more; an agent
  resolve links the thread to the current version when no version lists it
  yet.
- Linking never resolves. A person resolves, from any card.
- What is new is decided per viewer: the last version each viewer viewed,
  and when they last looked at each thread.

## 6. Batch send

- Each open thread card has a checkbox; Shift-click ticks a range.
- While any is ticked, a selection bar sits at the top of the sidebar:
  `N selected · sent together`, Clear, `Send N to <agent> ▾`, and an optional
  one-line note.
- `Send N unsent to <agent>` sits at the top of the sidebar whenever open
  threads have not been sent.
- The agent receives the batch as one delivery, led by the note, on every
  tier. Each thread keeps its own sent state, working marker and changelog
  link.
- A batch holds at most 20 threads. The page capability has no batch verb:
  a page sends its own threads one call at a time.

## 7. Where a send goes

- A send names one agent (`to`). It reaches only that agent, and that agent
  becomes the thread's target: later comments on the thread go to it while
  its session is live.
- The default target is the agent this viewer last sent to on the artifact,
  if it is live; otherwise the most recently active live agent receiving the
  artifact's comments (its owner session or a watcher).
- With no live agent, the shell sends without `to`. The comment is stored
  without a target and goes to the next session that publishes a version of
  the artifact or watches it, as comments with no live target always have.
- A send without `to` (the page's `sendToClaude`, `@agent` on a new thread)
  goes to the artifact's owner session and every watcher, and the thread has
  no target.

## 8. Decisions

- **Q1. Gallery cards have no thumbnails.** Cards lead with the version
  numeral, the title and the markers. Rendering page captures would cost
  time to usable.
- **Q2. The theme switch returns to the system** when a flip lands on the
  system's scheme; there is no "auto" state.
- **Q3. @mentions** are `@` and a viewer's whole display name, in any case,
  ending at whitespace, punctuation or the end of the text. A two-word name
  needs both words (`@Mia Kovač`). There is no autocomplete. Comments written
  before authors were recorded stay unattributed.
- **Q4. Looking at a thread** is its card being at least half visible for one
  second, or selecting it (by card or pin). The mark is written at
  once, so the gallery clears. The thread stays in Addressed in vN until the
  view is decided again: on the next load, or when a new version arrives. The
  group never empties itself while the viewer reads it.
- **Q5. Presence** is built now, as a light layer kept in memory. Here means
  the tab is visible; away means hidden, or idle for 5 minutes. The location
  is the anchor of the selected thread or of the comment being written; Clax
  never tracks scroll position. A "Share where I'm looking" switch in the
  people panel is on by default and stored per browser. The viewer's name
  moves into the people panel.
- **Q6. Keys** work only while focus is in Clax's chrome. While the page has
  focus, keys are the page's. Escape keeps its behaviour.
- **Q7. Reading state.** The last version each person viewed is public: it
  shows in the people panel, and the viewer's own in the gallery footer
  (`seen vK`). Which threads a person has looked at is private to them.
- **Q8. Agents** are named by harness, with a handle suffix when two share
  one. There is no "publishing" state: a publish takes milliseconds and has
  no reliable start signal.
- **Q9. Easter eggs.** Rally of 10 fires at v10 only: a gallery chip, and
  once per browser in the top bar. The mark's halves meet only on the
  gallery's mark.
- **Q10. Resolving.** Any viewer may resolve a thread, and an agent may
  resolve threads sent to it. The history records who.
- **Q11. The returning viewer's summary** goes in the top bar's summary line
  (`3 new versions · 7 addressed`), with the dot on the version button. No
  band over the page.
- **Q12. Version bands.** The "v5 published, reload" and "viewing an older
  version" bands move into the top bar: `v5 published` in the summary line
  with a Reload button, and a Latest link beside the version button. Error
  notices stay as they are.
