import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
// @ts-expect-error plain .mjs without declarations
import { keepDeclarations, thirdPartyNotices } from "./build-sdk.mjs";

let root = "";
beforeEach(() => { root = mkdtempSync(join(tmpdir(), "clax-build-sdk-")); });
afterEach(() => { rmSync(root, { recursive: true, force: true }); });

/** An npm package under `dir` (a node_modules path relative to the scratch
 * web/), with a license file of the given name, or none. */
function pkg(dir: string, name: string, license: string | null): string {
  const at = join(root, dir);
  mkdirSync(at, { recursive: true });
  writeFileSync(join(at, "package.json"), JSON.stringify({ name, version: "1.2.3", license: "MIT" }));
  if (license) writeFileSync(join(at, license), `${name}'s license text\n`);
  return join(at, "index.js");
}
const src = (file: string) => { const at = join(root, file); mkdirSync(dirname(at), { recursive: true }); return at; };

describe("thirdPartyNotices", () => {
  it("passes Clax files of web/bridge, web/overlay and web/sdk, with a query, and virtual modules", () => {
    const ids = [src("bridge/src/anchor.ts"), src("overlay/src/Overlay.svelte") + "?svelte&type=style&lang.css", src("sdk/src/index.ts"), "\0commonjsHelpers.js"];
    expect(thirdPartyNotices(ids, root)).toBe("The packages @empathic/clax-sdk bundles, and their licenses.\n\n");
  });

  it("refuses a Clax file outside them", () => {
    for (const file of ["shell/src/api.ts", "extension/src/overlay.ts", "sdkx/src/a.ts"]) {
      expect(() => thirdPartyNotices([src("sdk/src/index.ts"), src(file)], root)).toThrow(`the package bundles ${join(root, file)}, outside web/bridge, web/overlay, web/sdk, whose license LICENSE does not give the package`);
    }
  });

  it("carries the license of each package, scoped or not, from its root however deep the module, in the order of their paths", () => {
    const scoped = pkg("node_modules/@scope/name", "@scope/name", "LICENSE");
    const nested = pkg("node_modules/.pnpm/plain@1.2.3/node_modules/plain", "plain", "LICENSE.md");
    const notices = thirdPartyNotices([join(dirname(scoped), "dist/deep/a.js"), scoped, join(dirname(nested), "lib/b.js")], root);
    expect(notices).toBe(`The packages @empathic/clax-sdk bundles, and their licenses.

plain 1.2.3 (MIT)

plain's license text

${"-".repeat(78)}

@scope/name 1.2.3 (MIT)

@scope/name's license text
`);
  });

  it("fails on a package with no license file", () => {
    const bare = pkg("node_modules/bare", "bare", null);
    expect(() => thirdPartyNotices([bare], root)).toThrow("bare has no license file to carry");
  });
});

describe("keepDeclarations", () => {
  const entries = {
    "sdk/src/index.d.ts": `export { ReporterClient } from "./reporter";\nexport type * from "./types";\n`,
    "sdk/src/reporter.d.ts": `import type { Report } from "./types";\nexport declare class ReporterClient { r: Report; }\n`,
    "sdk/src/types.d.ts": "export interface Report { id: string }\n",
    "sdk/src/service.d.ts": "export declare class ServiceClient {}\n",
    "overlay/src/main.d.ts": `export type { OverlayOptions } from "./options";\nexport declare function overlay(o: import("./options").OverlayOptions): void;\n`,
    "overlay/src/options.d.ts": `import type { ReporterAuth } from "../../sdk/src/reporter";\nexport interface OverlayOptions { auth: ReporterAuth }\n`,
  };

  it("keeps what the entries reach, with `.js` specifiers", () => {
    const kept = keepDeclarations(new Map(Object.entries({ ...entries, "shell/src/api.d.ts": "export {};\n" })), root);
    expect([...kept.keys()].sort()).toEqual(Object.keys(entries).sort());
    expect(kept.get("sdk/src/index.d.ts")).toBe(`export { ReporterClient } from "./reporter.js";\nexport type * from "./types.js";\n`);
    expect(kept.get("overlay/src/main.d.ts")).toContain(`import("./options.js")`);
    expect(kept.get("overlay/src/options.d.ts")).toContain(`from "../../sdk/src/reporter.js"`);
  });

  it("refuses a declaration of a source outside web/bridge, web/overlay and web/sdk", () => {
    const emitted = new Map(Object.entries({ ...entries, "sdk/src/types.d.ts": `export type { Report } from "../../shell/src/api";\n`, "shell/src/api.d.ts": "export interface Report { id: string }\n" }));
    expect(() => keepDeclarations(emitted, root)).toThrow(`the package bundles ${join(root, "shell/src/api.d.ts")}, outside web/bridge, web/overlay, web/sdk, whose license LICENSE does not give the package`);
  });
});
