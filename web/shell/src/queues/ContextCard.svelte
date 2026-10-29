<svelte:options css="injected" />

<script lang="ts">
  // What a report's reporter's browser said at capture (spec
  // 2026-10-29-feedback-loop-design §5.1), as a card: the release (its
  // commit a link when `app.repo` names the repository, else text), the
  // feature flags as chips, the browser, the viewport, the page, the
  // capture, the time in the reader's zone and in the reporter's, and the
  // stamp of the server that received it. Every value is the reporter's
  // and shown as text. `zone` and `locale` are the reader's (the browser's
  // when absent).
  import type { ReportContext, ServerStamp } from "../../../sdk/src/types";
  import { browserOf, commitLink, httpUrl, timeIn } from "./model";

  const p: { context?: ReportContext; server?: ServerStamp; zone?: string; locale?: string } = $props();
  const c = $derived(p.context);
  const rel = $derived(c?.release);
  const commit = $derived(commitLink(c));
  const env = $derived(c?.environment);
  const flags = $derived(Object.entries(c?.flags ?? {}));
  const flagText = (v: unknown) => (v === true ? "on" : v === false ? "off" : String(v));
  const theirZone = $derived(env?.time_zone);
  const mine = $derived(c ? timeIn(c.captured_at, p.zone, p.locale) : null);
  const theirs = $derived(c && theirZone ? timeIn(c.captured_at, theirZone, p.locale) : null);
  const pageUrl = $derived(httpUrl(env?.url));
  const MODE = { element: "An element", range: "Selected text", area: "A drawn area", screen: "The screen" } as const;
</script>

<section class="ctx" aria-label="Context">
  <h3>Context</h3>
  {#if !c}
    <p class="none">The report came with no context.</p>
  {:else}
    <dl>
      {#if rel && (rel.version || rel.commit || rel.id || rel.channel)}
        <dt>Release</dt>
        <dd class="rel">
          {#if rel.version}<b>{rel.version}</b>{/if}
          {#if rel.channel}<span class="cchip">{rel.channel}</span>{/if}
          {#if commit}
            {#if commit.href}<a class="commit" href={commit.href} target="_blank" rel="noopener noreferrer" title={`${rel.commit} on ${commit.host}`}>{commit.text}</a>
            {:else}<code class="commit" title={rel.commit}>{commit.text}</code>{/if}
          {/if}
          {#if rel.id}<span class="muted">{rel.id}</span>{/if}
        </dd>
      {/if}
      {#if flags.length}
        <dt>Flags</dt>
        <dd><ul class="flags">{#each flags as [k, v] (k)}<li class="cchip" class:off={v === false}>{k}{v === true ? "" : `: ${flagText(v)}`}</li>{/each}</ul></dd>
      {/if}
      <dt>Captured</dt>
      <dd class="when">
        {#if mine}<span data-zone="reader">{mine}</span>{:else}<span>{c.captured_at}</span>{/if}
        {#if theirs && theirs !== mine}<span class="muted" data-zone="reporter">{theirs} for the reporter ({theirZone})</span>
        {:else if theirZone && !theirs}<span class="muted">reporter's zone: {theirZone}</span>{/if}
      </dd>
      {#if env?.user_agent || env?.platform}
        <dt>Browser</dt>
        <dd title={env.user_agent}>{[browserOf(env.user_agent), env.platform].filter(Boolean).join(" on ")}{#if env.locale}<span class="muted">{` · ${env.locale}`}</span>{/if}</dd>
      {/if}
      {#if env?.viewport}
        <dt>Viewport</dt>
        <dd>{env.viewport.w} × {env.viewport.h}{env.viewport.dpr !== 1 ? ` @${env.viewport.dpr}x` : ""}{#if env.scroll}<span class="muted">{` · scrolled ${env.scroll.x}, ${env.scroll.y}`}</span>{/if}{#if env.color_scheme}<span class="muted">{` · ${env.color_scheme}`}</span>{/if}</dd>
      {/if}
      {#if env?.url || env?.route || env?.title}
        <dt>Page</dt>
        <dd class="where">
          {#if env.title}<span>{env.title}</span>{/if}
          {#if pageUrl}<a href={pageUrl} target="_blank" rel="noopener noreferrer">{env.url}</a>{:else if env.url}<code>{env.url}</code>{/if}
          {#if env.route}<code class="muted">{env.route}</code>{/if}
        </dd>
      {/if}
      {#if c.capture}
        <dt>Capture</dt>
        <dd>{MODE[c.capture.mode] ?? c.capture.mode}, {c.capture.method === "display" ? "from the screen" : "a render of the page"}{#if c.capture.omitted?.length}<span class="muted">{` · ${c.capture.omitted.length} ${c.capture.omitted.length === 1 ? "part" : "parts"} left out`}</span>{/if}</dd>
      {/if}
      {#if p.server}
        <dt>Received</dt>
        <dd>{timeIn(p.server.received_at, p.zone, p.locale) ?? p.server.received_at}<span class="muted">{` · Clax ${p.server.version}${p.server.commit ? ` (${p.server.commit.slice(0, 12)})` : ""}`}</span></dd>
      {/if}
    </dl>
  {/if}
</section>

<style>
  .ctx { min-width: 0; }
  .ctx h3 { margin: 0 0 8px; font-size: 14px; }
  .none { margin: 0; color: var(--muted); font-size: 13px; }
  dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 6px 14px; margin: 0; font-size: 13px; }
  dt { color: var(--muted); }
  dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  .rel, .when, .where { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; }
  .muted { color: var(--muted); }
  .cchip { padding: 0 7px; border: 1px solid var(--border); border-radius: 999px; font-size: 12px; }
  .cchip.off { color: var(--muted); text-decoration: line-through; }
  .flags { display: flex; flex-wrap: wrap; gap: 4px; margin: 0; padding: 0; list-style: none; }
  .commit, code { font: 12.5px var(--mono); }
  .when, .where { flex-direction: column; gap: 2px; }
  @media (max-width: 520px) {
    dl { grid-template-columns: minmax(0, 1fr); gap: 2px; }
    dd { margin-bottom: 6px; }
  }
</style>
