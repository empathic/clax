<svelte:options css="injected" />

<script lang="ts">
  // One report (`/queues/<qid>/<tid>`, spec 2026-10-29-feedback-loop-design
  // §8): its clip (fitted to the page, or at its own size or twice it, with
  // what the render left out outlined), its note, what it is anchored on,
  // its context (`ContextCard`), the filing's diagnostics (capture 0) and
  // each reopen's with its comment (`Diagnostics`, collapsed),
  // the reporter as the integrator names them, the conversation with a
  // reply box, its fixes (`FixHistory`), and what a triage person may do in
  // its state: propose a fix (open, claimed or waiting for a verdict),
  // resolve it with a reason (the same), say for the reporter that the fix
  // works (waiting for a verdict), or that it is still broken (waiting for
  // a verdict, verified or resolved), and end a claim whoever holds it
  // (claimed: "Release claim", after asking, with a note for triage alone);
  // claiming is an agent's, never offered here. Every text the reporter's
  // browser sent is shown as text, and a note for triage alone says so.
  // Each action's answer is the report as it now is; a refusal says why
  // until the next action. The report is fetched again whenever the
  // queue's topic says it changed and when the stream comes back (one
  // fetch at a time, one more after it when another change came
  // meanwhile); an answer older than the latest is dropped. A refetch
  // keeps the page where it is, the drafts typed and the form open while
  // its state still allows it, and says politely what someone else
  // changed. Its back link returns to the queue view on the tab it came
  // from.
  import { onMount } from "svelte";
  import type { EventStream } from "../stream";
  import { relativeTime } from "../format";
  import * as api from "./api";
  import ContextCard from "./ContextCard.svelte";
  import Diagnostics from "./Diagnostics.svelte";
  import FixHistory from "./FixHistory.svelte";
  import { REASON_LABEL, STATE_LABEL, TABS, anchorText, httpUrl, noteOf, queueHref, queueTopic, refusal, reporterName } from "./model";

  const p: { qid: string; tid: string; queue?: api.Queue | null; nav: (e: MouseEvent) => void; stream?: EventStream; zone?: string; locale?: string } = $props();
  let report = $state<api.Report | null>(null);
  let loadErr = $state<string | null>(null);
  let err = $state<string | null>(null);
  let status = $state("");
  let busy = $state(false);
  // The action form open, and each form's fields.
  let form = $state<"fix" | "resolve" | "verify" | "reopen" | "release" | null>(null);
  let fix = $state({ summary: "", url: "", commit: "", verify_url: "" });
  let reason = $state<api.ResolveReason | "">("");
  let verifyNote = $state("");
  let reopenBody = $state("");
  let releaseNote = $state("");
  let replyText = $state("");
  // What someone else changed, said politely.
  let notice = $state("");
  // The clip: fitted, or at 1× or 2× its size on the reporter's screen.
  let zoom = $state<"fit" | 1 | 2>("fit");
  let natural = $state<{ w: number; h: number } | null>(null);

  const r = $derived(report);
  const st = $derived(r?.state);
  const canFix = $derived(st === "open" || st === "claimed" || st === "fix_proposed");
  const canVerify = $derived(st === "fix_proposed");
  const canReopen = $derived(st === "fix_proposed" || st === "verified" || st === "resolved");
  const canRelease = $derived(st === "claimed" && !!r?.claim);
  // The form open, while the report's state still allows it: one it no
  // longer allows closes (its fields keep what was typed).
  const shown = $derived(form && ({ fix: canFix, resolve: canFix, verify: canVerify, reopen: canReopen, release: canRelease } as const)[form] ? form : null);
  $effect(() => { if (form && !shown) form = null; });
  const conversation = $derived(r?.comments.slice(1) ?? []);
  const omitted = $derived(r?.context?.capture?.omitted ?? []);
  const dpr = $derived(r?.context?.environment?.viewport?.dpr || 1);
  const pageUrl = $derived(httpUrl(r?.artifact.url));
  const profile = $derived(r?.reporter_profile);
  const github = $derived(profile?.github && /^[A-Za-z0-9-]+$/.test(profile.github) ? `https://github.com/${profile.github}` : null);
  const width = $derived(natural ? `${(natural.w / dpr) * (zoom === "fit" ? 1 : zoom)}px` : null);
  const pct = (v: number, of: number) => `${(v / of) * 100}%`;
  // The queue view, on the tab the report was opened from (`?state=`).
  const tab = TABS.find(t => t.key !== "active" && t.key === new URLSearchParams(location.search).get("state"))?.key;
  // svelte-ignore state_referenced_locally
  const back = `${queueHref(p.qid)}${tab ? `?state=${tab}` : ""}`;

  /** What changed between `was` and `now`, as a sentence; "" for nothing said. */
  function changed(was: api.Report, now: api.Report): string {
    if (now.state !== was.state) return now.state === "claimed" && now.claim ? `Now claimed by ${now.claim.holder_name}.` : `Now ${(STATE_LABEL[now.state] ?? now.state).toLowerCase()}.`;
    const added = now.comments.filter(c => !was.comments.some(w => w.id === c.id));
    if (added.length === 1) return `New reply from ${added[0].author_name}.`;
    if (added.length) return `${added.length} new replies.`;
    return "";
  }

  /** Says `text` in the live region, emptied first so a repeat is said again. */
  function say(text: string) {
    notice = "";
    queueMicrotask(() => { notice = text; });
  }
  // Each fetch's turn: a load answered after a later load or action is dropped.
  let turn = 0;
  async function load() {
    const mine = ++turn;
    try {
      const got = (await api.getReport(p.qid, p.tid)).report;
      if (mine !== turn) return;
      const was = report;
      report = got;
      // While this person's own action is out, a change may be theirs:
      // nothing is said of it (the action's answer says what it did).
      if (was && !busy) { const said = changed(was, got); if (said) say(said); }
      loadErr = null;
      document.title = `${(noteOf(got).split("\n")[0] || "Report").slice(0, 60)} · ${p.queue?.name ?? "Queues"} · Clax`;
    } catch (x) {
      if (mine === turn) loadErr = `Could not load the report: ${refusal(x)}`;
    }
  }
  async function act<T extends { report: api.Report }>(done: string | ((answer: T) => string), fn: () => Promise<T>): Promise<boolean> {
    if (busy) return false;
    busy = true; err = null; status = "";
    try {
      const answer = await fn();
      turn++;
      report = answer.report;
      status = typeof done === "string" ? done : done(answer);
      form = null;
      return true;
    } catch (x) {
      err = refusal(x);
      return false;
    } finally { busy = false; }
  }
  const open = (f: typeof form) => { form = shown === f ? null : f; err = null; status = ""; };
  // One refetch at a time; a change heard meanwhile fetches once more after it.
  let fetching = false;
  let again = false;
  async function refresh() {
    if (fetching) { again = true; return; }
    fetching = true;
    try { do { again = false; await load(); } while (again); } finally { fetching = false; }
  }

  async function proposeFix(e: Event) {
    e.preventDefault();
    const summary = fix.summary.trim();
    if (!summary) { err = "Say what the fix changes."; return; }
    const f: api.FixFields = { summary };
    for (const k of ["url", "commit", "verify_url"] as const) { const v = fix[k].trim(); if (v) f[k] = v; }
    if (await act("Proposed the fix: the reporter can now check it.", () => api.proposeFix(p.qid, p.tid, f))) fix = { summary: "", url: "", commit: "", verify_url: "" };
  }
  async function resolve(e: Event) {
    e.preventDefault();
    if (!reason) { err = "Choose why the report is resolved."; return; }
    const why = reason;
    if (await act(`Resolved: ${REASON_LABEL[why].toLowerCase()}.`, () => api.resolve(p.qid, p.tid, why))) reason = "";
  }
  async function verify(e: Event) {
    e.preventDefault();
    if (await act("Marked verified for the reporter.", () => api.verify(p.qid, p.tid, verifyNote.trim() || undefined))) verifyNote = "";
  }
  async function reopen(e: Event) {
    e.preventDefault();
    const body = reopenBody.trim();
    if (!body) { err = "Say what is still broken."; return; }
    if (await act("Reopened for the reporter.", () => api.reopen(p.qid, p.tid, body))) reopenBody = "";
  }
  async function release(e: Event) {
    e.preventDefault();
    const note = releaseNote.trim();
    const said = (a: { released?: boolean }) => a.released === false ? "The claim had already ended: the report is open." : "Released the claim: the report is open again.";
    if (await act(said, () => api.releaseClaim(p.qid, p.tid, note || undefined))) releaseNote = "";
  }
  async function reply(e: Event) {
    e.preventDefault();
    const body = replyText.trim();
    if (!body) return;
    if (await act("Replied.", () => api.reply(p.qid, p.tid, body))) replyText = "";
  }

  onMount(() => {
    void refresh();
    // The topic's first `ready` is its going live as the view opens, which
    // the fetch above covers; a later one is the stream coming back.
    let opened = false;
    return p.stream?.watch([queueTopic(p.qid)], e => {
      if (e.type === "ready" && !opened) { opened = true; return; }
      if (e.type === "ready" || e.type === "resync" || (e.type === "report" && e.report_id === p.tid)) void refresh();
    });
  });
