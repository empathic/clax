import { describe, expect, it } from "vitest";
import type { Prompt } from "../caps/grants";
import { type Ask, promptQueue } from "./prompt-queue";

const p: Prompt = { kind: "capability", title: "t", body: "b", allow: "Allow", deny: "Deny" };

describe("promptQueue", () => {
  it("shows one prompt at a time, in order", async () => {
    const shown: (Ask | null)[] = [];
    const ask = promptQueue(a => shown.push(a));
    const first = ask(p);
    const second = ask({ ...p, title: "t2" });
    await Promise.resolve();
    expect(shown.filter(Boolean)).toHaveLength(1);
    shown.at(-1)!.answer("allow");
    await expect(first).resolves.toBe("allow");
    await new Promise(r => setTimeout(r, 0));
    expect(shown.at(-1)!.prompt.title).toBe("t2");
    shown.at(-1)!.answer("deny");
    await expect(second).resolves.toBe("deny");
  });

  it("gives each ask its own ID and carries the prompt's kind and always label", async () => {
    const shown: (Ask | null)[] = [];
    const ask = promptQueue(a => shown.push(a));
    void ask({ ...p, kind: "crossRead", always: "Always for this app" });
    void ask({ ...p, kind: "download" });
    await Promise.resolve();
    const a = shown.at(-1)!;
    expect(a.prompt).toMatchObject({ kind: "crossRead", always: "Always for this app" });
    a.answer("allow");
    await new Promise(r => setTimeout(r, 0));
    const b = shown.at(-1)!;
    expect(b.prompt.kind).toBe("download");
    expect(b.prompt.always).toBeUndefined();
    expect(typeof a.id).toBe("string");
    expect(a.id).not.toBe(b.id);
    // A second load's queue names its asks apart from this one's.
    const other: (Ask | null)[] = [];
    void promptQueue(x => other.push(x))(p);
    await Promise.resolve();
    expect([a.id, b.id]).not.toContain(other.at(-1)!.id);
  });

  it("answers once: a second answer to the same ask changes nothing", async () => {
    const shown: (Ask | null)[] = [];
    const ask = promptQueue(a => shown.push(a));
    const first = ask(p);
    void ask({ ...p, title: "t2" });
    await Promise.resolve();
    const a = shown.at(-1)!;
    a.answer("deny");
    a.answer("allow");
    await expect(first).resolves.toBe("deny");
    await new Promise(r => setTimeout(r, 0));
    expect(shown.filter(Boolean).map(x => x!.prompt.title)).toEqual(["t", "t2"]);
  });

  it("cancel dismisses the prompt shown and every queued one", async () => {
    const shown: (Ask | null)[] = [];
    const ask = promptQueue(a => shown.push(a));
    const first = ask(p);
    const second = ask({ ...p, title: "t2" });
    await Promise.resolve();
    ask.cancel();
    await expect(first).resolves.toBe("dismiss");
    await expect(second).resolves.toBe("dismiss");
    await new Promise(r => setTimeout(r, 0));
    expect(shown.at(-1)).toBeNull();
    expect(shown.filter(Boolean).map(x => x!.prompt.title)).toEqual(["t"]);
    // The queue goes on afterwards.
    const third = ask({ ...p, title: "t3" });
    await new Promise(r => setTimeout(r, 0));
    expect(shown.at(-1)!.prompt.title).toBe("t3");
    shown.at(-1)!.answer("allow");
    await expect(third).resolves.toBe("allow");
  });
});
