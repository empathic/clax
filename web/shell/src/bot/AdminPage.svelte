<svelte:options css="injected" />

<script lang="ts">
  // Settings' screens for admins (spec 2026-10-21-claxbot §5, §11.5), one
  // at a time by `tab`: Roles & ClaxBot access (`people`), Model provider,
  // Web access, Pricing and Usage. Anyone else is told the screen is for
  // admins and fetches nothing more. The owner alone changes roles (other
  // admins see them read-only).
  import { onMount, type Component } from "svelte";
  import type { Role } from "./api";
  import { isAdmin, myRole } from "./me";
  import BotPage from "./BotPage.svelte";

  export type Tab = "people" | "provider" | "web" | "pricing" | "usage";
  let { now, tab = "people" }: { now?: () => Date; tab?: Tab } = $props();
  const TABS: Record<Tab, { title: string; lede: string; load: () => Promise<{ default: unknown }> }> = {
    people: { title: "Roles & ClaxBot access", lede: "Who is an admin, who may use ClaxBot, and with how many credits each week. The owner is always an admin and alone makes others admins.", load: () => import("./AdminPeople.svelte") },
    provider: { title: "Model provider", lede: "The API ClaxBot calls, its key and models, and the caps on a run.", load: () => import("./AdminProvider.svelte") },
    web: { title: "Web access", lede: "Whether ClaxBot may search the web and fetch pages, and how.", load: () => import("./AdminWeb.svelte") },
    pricing: { title: "Pricing", lede: "What a credit costs, the free credits each week, and what each model costs.", load: () => import("./AdminPricing.svelte") },
    usage: { title: "Usage", lede: "What a week cost, by person and by cost line.", load: () => import("./AdminUsage.svelte") },
  };
  // The screen is mounted afresh for another tab, so the first is the one shown.
  // svelte-ignore state_referenced_locally
  const t = TABS[tab];
  let role = $state<Role | null>(null);
  let Body = $state<Component<{ role: Role; now?: () => Date }> | null>(null);
  onMount(() => {
    document.title = `${t.title} · Settings · Clax`;
    void myRole().then(r => {
      role = r;
      if (isAdmin(r)) void t.load().then(m => { Body = m.default as Component<{ role: Role; now?: () => Date }>; }, () => {});
    });
  });
</script>

<BotPage title={t.title} lede={t.lede}>
  {#if role !== null && !isAdmin(role)}
    <p class="note">This screen is for the owner and the admins they name. Your own ClaxBot usage is under <a href="/settings/claxbot">Your ClaxBot</a>.</p>
  {:else if role && Body}
    <Body {role} {now} />
  {:else}
    <p class="note" role="status">Loading…</p>
  {/if}
</BotPage>
