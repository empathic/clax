import type { Prompt, PromptAnswer } from "../caps/grants";

/** A prompt shown: `id` names it to the Clax app's host (`askAnswer`),
 * unique in this load and apart from other loads'; `answer` resolves it,
 * once (a later answer changes nothing). */
export type Ask = { id: string; prompt: Prompt; answer(a: PromptAnswer): void };

/** How long "Allow" stays disabled, counted from the dialog's first paint. */
export const ALLOW_DELAY_MS = 500;

/** A prompt function that shows one dialog at a time through `setAsk`;
 * its `cancel` dismisses the prompt shown and every queued one (the page
 * that asked went away). */
export function promptQueue(setAsk: (a: Ask | null) => void): ((p: Prompt) => Promise<PromptAnswer>) & { cancel(): void } {
  let chain: Promise<unknown> = Promise.resolve();
  let n = 0;
  const load = Math.random().toString(36).slice(2, 8);
  // Each prompt not yet answered, by the function that dismisses it.
  const open = new Set<() => void>();
  const ask = (p: Prompt) => {
    let answer: ((a: PromptAnswer) => void) | null = null;
    // Shown, it is answered "dismiss"; queued, it is never shown.
    const drop = () => { if (answer) answer("dismiss"); else open.delete(drop); };
    open.add(drop);
    const next = chain.then(() => new Promise<PromptAnswer>(resolve => {
      if (!open.has(drop)) { resolve("dismiss"); return; }
      answer = a => {
        if (!open.delete(drop)) return;
        setAsk(null);
        resolve(a);
      };
      setAsk({ id: `${load}-${++n}`, prompt: p, answer });
    }));
    chain = next.catch(() => {});
    return next;
  };
  return Object.assign(ask, { cancel: () => { for (const f of [...open]) f(); } });
}
