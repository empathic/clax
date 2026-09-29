import { useEffect, useRef, useState } from "preact/hooks";
import type { Prompt, PromptAnswer } from "./caps/grants";

export type Ask = { prompt: Prompt; answer(a: PromptAnswer): void };

/** How long "Allow" stays disabled after the dialog opens. */
export const ALLOW_DELAY_MS = 500;

/** The one modal the shell shows for a page: a capability's consent or a
 * download's confirmation. It opens with focus on the refusing button, and
 * "Allow" stays disabled for [`ALLOW_DELAY_MS`], so a keystroke meant for the
 * page cannot grant consent. Escape dismisses it (neither allow nor deny). */
export function PromptDialog({ ask }: { ask: Ask }) {
  const deny = useRef<HTMLButtonElement>(null);
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    setArmed(false);
    deny.current?.focus();
    const timer = setTimeout(() => setArmed(true), ALLOW_DELAY_MS);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") ask.answer("dismiss"); };
    addEventListener("keydown", onKey);
    return () => { clearTimeout(timer); removeEventListener("keydown", onKey); };
  }, [ask]);
  return (
    <div class="prompt-backdrop">
      <div class="prompt" role="dialog" aria-modal="true" aria-labelledby="prompt-title" aria-describedby="prompt-body">
        <h2 id="prompt-title">{ask.prompt.title}</h2>
        <p id="prompt-body">{ask.prompt.body}</p>
        <div class="actions">
          <button type="button" ref={deny} onClick={() => ask.answer("deny")}>{ask.prompt.deny}</button>
          <button type="button" class="primary" disabled={!armed} onClick={() => { if (armed) ask.answer("allow"); }}>{ask.prompt.allow}</button>
        </div>
      </div>
    </div>
  );
}

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
