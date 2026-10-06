<script lang="ts">
  // One gallery card: the version numeral first, the title (starred when
  // pinned and no Pin button shows it), who published it (a live page: its
  // URL) and when, the
  // markers and the rally chip, and a footer slot; Pin and Delete, once the
  // token is known, sit in the footer's right end. Props are read off `p`
  // rather than destructured, so the gallery's eager bundle needs no prop
  // runtime.
  import type { Snippet } from "svelte";
  import type { Artifact } from "../api";
  import { relativeTime } from "../format";
  import { rally } from "../view/gallery-model";

  const p: { a: Artifact; token: string | null; onPin(): void; onDelete(): void; markers?: Snippet; footer?: Snippet } = $props();
</script>

<div class="card-wrap">
  <a class="card" href={`/a/${p.a.id}`} title={p.a.description ?? undefined}>
    <span class="cb2">
      <span class="v g">v{p.a.current_version}</span>
      <h3>{p.a.title}{#if p.a.pinned && !p.token}<span class="pin" title="Pinned">{" ★"}</span>{/if}</h3>
      <span class="by">{#if p.a.owner_live}<span class="live-dot" role="img" aria-label="session is live" title="Session is live"></span>{/if}{p.a.live ? p.a.live.page_url.replace(/^\w+:\/\//, "") : p.a.owner_harness || "command line"} · {relativeTime(p.a.updated_at)}</span>
    </span>
    {#if p.markers || rally(p.a)}
      <span class="mks">{@render p.markers?.()}{#if rally(p.a)}<span class="chip rally">rally of 10</span>{/if}</span>
    {/if}
    <span class="ft">{@render p.footer?.()}</span>
  </a>
  {#if p.token}
    <div class="card-tools">
      <button type="button" class="ghost" title={p.a.pinned ? "Unpin" : "Pin"} aria-label={p.a.pinned ? `Unpin ${p.a.title}` : `Pin ${p.a.title}`} onclick={p.onPin}>{p.a.pinned ? "★" : "☆"}</button>
      <button type="button" class="ghost" title="Delete" onclick={p.onDelete}>Delete</button>
    </div>
  {/if}
</div>
