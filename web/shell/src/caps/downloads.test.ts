import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { forgetBudgets } from "./budget";
import { ALLOWED_EXTENSIONS, downloadsHandler, extensionOf, sanitizeFilename, saveBlob } from "./downloads";
import type { CapEnv } from "./host";

describe("downloads in the shell", () => {
  // jsdom has no URL.createObjectURL/revokeObjectURL; define them so they can be spied on.
  beforeEach(() => {
    Object.defineProperty(URL, "createObjectURL", { value: () => "blob:x", configurable: true, writable: true });
    Object.defineProperty(URL, "revokeObjectURL", { value: () => {}, configurable: true, writable: true });
  });
  afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); sessionStorage.clear(); forgetBudgets(); });

  it("sanitizes filenames", () => {
    expect(sanitizeFilename("../../etc/pa\u200Bss wd.txt")).toBe("etc_pass wd.txt");
    expect(sanitizeFilename("..\\..\\x.txt")).toBe("x.txt");
    expect(sanitizeFilename(" \u200B../.hidden.txt")).toBe("hidden.txt");
    expect(sanitizeFilename("a/../b.txt")).toBe("a_.._b.txt");
    expect(sanitizeFilename("a \u200B b.txt")).toBe("a b.txt");
    expect(sanitizeFilename("  a\t\tb.csv ")).toBe("a b.csv");
    const long = sanitizeFilename(`${"é".repeat(200)}.csv`);
    expect(new TextEncoder().encode(long).length).toBeLessThanOrEqual(240);
    expect(long.endsWith(".csv")).toBe(true);
    expect(extensionOf("Report.CSV")).toBe("csv");
    expect(extensionOf("noext")).toBeNull();
  });

  it("allows exactly the contract's extensions", () => {
    expect([...ALLOWED_EXTENSIONS].sort()).toEqual("gif png jpg jpeg webp mp4 webm txt json md docx pptx epub csv ttf html svg pdf xlsx zip".split(" ").sort());
  });

  it("saves through an object URL that is revoked afterwards", () => {
    vi.useFakeTimers();
    const revoked: string[] = [];
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:y");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(u => { revoked.push(u); });
    const clicked: { href: string; download: string }[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) { clicked.push({ href: this.href, download: this.download }); });
    saveBlob(new Blob(["x"]), "a.txt");
    expect(clicked).toEqual([{ href: "blob:y", download: "a.txt" }]);
    expect(document.querySelector("a[download]")).toBeNull();
    vi.runAllTimers();
    expect(revoked).toEqual(["blob:y"]);
  });

  it("asks the viewer, saves on Save, and rejects otherwise", async () => {
    const created = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    const clicks: string[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) { clicks.push(this.download); });
    const answers: ("allow" | "deny" | "dismiss")[] = ["allow", "deny", "dismiss"];
    const prompt = vi.fn(async (_p: unknown) => answers.shift()!);
    const h = downloadsHandler({ aid: "7q3k9mzx2b4t", prompt } as unknown as CapEnv, null as never);
    await expect(h.call("save", [{ filename: "report.csv", blob: new Blob(["a,b"]) }])).resolves.toEqual({ status: "saved" });
    expect(prompt.mock.calls[0][0]).toMatchObject({ body: "report.csv (3 bytes)", allow: "Save" });
    expect(clicks).toEqual(["report.csv"]);
    expect((created.mock.calls[0][0] as Blob).type).toBe("text/csv");
    await expect(h.call("save", [{ filename: "report.csv", blob: new Blob(["a"]) }])).rejects.toMatchObject({ code: "declined" });
    await expect(h.call("save", [{ filename: "report.csv", blob: new Blob(["a"]) }])).rejects.toMatchObject({ code: "declined" });
    expect(clicks).toHaveLength(1);
  });

  it("refuses unlisted extensions and a second prompt while one is open", async () => {
    let answer!: (a: "allow") => void;
    const prompt = vi.fn(() => new Promise<"allow">(r => { answer = r; }));
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const h = downloadsHandler({ aid: "7q3k9mzx2b4t", prompt } as unknown as CapEnv, null as never);
    for (const bad of ["run.exe", "app.js", "x.bat", "noext", "a.csv.exe", "   "]) {
      await expect(h.call("save", [{ filename: bad, blob: new Blob(["x"]) }])).rejects.toMatchObject({ code: "rejected_extension" });
    }
    const first = h.call("save", [{ filename: "a.txt", blob: new Blob(["x"]) }]);
    await vi.waitFor(() => expect(prompt).toHaveBeenCalledTimes(1));
    await expect(h.call("save", [{ filename: "b.txt", blob: new Blob(["x"]) }])).rejects.toMatchObject({ code: "rate_limited" });
    answer("allow");
    await expect(first).resolves.toEqual({ status: "saved" });
    expect(prompt).toHaveBeenCalledTimes(1);
  });

  it("refuses hostile arguments without a prompt or a save", async () => {
    const prompt = vi.fn(async () => "allow" as const);
    const created = vi.spyOn(URL, "createObjectURL");
    const h = downloadsHandler({ aid: "7q3k9mzx2b4t", prompt } as unknown as CapEnv, null as never);
    const bad: [unknown[], string][] = [
      [[], "bad_request"],
      [[null], "bad_request"],
      [[{ filename: 7, blob: new Blob(["x"]) }], "bad_request"],
      [[{ filename: "x".repeat(509) + ".txt", blob: new Blob(["x"]) }], "bad_request"],
      [[{ filename: "a.txt", blob: "x" }], "bad_request"],
      [[{ filename: "a.txt", blob: new Blob([]) }], "bad_request"],
      [[{ filename: "a.txt", blob: new Blob(["x"]), request: 5 }], "bad_request"],
      [[{ filename: "a.txt", blob: new Blob(["x"]), request: "tok" }], "request_unknown"],
    ];
    for (const [args, code] of bad) await expect(h.call("save", args)).rejects.toMatchObject({ code });
    await expect(h.call("open", [])).rejects.toMatchObject({ code: "capability_removed" });
    expect(prompt).not.toHaveBeenCalled();
    expect(created).not.toHaveBeenCalled();
  });

  it("after dispose saves nothing, even when the viewer then accepts", async () => {
    let answer!: (a: "allow") => void;
    const prompt = vi.fn(() => new Promise<"allow">(r => { answer = r; }));
    const created = vi.spyOn(URL, "createObjectURL");
    const h = downloadsHandler({ aid: "7q3k9mzx2b4t", prompt } as unknown as CapEnv, null as never);
    const p = h.call("save", [{ filename: "a.txt", blob: new Blob(["x"]) }]);
    await vi.waitFor(() => expect(prompt).toHaveBeenCalledTimes(1));
    h.dispose!();
    answer("allow");
    await expect(p).rejects.toHaveProperty("code");
    expect(created).not.toHaveBeenCalled();
    await expect(h.call("save", [{ filename: "a.txt", blob: new Blob(["x"]) }])).rejects.toHaveProperty("code");
    expect(prompt).toHaveBeenCalledTimes(1);
  });

  it("shows at most 3 prompts per 30 s, saved or declined, then rejects rate_limited with no dialog", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(5_000_000);
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const answers: ("allow" | "deny")[] = ["allow", "deny", "deny", "allow"];
    const prompt = vi.fn(async (_p: unknown) => answers.shift()!);
    const h = downloadsHandler({ aid: "7q3k9mzx2b4t", prompt } as unknown as CapEnv, null as never);
    const save = () => h.call("save", [{ filename: "a.txt", blob: new Blob(["x"]) }]);
    await expect(save()).resolves.toEqual({ status: "saved" });
    await expect(save()).rejects.toMatchObject({ code: "declined" });
    await expect(save()).rejects.toMatchObject({ code: "declined" });
    await expect(save()).rejects.toMatchObject({ code: "rate_limited", message: expect.stringMatching(/30 s/) });
    expect(prompt).toHaveBeenCalledTimes(3);
    // Another handler for the same artifact in this tab (a reload) shares the budget.
    forgetBudgets();
    const again = downloadsHandler({ aid: "7q3k9mzx2b4t", prompt } as unknown as CapEnv, null as never);
    await expect(again.call("save", [{ filename: "a.txt", blob: new Blob(["x"]) }])).rejects.toMatchObject({ code: "rate_limited" });
    vi.setSystemTime(5_030_000);
    await expect(save()).resolves.toEqual({ status: "saved" });
    expect(prompt).toHaveBeenCalledTimes(4);
  });
});
