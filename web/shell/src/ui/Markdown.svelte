<svelte:options css="injected" />

<script module lang="ts">
  import { loadMarkdown, md } from "../view/md-load.svelte";
  // Whatever shows a comment loads the Markdown module as it loads.
  void loadMarkdown();
</script>

<script lang="ts">
  // A comment body as Markdown (`view/markdown.ts`): until the module is
  // here, its text as typed. Every string in the tree is text; a link's
  // address is checked again here (`safeHref`), opens in a new tab, and a
  // click on it is its own, not the card's.
  import type { Block, Inline, Item } from "../view/markdown";
  let { text, class: cls }: { text: string; class?: string } = $props();
  const tree = $derived(md.m ? md.m.parse(text) : null);
  const href = (h: string) => md.m?.safeHref(h) ?? null;
</script>

<div class={["md", cls]}>{#if tree}{@render blocks(tree)}{:else}<p>{text}</p>{/if}</div>

{#snippet blocks(bs: Block[])}{#each bs as b, i (i)}{@render block(b)}{/each}{/snippet}

<!-- A comment's headings sit under the page's: # is h3, and so on to h6. -->
{#snippet block(b: Block)}{#if b.t === "p"}<p>{@render inline(b.c)}</p>{:else if b.t === "line"}<span class="line">{@render inline(b.c)}</span>{:else if b.t === "h"}{#if b.level === 1}<h3 class="h1">{@render inline(b.c)}</h3>{:else if b.level === 2}<h4 class="h2">{@render inline(b.c)}</h4>{:else if b.level === 3}<h5 class="h3">{@render inline(b.c)}</h5>{:else}<h6 class={`h${b.level}`}>{@render inline(b.c)}</h6>{/if}{:else if b.t === "pre"}<pre><code>{b.v}</code></pre>{:else if b.t === "quote"}<blockquote>{@render blocks(b.c)}</blockquote>{:else if b.t === "list"}{#if b.ordered}<ol start={b.start ?? undefined}>{@render items(b.items)}</ol>{:else}<ul>{@render items(b.items)}</ul>{/if}{:else if b.t === "table"}<div class="tbl"><table><thead><tr>{#each b.head as c, j (j)}<th style:text-align={b.align[j]}>{@render inline(c)}</th>{/each}</tr></thead>{#if b.rows.length}<tbody>{#each b.rows as r, k (k)}<tr>{#each r as c, j (j)}<td style:text-align={b.align[j]}>{@render inline(c)}</td>{/each}</tr>{/each}</tbody>{/if}</table></div>{:else if b.t === "hr"}<hr />{/if}{/snippet}

{#snippet items(list: Item[])}{#each list as it, i (i)}<li class={[it.task && "task"]}>{#if it.task}<input type="checkbox" disabled checked={it.checked} />{/if}{@render blocks(it.c)}</li>{/each}{/snippet}

{#snippet inline(c: Inline[])}{#each c as x, i (i)}{#if x.t === "text"}{x.v}{:else if x.t === "code"}<code>{x.v}</code>{:else if x.t === "em"}<em>{@render inline(x.c)}</em>{:else if x.t === "strong"}<strong>{@render inline(x.c)}</strong>{:else if x.t === "del"}<del>{@render inline(x.c)}</del>{:else if x.t === "br"}<br />{:else if x.t === "link"}{@const h = href(x.href)}{#if h}<a href={h} target="_blank" rel="noopener noreferrer nofollow" onclick={e => e.stopPropagation()}>{@render inline(x.c)}</a>{:else}{@render inline(x.c)}{/if}{/if}{/each}{/snippet}

<!-- Sized for a comment card: headings a step above the text, code and
     tables scrolling within the card rather than widening it. -->
<style>
  :global {
    .md { min-width: 0; overflow-wrap: anywhere; }
    .md > :first-child { margin-top: 0; }
    .md > :last-child { margin-bottom: 0; }
    .md :is(p, li, th, td, h3, h4, h5, h6) { white-space: pre-wrap; }
    .md p { margin: 0; }
    .md p + p, .md p + :is(ul, ol, pre, blockquote, .tbl), .md :is(ul, ol, pre, blockquote, .tbl) + p { margin-top: .5em; }
    .md :is(h3, h4, h5, h6) { margin: .7em 0 .2em; font-weight: 650; line-height: 1.35; color: var(--fg); }
    .md .h1 { font-size: 15.5px; }
    .md .h2 { font-size: 14.5px; }
    .md :is(.h3, .h4, .h5, .h6) { font-size: 13.5px; }
    .md :is(.h5, .h6) { color: var(--muted); }
    .md code { font: 12.5px/1.4 var(--mono); padding: 1px 4px; border-radius: var(--radius-xs); background: var(--hover); }
    .md pre { margin: .5em 0; padding: 8px 10px; overflow-x: auto; white-space: pre; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); tab-size: 4; }
    .md pre code { padding: 0; background: none; font: 12px/1.45 var(--mono); }
    .md :is(ul, ol) { margin: .35em 0; padding-left: 1.45em; }
    .md li + li { margin-top: .15em; }
    .md li > :is(ul, ol) { margin: .15em 0 0; }
    .md li.task { list-style: none; }
    .md li.task > input { width: 13px; height: 13px; min-height: 0; padding: 0; margin: 0 6px 0 -1.25em; vertical-align: -2px; accent-color: var(--accent); }
    .md blockquote { margin: .5em 0; padding: 0 0 0 10px; border-left: 2px solid var(--border-strong); color: var(--muted); }
    .md hr { margin: .7em 0; border: 0; border-top: 1px solid var(--border); }
    .md a { color: var(--accent-ink); text-decoration: underline; text-underline-offset: 2px; }
    .md a:hover { text-decoration-thickness: 2px; }
    .md del { color: var(--muted); }
    .md .tbl { margin: .5em 0; max-width: 100%; overflow-x: auto; }
    .md table { border-collapse: collapse; font-size: 12.5px; line-height: 1.4; }
    .md :is(th, td) { padding: 3px 8px; border: 1px solid var(--border); text-align: left; vertical-align: top; overflow-wrap: normal; }
    .md th { font-weight: 600; background: var(--hover); }
  }
</style>
