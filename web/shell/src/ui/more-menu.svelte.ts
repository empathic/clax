// The top bar's more menu, roster and working summary, the Clone dialog's
// opener, and its Fork, Get in Clax, Share and Publish buttons, are chunks of their own,
// out of the artifact entry: the entry asks for their code (and their
// styles) as it starts, beside the view's data (the page's inline
// `clax-preload` script has them downloading with the HTML), and the bar
// shows its controls once the chunks are in (`loading`) or failed, each
// failed one left out. Publish, the local owner's shell's alone, is asked
// at once where this browser holds the owner's proof (as the inline script
// preloads it), else once the token says it is that shell; then it comes
// late, in its slot.
import type { Component } from "svelte";
import type WhoT from "./Who.svelte";
import { getToken } from "../api";
import { ownerProof } from "../owner-proof";
import { botKnown, runsAsked } from "../view/deciders.svelte";
import type { ArtifactController } from "../view/artifact-controller";

/** What the more menu's Publish item says and opens. */
export type PublishOpener = { label: string; open(): void };
/** The more menu's Usage item: this week's ClaxBot credits, linking to the
 * page that details them. `text` is shown, `label` is its accessible name
 * and `title` its hover text. */
export type UsageItem = { href: string; text: string; label: string; title: string; warn: boolean; out: boolean };
/** A hosted Clax's Terms of Service and Privacy Policy, which the more menu links. */
export type LegalLinks = { terms_url: string; privacy_url: string };
type More = Component<{ rawHref: string | null; canCopy: boolean; onCopy(): void; onVersions?(): void; pageHref?: string | null; getHref?: string | null; onFork?(): void; onClone?: (() => void) | null; onShare?: (() => void) | null; onPublish?: PublishOpener | null; onPermissions?: (() => void) | null; onAnalytics?: (() => void) | null; onPeople?: (() => void) | null; onBot?: PublishOpener | null; usage?: UsageItem | null; legal?: LegalLinks | null }>;
type CloneT = Component<{ id: string }>;
type GetT = Component<{ id: string; ghost?: boolean }>;
type ForkT = Component<{ shown: number; disabled: boolean; onFork(): void }>;
type ShareT = Component<{ id: string; title: string; onComments?(on: boolean): void }>;
type PermissionsT = Component<{ ctl: ArtifactController }>;
type PublishT = Component<{ id: string; title: string; latest: number }>;

let loaded: More | null = $state(null);
let whoLoaded: typeof WhoT | null = $state(null);
let cloneLoaded: CloneT | null = $state(null);
let forkLoaded: ForkT | null = $state(null);
let getLoaded: GetT | null = $state(null);
let cloneOpener: (() => void) | null = $state(null);
let shareLoaded: ShareT | null = $state(null);
let shareOpener: (() => void) | null = $state(null);
let publishLoaded: PublishT | null = $state(null);
let publishOpener: PublishOpener | null = $state(null);
let permissionsLoaded: PermissionsT | null = $state(null);
let botLoaded: Component<{ ctl: ArtifactController }> | null = $state(null);
let permissionsOpener: PermissionsOpener | null = $state(null);
let legalLinks: LegalLinks | null = $state(null);
let usageItem: UsageItem | null = $state(null);
let botOpener: PublishOpener | null = $state(null);
let pending = $state(0);

/** The more menu's component once `loadMoreMenu` has loaded it, else null. */
export const moreMenu = { get current(): More | null { return loaded; } };
/** The roster and summary component once `loadMoreMenu` has loaded it, else null. */
export const who = { get current(): typeof WhoT | null { return whoLoaded; } };
/** The Fork button's component once `loadMoreMenu` has loaded it, else null. */
export const forkButton = { get current(): ForkT | null { return forkLoaded; } };
/** Get in Clax's link (an app's) once `loadMoreMenu` has loaded it, else null. */
export const getInClax = { get current(): GetT | null { return getLoaded; } };
/** The Clone dialog's host (no button of its own) once `loadMoreMenu` has loaded it, else null. */
export const clone = { get current(): CloneT | null { return cloneLoaded; } };
/** What opens the Clone dialog while the viewer may clone (the more menu's
 * Clone with git item), else null. */
export const cloneOpen = { get current(): (() => void) | null { return cloneOpener; } };

/** Sets what opens the Clone dialog (the Clone host sets it once it knows
 * the viewer may clone; null clears it). */
export function setCloneOpener(f: (() => void) | null): void { cloneOpener = f; }

/** The Share button's component once `loadMoreMenu` has loaded it, else null. */
export const share = { get current(): ShareT | null { return shareLoaded; } };
/** What opens the Share dialog while the viewer may share (the more menu's
 * Share item at phone width), else null. */
export const shareOpen = { get current(): (() => void) | null { return shareOpener; } };
/** Sets what opens the Share dialog (the Share button sets it; null clears it). */
export function setShareOpener(f: (() => void) | null): void { shareOpener = f; }

