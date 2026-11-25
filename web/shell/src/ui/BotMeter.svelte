<script lang="ts">
  // ClaxBot's week in the gallery's header (spec 2026-10-21-claxbot §11.4),
  // first in the header's links (`GalleryLinks`), from `GET /api/me/bot`'s
  // answer the header's first render already holds, so it takes its place
  // with the other links and arrives after none of them. Where the bar has
  // room, in words ("ClaxBot · 310 of 2,000 credits", marked "your key" when
  // their runs use their own model key: `meterWords`); narrower, a ring
  // filled to the share used, beside that share ("16%"). Its accessible name
  // is the whole sentence at every width. It links to Settings' Your ClaxBot.
  import type { MeBot } from "../bot/api";
  import { credits, meter, meterWords } from "../bot/meter";

  const p: { me: MeBot } = $props();
  const m = $derived(meter(p.me.week));
  const words = $derived(meterWords(p.me));
  const used = $derived(`${Math.round(m.ratio * 100)}%`);
</script>

<a class={m.exhausted ? "bot-meter out" : m.warn ? "bot-meter warn" : "bot-meter"} href="/settings/claxbot" aria-current={location.pathname === "/settings/claxbot" ? "page" : undefined} aria-label={words.label} title={words.title}>
  <span class="long" aria-hidden="true">{words.own ? "ClaxBot · your key · " : "ClaxBot · "}{credits(m.spent)} of {credits(m.total)} credits</span>
  <span class="short" aria-hidden="true"><span class="ring" style:--used={used}></span>{used}</span>
</a>