</script>

<article class="rview" aria-labelledby="rv-h">
  <p class="back"><a href={back} onclick={p.nav}>{p.queue?.name ?? "Queue"}</a></p>
  {#if loadErr}<p class="err" role="alert">{loadErr}</p>{/if}
  {#if r}
    <header class="rh">
      <h2 id="rv-h">{noteOf(r).split("\n")[0] || "Report"}</h2>
      <span class={["state", r.state]}>{STATE_LABEL[r.state] ?? r.state}</span>
      {#if r.resolve_reason && (r.state === "resolved" || r.state === "verified")}<span class="muted">{REASON_LABEL[r.resolve_reason] ?? r.resolve_reason}</span>{/if}
      {#if r.reopen_count}<span class="muted">reopened {r.reopen_count}×</span>{/if}
    </header>
    <p class="by">
      Filed by <b>{reporterName(r)}</b>
      {#if profile?.email}<span class="muted">{profile.email}</span>{/if}
      {#if github}<a href={github} target="_blank" rel="noopener noreferrer">@{profile?.github}</a>{/if}
      · <time datetime={r.created_at} title={r.created_at}>{relativeTime(r.created_at)}</time>
      {#if r.claim && r.state === "claimed"}· <span class="claim">claimed by {r.claim.holder_name} until <time datetime={r.claim.expires_at}>{new Date(r.claim.expires_at).toLocaleTimeString(p.locale, { timeStyle: "short", timeZone: p.zone })}</time></span>{/if}
    </p>

    <div class="cols">
      <div class="col">
        {#if r.has_clip}
          <figure class="clip">
            <div class="zoom" role="group" aria-label="Clip size">
              {#each [["fit", "Fit"], [1, "1×"], [2, "2×"]] as const as [z, label] (z)}
                <button type="button" class="ghost" aria-pressed={zoom === z} onclick={() => { zoom = z; }}>{label}</button>
              {/each}
            </div>
            <div class="clipbox" class:fit={zoom === "fit"}>
              <div class="clipframe" style:width={width}>
                <img src={api.clipUrl(p.qid, p.tid)} alt="The reporter's clip" onload={e => { const i = e.currentTarget as HTMLImageElement; natural = { w: i.naturalWidth, h: i.naturalHeight }; }} />
                {#if natural}
                  {#each omitted as o, i (i)}
                    <span class="omit" title={`Left out of the render: ${o.kind}`} style:left={pct(o.rect.x, natural.w)} style:top={pct(o.rect.y, natural.h)} style:width={pct(o.rect.w, natural.w)} style:height={pct(o.rect.h, natural.h)}><span>{o.kind}</span></span>
                  {/each}
                {/if}
              </div>
            </div>
            {#if omitted.length}<figcaption class="muted">Outlined: what the render left out ({omitted.map(o => o.kind).join(", ")}).</figcaption>{/if}
          </figure>
        {/if}
        <section class="note" aria-label="Note">
          <p class="body">{noteOf(r)}</p>
          {#if anchorText(r.anchor)}<p class="muted anchor">On {anchorText(r.anchor)}</p>{/if}
          <p class="muted where">{r.artifact.title}{#if pageUrl}{" · "}<a href={pageUrl} target="_blank" rel="noopener noreferrer">{r.artifact.url}</a>{/if}</p>
        </section>
        {#if r.filing_diagnostics_lines}
          <Diagnostics count={r.filing_diagnostics_lines} label="Diagnostics at filing" load={() => api.diagnostics(p.qid, p.tid, 0).then(b => b.lines)} />
        {/if}

        <section class="convo" aria-label="Conversation">
          <h3>Conversation</h3>
          {#if conversation.length}
            <ol>
              {#each conversation as c (c.id)}
                <li class={["cm", c.author_kind]} data-id={c.id}>
                  <header><b>{c.author_name}</b> <span class="muted">{c.author_kind === "reporter" ? "reporter" : c.author_kind === "agent" ? "agent" : ""}</span>{#if c.triage_only}<span class="only">Triage only</span>{/if} <time class="muted" datetime={c.created_at} title={c.created_at}>{relativeTime(c.created_at)}</time></header>
                  <p class="body">{c.body}</p>
                  {#if c.has_clip}<img class="cclip" src={api.commentClipUrl(p.qid, p.tid, c.id)} alt={`The clip ${c.author_name} sent with this`} loading="lazy" />{/if}
                  {#if c.diagnostics_lines && c.capture !== undefined}
                    {@const n = c.capture}
                    <Diagnostics count={c.diagnostics_lines} label={`Diagnostics of reopen ${n}`} load={() => api.diagnostics(p.qid, p.tid, n).then(b => b.lines)} />
                  {/if}
                </li>
              {/each}
            </ol>
          {:else}
            <p class="muted">No replies yet.</p>
          {/if}
          <form class="reply" onsubmit={reply}>
            <label for="rv-reply">Reply</label>
            <textarea id="rv-reply" rows="3" placeholder="The reporter sees your reply." bind:value={replyText}></textarea>
            <button type="submit" class="primary" disabled={busy || !replyText.trim()}>Reply</button>
          </form>
        </section>
      </div>

      <aside class="col side">
        <section class="acts" aria-label="Actions">
          <h3>Actions</h3>
          <p class="stat" role="status">{status}</p>
          <p class="sr" aria-live="polite">{notice}</p>
          {#if err}<p class="err" role="alert">{err}</p>{/if}
          <div class="btns">
            {#if canFix}<button type="button" aria-expanded={shown === "fix"} onclick={() => open("fix")}>Propose fix</button>{/if}
            {#if canFix}<button type="button" aria-expanded={shown === "resolve"} onclick={() => open("resolve")}>Resolve</button>{/if}
            {#if canVerify}<button type="button" aria-expanded={shown === "verify"} onclick={() => open("verify")}>Verify for the reporter</button>{/if}
            {#if canReopen}<button type="button" aria-expanded={shown === "reopen"} onclick={() => open("reopen")}>Reopen for the reporter</button>{/if}
            {#if canRelease}<button type="button" aria-expanded={shown === "release"} onclick={() => open("release")}>Release claim</button>{/if}
          </div>
          {#if shown === "fix"}
            <form class="af fix" onsubmit={proposeFix}>
              <label>Summary <textarea name="summary" rows="4" maxlength="4096" aria-required="true" placeholder="What the fix changes; the reporter reads it." bind:value={fix.summary}></textarea></label>
              <label>Pull request or change <input name="url" type="url" placeholder="https://github.com/…/pull/123" bind:value={fix.url} /></label>
              <label>Commit <input name="commit" maxlength="256" bind:value={fix.commit} /></label>
              <label>Where the reporter checks it <input name="verify_url" type="url" placeholder="https://…" bind:value={fix.verify_url} /></label>
              <span class="row"><button type="submit" class="primary" disabled={busy}>Propose fix</button><button type="button" class="ghost" onclick={() => { form = null; }}>Cancel</button></span>
            </form>
          {:else if shown === "resolve"}
            <form class="af" onsubmit={resolve}>
              <label>Reason
                <select name="reason" bind:value={reason}>
                  <option value="" disabled>Choose a reason</option>
                  {#each Object.entries(REASON_LABEL) as [k, label] (k)}<option value={k}>{label}</option>{/each}
                </select>
              </label>
              <span class="row"><button type="submit" class="primary" disabled={busy}>Resolve</button><button type="button" class="ghost" onclick={() => { form = null; }}>Cancel</button></span>
            </form>
          {:else if shown === "verify"}
            <form class="af" onsubmit={verify}>
              <p class="muted">Says the fix works, on the reporter's behalf; it is recorded as yours.</p>
              <label>Note <textarea name="note" rows="2" placeholder="Optional" bind:value={verifyNote}></textarea></label>
              <span class="row"><button type="submit" class="primary" disabled={busy}>Mark verified</button><button type="button" class="ghost" onclick={() => { form = null; }}>Cancel</button></span>
            </form>
          {:else if shown === "reopen"}
            <form class="af" onsubmit={reopen}>
              <p class="muted">Says it is still broken, on the reporter's behalf; it is recorded as yours.</p>
              <label>What is still broken <textarea name="body" rows="3" aria-required="true" bind:value={reopenBody}></textarea></label>
              <span class="row"><button type="submit" class="primary" disabled={busy}>Reopen</button><button type="button" class="ghost" onclick={() => { form = null; }}>Cancel</button></span>
            </form>
          {:else if shown === "release" && r.claim}
            <form class="af" onsubmit={release}>
              <p class="muted">Ends {r.claim.holder_name}'s claim now: its next renew or fix is refused, and it is told. The report is open for anyone to take.</p>
              <label>Note <textarea name="note" rows="2" placeholder="Optional: why. Triage and the agent see it, the reporter does not." bind:value={releaseNote}></textarea></label>
              <span class="row"><button type="submit" class="primary" disabled={busy}>Release claim</button><button type="button" class="ghost" onclick={() => { form = null; }}>Cancel</button></span>
            </form>
          {/if}
        </section>
        <ContextCard context={r.context} server={r.server} zone={p.zone} locale={p.locale} />
        <FixHistory fixes={r.fixes ?? []} />
      </aside>
    </div>
  {:else if !loadErr}
    <p class="muted" role="status">Loading…</p>
  {/if}
</article>

<style>
  .rview { max-width: 1120px; margin: 0 auto; min-width: 0; }
  .back { margin: 0 0 6px; font-size: 13px; }
  .back a::before { content: "← "; }
  .rh { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; }
  .rh h2 { margin: 0; font-size: 20px; line-height: 1.25; overflow-wrap: anywhere; min-width: 0; }
  .by { margin: 6px 0 14px; font-size: 13px; display: flex; flex-wrap: wrap; gap: 2px 8px; align-items: baseline; overflow-wrap: anywhere; }
  .muted { color: var(--muted); font-size: 13px; }
  .err { color: var(--danger); font-size: 13px; margin: 6px 0; }
  .stat { margin: 0; min-height: 1.4em; color: var(--muted); font-size: 13px; }
  .state { padding: 1px 8px; border-radius: 999px; font-size: 12px; white-space: nowrap; border: 1px solid var(--tag-gray-bd); background: var(--tag-gray-bg); color: var(--tag-gray-fg); }
  .state.open { border-color: var(--tag-blue-bd); background: var(--tag-blue-bg); color: var(--tag-blue-fg); }
  .state.claimed { border-color: var(--tag-purple-bd); background: var(--tag-purple-bg); color: var(--tag-purple-fg); }
  .state.fix_proposed { border-color: var(--tag-orange-bd); background: var(--tag-orange-bg); color: var(--tag-orange-fg); }
  .state.verified { border-color: var(--tag-green-bd); background: var(--tag-green-bg); color: var(--tag-green-fg); }
  .cols { display: grid; grid-template-columns: minmax(0, 1fr) minmax(260px, 340px); gap: 20px; align-items: start; }
  .col { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
  .side > :global(section) { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px 14px; }
  h3 { margin: 0 0 8px; font-size: 14px; }
  .clip { margin: 0; min-width: 0; }
  .zoom { display: flex; gap: 2px; margin-bottom: 6px; }
  .zoom button { min-height: 28px; padding: 2px 10px; font-size: 12.5px; }
  .zoom button[aria-pressed="true"] { background: var(--hover); color: var(--fg); font-weight: 600; }
  .clipbox { max-height: 70vh; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); }
  .clipframe { position: relative; display: block; }
  .clipbox.fit .clipframe { max-width: 100%; }
  .clipframe img { display: block; width: 100%; height: auto; }
  .omit { position: absolute; box-sizing: border-box; border: 2px dashed var(--danger); background: color-mix(in srgb, var(--danger) 8%, transparent); pointer-events: auto; }
  .omit span { position: absolute; top: 0; left: 0; padding: 0 4px; font-size: 11px; background: var(--danger); color: var(--on-accent); }
  .clip figcaption { margin-top: 4px; font-size: 12.5px; }
  .note .body, .cm .body { margin: 0; font-size: 14px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .note p { margin: 4px 0 0; overflow-wrap: anywhere; }
  .convo ol { list-style: none; margin: 0 0 12px; padding: 0; display: flex; flex-direction: column; gap: 10px; }
  .cm { padding: 8px 12px; border-left: 3px solid var(--border); min-width: 0; }
  .cm.reporter { border-left-color: var(--accent); }
  .cm.agent { border-left-color: var(--agent); }
  .cm header { font-size: 13px; margin-bottom: 2px; }
  .only { margin-left: 6px; padding: 0 6px; border: 1px solid var(--border); border-radius: 999px; color: var(--muted); font-size: 11.5px; }
  .sr { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; border: 0; }
  .cclip { display: block; max-width: 100%; max-height: 240px; margin-top: 6px; border: 1px solid var(--border); border-radius: var(--radius-xs); }
  .reply { display: grid; gap: 6px; }
  .reply label { font-size: 13px; font-weight: 600; }
  .reply button { justify-self: start; }
  textarea, input, select { width: 100%; box-sizing: border-box; }
  .btns { display: flex; flex-wrap: wrap; gap: 6px; }
  .af { display: grid; gap: 8px; margin-top: 10px; }
  .af label { display: grid; gap: 3px; font-size: 12.5px; color: var(--muted); }
  .af p { margin: 0; }
  .row { display: flex; flex-wrap: wrap; gap: 6px; }
  @media (max-width: 860px) { .cols { grid-template-columns: minmax(0, 1fr); } }
  @media (pointer: coarse) { .zoom button { min-height: 44px; } }
</style>
