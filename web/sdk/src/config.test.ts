import Ajv2020 from "ajv/dist/2020";
import COMMITTED from "../clax.config.schema.json";
import { describe, expect, it } from "vitest";
import { FIELDS, configSchema, flatten, isDevOrigin, linkOverride, parseContainer, readOverride, resolve, writeOverride } from "./config";

describe("the config schema", () => {
  it("is the committed clax.config.schema.json (regenerate it from configSchema())", () => {
    expect(COMMITTED).toEqual(configSchema());
  });

  it("takes a nested config of every axis, and refuses an unknown field or a bad value", () => {
    const validate = new Ajv2020({ strict: false }).compile(configSchema());
    const good = {
      enabled: true,
      entry: { launcher: false, targets: true, key: "f", palette: true, link: true },
      targets: [{ selector: "#feedback", action: "report" }],
      presentation: "dialog", render: "native", start: "note",
      attachments: { screenshot: true, errors: false, logs: false, version: true, browser: true },
      annotate: true, filters: { "code-blur": true, "code-selector": ".source-cell" }, pickers: { "code-line": true }, pins: false,
      theme: { "--clax-bg": "#fff" }, overrides: { viewer: "local" },
    };
    expect(validate(good), JSON.stringify(validate.errors)).toBe(true);
    expect(validate({ presentation: "popover" })).toBe(false);
    expect(validate({ entry: { mouse: true } })).toBe(false);
    expect(validate({ theme: { "--bg": "red" } })).toBe(false);
    expect(validate({ targets: [{ selector: "#x", action: "explode" }] })).toBe(false);
  });

  it("names every field once, each with a doc line", () => {
    for (const [k, f] of Object.entries(FIELDS)) expect(f.doc, k).toMatch(/\S/);
    expect(Object.keys(flatten(configSchema().properties))).not.toHaveLength(0);
  });
});

describe("resolve", () => {
  const LOCAL = "http://127.0.0.1:7700";
  const PROD = "https://changeling.io";

  it("gives Clax's defaults with no layers, each from 'default'", () => {
    const r = resolve({}, PROD);
    expect(r.config.presentation).toBe("floating");
    expect(r.config["attachments.screenshot"]).toBe(true);
    expect(new Set(Object.values(r.source))).toEqual(new Set(["default"]));
  });

  it("puts the Clax container over the site's defaults, and the viewer's cookie over both on a local origin", () => {
    const r = resolve({
      site: { presentation: "dialog", start: "note", attachments: { logs: true } },
      clax: { presentation: "drawer", "entry.key": "f" },
      you: { presentation: "window" },
    }, LOCAL);
    expect(r.config).toMatchObject({ presentation: "window", start: "note", "attachments.logs": true, "entry.key": "f" });
    expect(r.source).toMatchObject({ presentation: "you", start: "site", "attachments.logs": "site", "entry.key": "clax", enabled: "default" });
  });

  it("ignores the viewer's override on a production origin unless the Clax layer allows it", () => {
    const off = resolve({ clax: { presentation: "drawer" }, you: { presentation: "window" } }, PROD);
    expect(off.config.presentation).toBe("drawer");
    expect(off.source.presentation).toBe("clax");
    expect(off.ignored).toEqual([{ layer: "you", field: "presentation", reason: expect.stringMatching(/local and tailnet origins only/) }]);
    const on = resolve({ clax: { overrides: { viewer: "always" } }, you: { presentation: "window" } }, PROD);
    expect(on.config.presentation).toBe("window");
    const never = resolve({ clax: { overrides: { viewer: "never" } }, you: { presentation: "window" } }, LOCAL);
    expect(never.config.presentation).toBe("floating");
  });

  it("never lets the viewer's override change whether overrides apply", () => {
    const r = resolve({ clax: { overrides: { viewer: "local" } }, you: { overrides: { viewer: "always" } } }, LOCAL);
    expect(r.config["overrides.viewer"]).toBe("local");
    expect(r.ignored[0]).toMatchObject({ layer: "you", field: "overrides.viewer" });
  });

  it("reports values it leaves out, by layer, and keeps the layer below", () => {
    const r = resolve({ site: { presentation: "dialog", colour: "red" }, clax: { presentation: "popover", "entry.key": "ff" } }, PROD);
    expect(r.config.presentation).toBe("dialog");
    expect(r.ignored).toEqual([
      { layer: "site", field: "colour", reason: "no such field" },
      { layer: "clax", field: "presentation", reason: expect.stringMatching(/not one of/) },
      { layer: "clax", field: "entry.key", reason: expect.stringMatching(/at most 1/) },
    ]);
  });

  it("takes link strings for booleans", () => {
    expect(resolve({ you: { "attachments.logs": "on", annotate: "0" } }, LOCAL).config).toMatchObject({ "attachments.logs": true, annotate: false });
  });
});

describe("the viewer's override", () => {
  it("reads a ?clax.<field>= link, JSON for targets and theme", () => {
    expect(linkOverride("?a=1&clax.presentation=dialog&clax.theme=%7B%22--clax-bg%22%3A%22%23000%22%7D")).toEqual({ presentation: "dialog", theme: { "--clax-bg": "#000" } });
    expect(linkOverride("?x=1")).toBeUndefined();
  });

  it("round-trips through the cookie and clears", () => {
    writeOverride(document, { presentation: "sheet", "attachments.logs": true });
    expect(readOverride(document.cookie)).toEqual({ presentation: "sheet", "attachments.logs": true });
    writeOverride(document, null);
    expect(readOverride(document.cookie)).toBeUndefined();
    expect(readOverride("clax_override=%7Bbroken")).toBeUndefined();
  });

  it.each([
    ["http://localhost:5173", true], ["http://127.0.0.1:7700", true], ["https://phobos.tail556f5b.ts.net", true], ["http://app.localhost", true],
    ["https://changeling.io", false], ["https://evil.ts.net.example.com", false], ["not a url", false],
  ])("treats %s as a dev origin: %s", (o, dev) => {
    expect(isDevOrigin(o)).toBe(dev);
  });
});

describe("parseContainer", () => {
  it("takes a published container and refuses anything else", () => {
    expect(parseContainer({ id: "q1", version: 3, config: { presentation: "dialog" } })).toEqual({ id: "q1", version: 3, config: { presentation: "dialog" } });
    expect(() => parseContainer({ id: "q1", version: -1, config: {} })).toThrow(/version/);
    expect(() => parseContainer({ id: "", version: 1, config: {} })).toThrow(/id/);
    expect(() => parseContainer(null)).toThrow();
  });
});
