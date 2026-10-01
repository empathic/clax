import type { Prompt, PromptAnswer } from "../caps/grants";

export type Ask = { prompt: Prompt; answer(a: PromptAnswer): void };

/** How long "Allow" stays disabled after the dialog opens. */
export const ALLOW_DELAY_MS = 500;

/** A prompt function that shows one dialog at a time through `setAsk`. */
export function promptQueue(setAsk: (a: Ask | null) => void): (p: Prompt) => Promise<PromptAnswer> {
  let chain: Promise<unknown> = Promise.resolve();
  return p => {
    const next = chain.then(() => new Promise<PromptAnswer>(resolve => {
      setAsk({ prompt: p, answer: a => { setAsk(null); resolve(a); } });
    }));
    chain = next.catch(() => {});
    return next;
  };
}
