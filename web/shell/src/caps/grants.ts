// Per-viewer, per-artifact permission state (permissions.d.ts). Grants persist
// in localStorage under `clax.grants.v1:<aid>:<viewer public ID>`; a denial
// or a dismissed prompt lasts for the page load only. One dialog at a time.
import { CAPABILITIES, type Declared, consentGated, isAvailable } from "./availability";

export type PermissionState = "granted" | "prompt" | "denied" | "unavailable";
export type Prompt = { title: string; body: string; allow: string; deny: string };
export type PromptAnswer = "allow" | "deny" | "dismiss";

export const grantsKey = (aid: string, viewer: string) => `clax.grants.v1:${aid}:${viewer}`;

const ASKS: Record<string, string> = { comments: "post comments on this artifact under your name" };

function read(storage: Storage | null, key: string): string[] {
  try {
    const v: unknown = JSON.parse(storage?.getItem(key) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function write(storage: Storage | null, key: string, names: string[]): void {
  try {
    storage?.setItem(key, JSON.stringify(names));
  } catch {
    // Storage unavailable: the grant lasts for this page load.
  }
}

export class Grants {
  private readonly granted: Set<string>;
  private readonly denied = new Set<string>();
  private readonly dismissed = new Set<string>();
  private chain: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly key: string,
    private readonly storage: Storage | null,
    private readonly declared: Declared,
    private readonly owner: boolean,
    private readonly ask: (p: Prompt) => Promise<PromptAnswer>,
  ) {
    this.granted = new Set(read(storage, key));
  }

  state(name: string): PermissionState {
    if (name === "permissions" || !isAvailable(name, this.declared, this.owner)) return "unavailable";
    if (!consentGated(name, this.declared) || this.granted.has(name)) return "granted";
    if (this.denied.has(name) || this.dismissed.has(name)) return "denied";
    return "prompt";
  }

  /** Every available capability's state (unavailable ones omitted). */
  all(): Record<string, PermissionState> {
    const out: Record<string, PermissionState> = {};
    for (const n of CAPABILITIES) {
      const s = this.state(n);
      if (s !== "unavailable") out[n] = s;
    }
    return out;
  }

  /** Why a consent-gated write cannot go ahead: `forbidden` after "Don't
   * allow", `consent_required` after a dismissed prompt; `null` otherwise. */
  refusal(name: string): "forbidden" | "consent_required" | null {
    if (this.denied.has(name)) return "forbidden";
    if (this.dismissed.has(name)) return "consent_required";
    return null;
  }

  /** Asks in one dialog for every name in `names` whose state is `prompt`;
   * names already decided this load are never asked again. */
  request(names: readonly string[]): Promise<void> {
    const run = async () => {
      const askable = [...new Set(names)].filter(n => this.state(n) === "prompt");
      if (!askable.length) return;
      const answer = await this.ask({
        title: "Allow this page to act as you?",
        body: `This page asks to ${askable.map(n => ASKS[n] ?? `use ${n}`).join(" and ")}.`,
        allow: "Allow",
        deny: "Don't allow",
      });
      for (const n of askable) {
        if (answer === "allow") this.granted.add(n);
        else if (answer === "deny") this.denied.add(n);
        else this.dismissed.add(n);
      }
      if (answer === "allow") write(this.storage, this.key, [...this.granted]);
    };
    const p = this.chain.then(run, run);
    this.chain = p.catch(() => {});
    return p;
  }
}
