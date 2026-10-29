import { describe, expect, it } from "vitest";
import { acceptFromShell, forwardedKey, pageShortcut, shellOrigins } from "../src/channel";
import { dispatchTrusted, trusted } from "./trusted";

describe("shellOrigins", () => {
  it("allows a local daemon's shell on each loopback name at its port", () => {
    const loopback = ["http://localhost:7480", "http://127.0.0.1:7480"];
    expect(shellOrigins("http://localhost:7480")).toEqual(loopback);
    expect(shellOrigins("http://127.0.0.1:7480")).toEqual(loopback);
  });
  it("allows only the origin the bridge tag names (data-origin) otherwise", () => {
    expect(shellOrigins("https://clax.example")).toEqual(["https://clax.example"]);
    expect(shellOrigins("http://192.168.1.5:7480")).toEqual(["http://192.168.1.5:7480"]);
  });
  it("allows no one without an origin, or with one that does not parse", () => {
    expect(shellOrigins(undefined)).toEqual([]);
    expect(shellOrigins("not an origin")).toEqual([]);
  });
});

describe("acceptFromShell", () => {
  const origins = ["http://localhost:7480"];
  const ev = (data: unknown, origin: string, source: Window | null) => trusted(new MessageEvent("message", { data, origin, source }));
  it("accepts known messages from the parent at an allowed origin", () => {
    expect(acceptFromShell(ev({ type: "clax:comment-mode", on: true }, "http://localhost:7480", window), window, origins)).toEqual({ type: "clax:comment-mode", on: true });
  });
  it("rejects a message the page made up and dispatched itself, whatever source and origin it names", () => {
    const forged = new MessageEvent("message", { data: { type: "clax:composer-ready", pickId: "p1" }, origin: "http://localhost:7480", source: window });
    expect(forged.isTrusted).toBe(false);
    expect(acceptFromShell(forged, window, origins)).toBeNull();
  });
  it("rejects other sources, origins, and types", () => {
    expect(acceptFromShell(ev({ type: "clax:comment-mode", on: true }, "http://evil.test", window), window, origins)).toBeNull();
    expect(acceptFromShell(ev({ type: "clax:comment-mode", on: true }, "http://localhost:7480", null), window, origins)).toBeNull();
    expect(acceptFromShell(ev({ type: "clax:pick" }, "http://localhost:7480", window), window, origins)).toBeNull();
    expect(acceptFromShell(ev("clax:comment-mode", "http://localhost:7480", window), window, origins)).toBeNull();
  });
});

describe("forwardedKey", () => {
  it("takes Option, Up, Down, and Escape with a direction, and nothing else", () => {
    for (const key of ["Alt", "ArrowUp", "ArrowDown", "Escape"]) expect(forwardedKey({ key, down: true })).toEqual({ key, down: true });
    expect(forwardedKey({ key: "Escape", down: false })).toEqual({ key: "Escape", down: false });
    expect(forwardedKey({ key: "Enter", down: true })).toBeNull();
    expect(forwardedKey({ key: "Escape", down: "yes" })).toBeNull();
    expect(forwardedKey({ key: 5, down: true })).toBeNull();
  });
});

describe("pageShortcut", () => {
  /** What `pageShortcut` says of keydown `init` on `target`, read while the
   * event is dispatched, as the bridge's listener reads it. */
  const at = (target: Element, init: KeyboardEventInit, opts: { untrusted?: boolean; cancel?: boolean } = {}) => {
    let got: "c" | "f" | null | undefined;
    const e = new KeyboardEvent("keydown", { bubbles: true, composed: true, cancelable: true, ...init });
    if (opts.cancel) target.addEventListener("keydown", ev => ev.preventDefault(), { once: true });
    const listen = (ev: Event) => { got = pageShortcut(ev as KeyboardEvent); };
    window.addEventListener("keydown", listen);
    try {
      if (opts.untrusted) target.dispatchEvent(e); else dispatchTrusted(target, e);
    } finally {
      window.removeEventListener("keydown", listen);
    }
    return got;
  };
  const el = (html: string) => {
    const box = document.createElement("div");
    box.innerHTML = html;
    document.body.replaceChildren(box);
    return box.querySelector("[data-t]")!;
  };

  it("forwards C pressed outside a text field, in either case", () => {
    const p = el("<p data-t>Text</p>");
    expect(at(p, { key: "c" })).toBe("c");
    expect(at(p, { key: "C" })).toBe("c");
    expect(at(document.body, { key: "c" })).toBe("c");
  });

  it("forwards F, the report key, in either case or by its key's position", () => {
    const p = el("<p data-t>Text</p>");
    expect(at(p, { key: "f" })).toBe("f");
    expect(at(p, { key: "F" })).toBe("f");
    // A layout whose F key types another letter.
    expect(at(p, { key: "а", code: "KeyF" })).toBe("f");
    expect(at(p, { key: "f", ctrlKey: true })).toBeNull();
    expect(at(p, { key: "F", shiftKey: true })).toBeNull();
    expect(at(el("<input data-t>"), { key: "f" })).toBeNull();
  });

  it("forwards no F pressed in the page's own dialog, as the shell's F skips one", () => {
    expect(at(el("<div role=dialog><button data-t>OK</button></div>"), { key: "f" })).toBeNull();
    expect(at(el("<div aria-modal=true><button data-t>OK</button></div>"), { key: "f" })).toBeNull();
    // C keeps its rules there.
    expect(at(el("<div role=dialog><button data-t>OK</button></div>"), { key: "c" })).toBe("c");
  });

  it("forwards no other key: not ?, not a letter, not Escape", () => {
    const p = el("<p data-t>Text</p>");
    for (const key of ["?", "t", "x", "Escape", "Enter", " "]) expect(at(p, { key }), key).toBeNull();
  });

  it("forwards nothing with a modifier, on a repeat, or while an input method composes", () => {
    const p = el("<p data-t>Text</p>");
    for (const mod of ["shiftKey", "ctrlKey", "altKey", "metaKey", "repeat", "isComposing"] as const) expect(at(p, { key: "c", [mod]: true }), mod).toBeNull();
  });

  it("forwards nothing typed into a field, editable content, or a custom text field or combo box", () => {
    for (const html of [
      "<input data-t>", "<textarea data-t></textarea>", "<select data-t><option>a</option></select>",
      "<div contenteditable data-t>Edit</div>", "<div contenteditable=true><b data-t>Edit</b></div>", "<div contenteditable=plaintext-only data-t>Edit</div>",
      "<div role=textbox data-t>Edit</div>", "<div role=combobox><span data-t>Pick</span></div>",
    ]) expect(at(el(html), { key: "c" }), html).toBeNull();
    expect(at(el("<div contenteditable=false data-t>Not editable</div>"), { key: "c" })).toBe("c");
  });

  it("forwards nothing from a field inside an open shadow root", () => {
    const host = el("<div data-t></div>");
    const input = document.createElement("input");
    host.attachShadow({ mode: "open" }).append(input);
    expect(at(input, { key: "c" })).toBeNull();
  });

  it("forwards nothing the page cancelled or dispatched itself", () => {
    const p = el("<p data-t>Text</p>");
    expect(at(p, { key: "c" }, { cancel: true })).toBeNull();
    expect(at(p, { key: "c" }, { untrusted: true })).toBeNull();
  });
});