/** The Publish button's component once `loadMoreMenu` has loaded it, else null. */
export const publish = { get current(): PublishT | null { return publishLoaded; } };
/** What the more menu's Publish item opens while there is something to
 * publish (at phone width), else null. */
export const publishOpen = { get current(): PublishOpener | null { return publishOpener; } };
/** Sets the Publish item (the Publish button sets it; null clears it). */
export function setPublishOpener(o: PublishOpener | null): void { publishOpener = o; }

/** The Permissions panel's component once `loadMoreMenu` has loaded it, else null. */
export const permissions = { get current(): PermissionsT | null { return permissionsLoaded; } };
/** Opens the Permissions panel; `"policy"` focuses its Content Security Policy setting (the blocked notice's Permissions…). */
export type PermissionsOpener = (at?: "policy") => void;
/** What opens the Permissions panel while it is mounted (the more menu's Permissions… item, the blocked notice's), else null. */
export const permissionsOpen = { get current(): PermissionsOpener | null { return permissionsOpener; } };
/** Sets what opens the Permissions panel (the panel sets it on mount; null clears it). */
export function setPermissionsOpener(f: PermissionsOpener | null): void { permissionsOpener = f; }

/** The more menu's Usage item while ClaxBot is on for this person, else null. */
export const usage = { get current(): UsageItem | null { return usageItem; } };
/** Sets the Usage item (the artifact view's ClaxBot part sets it from the
 * week's meter; null clears it). */
export function setUsage(u: UsageItem | null): void { usageItem = u; }

/** The more menu's ClaxBot item at phone width (the bar's ClaxBot button
 * is hidden there) while the artifact has runs, else null. */
export const botOpen = { get current(): PublishOpener | null { return botOpener; } };
/** Sets the ClaxBot item (the artifact view's ClaxBot part sets it; null clears it). */
export function setBotOpener(o: PublishOpener | null): void { botOpener = o; }

let analyticsOpener: (() => void) | null = $state(null);
/** What opens the Analytics panel while it is mounted (the top bar mounts
 * it, its own lazy chunk, for whoever administers a hosted clax), else null. */
export const analyticsOpen = { get current(): (() => void) | null { return analyticsOpener; } };
/** Sets what opens the Analytics panel (the panel sets it on mount; null clears it). */
export function setAnalyticsOpener(f: (() => void) | null): void { analyticsOpener = f; }

/** ClaxBot's part of the bar (`bot/ArtifactBot.svelte`) once `loadMoreMenu`
 * has loaded it, where the bootstrap block says ClaxBot is on for this
 * browser or the artifact has runs (`view/deciders.svelte.ts`), so its runs
 * button shows with the bar; else null (the roster loads it itself). */
export const botPart = { get current(): Component<{ ctl: ArtifactController }> | null { return botLoaded; } };

/** Whether a chunk `loadMoreMenu` asked for is still on its way. */
export const loading = { get current(): boolean { return pending > 0; } };

/** The hosted Clax's documents once `loadMoreMenu` has asked for them, else null. */
export const legal = { get current(): LegalLinks | null { return legalLinks; } };

/** Loads the more menu's, the roster's, the Clone dialog host's and the
 * Fork, Get in Clax, Share and Publish buttons' code, and ClaxBot's part
 * where the block says it is on or the artifact has runs (the bar waits
 * for that answer). A failed load leaves its part out. */
export function loadMoreMenu(): void {
  const load = <T>(p: Promise<T>, take: (m: T) => void) => {
    pending++;
    p.then(take, () => {}).finally(() => { pending--; });
  };
  load(import("./MoreMenu.svelte"), m => { loaded = m.default; });
  load(import("./Who.svelte"), m => { whoLoaded = m.default; });
  load(import("./Clone.svelte"), m => { cloneLoaded = m.default; });
  load(import("./ForkButton.svelte"), m => { forkLoaded = m.default; });
  load(import("./GetInClax.svelte"), m => { getLoaded = m.default; });
  load(import("./Share.svelte"), m => { shareLoaded = m.default; });
  load(import("./PermissionsPanel.svelte"), m => { permissionsLoaded = m.default; });
  const bot = () => import("../bot/ArtifactBot.svelte");
  if (botKnown()?.enabled) load(bot(), m => { botLoaded = m.default; });
  // Its code once the runs say it shows: the runs are the bar's capped wait (`barDecided`), its code not.
  else void (runsAsked() ?? Promise.resolve([])).then(r => { if (r.length) load(bot(), m => { botLoaded = m.default; }); }, () => {});
  const publishCode = () => load(import("./Publish.svelte"), m => { publishLoaded = m.default; });
  if (ownerProof()) publishCode();
  else void getToken().then(t => { if (t && !t.startsWith("cxo_")) publishCode(); });
  // A hosted Clax's documents, and its prompt for a person who has not
  // agreed to them. The owner's shell on a local daemon holds a token that
  // is not a hosted owner's (`cxo_`), and asks nothing.
  void getToken().then(t => (!t || t.startsWith("cxo_") ? import("../hosted/legal").then(m => m.hostedMe()) : null)).then(me => { legalLinks = me?.legal ?? null; }, () => {});
}
