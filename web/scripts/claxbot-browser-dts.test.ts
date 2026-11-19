// The browser declarations ClaxBot's models are shown (crates/clax-bot's
// api::dts, pinned by its goldens src/api.browser-*.d.ts.golden, written
// from the one table in src/browser/layer.rs) compile with TypeScript,
// and with them the scripts of src/browser.examples.ts, written as
// Playwright's documentation writes them (spec
// 2026-11-19-claxbot-browser §6.11): a member of the table renamed or
// re-typed fails here.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import ts from "typescript";

// The unit tests run from web/.
const dir = join(process.cwd(), "../crates/clax-bot/src/");
const read = (name: string) => readFileSync(dir + name, "utf8");

/** The diagnostics of `files` (name to text) compiled together, strict, with ES2022's library alone. */
function diagnostics(files: Record<string, string>): string[] {
  const options: ts.CompilerOptions = { strict: true, noEmit: true, target: ts.ScriptTarget.ES2022, lib: ["lib.es2022.d.ts"], types: [] };
  const host = ts.createCompilerHost(options);
  const readHost = host.readFile.bind(host);
  host.readFile = (name) => (name in files ? files[name] : readHost(name));
  host.fileExists = ((exists) => (name: string) => name in files || exists(name))(host.fileExists.bind(host));
  const program = ts.createProgram(Object.keys(files), options, host);
  return ts.getPreEmitDiagnostics(program).map((d) => ts.flattenDiagnosticMessageText(d.messageText, "\n"));
}

describe("the browser declarations", () => {
  for (const shape of ["artifact", "content", "url-artifact-content"]) {
    it(`compile alone (${shape})`, () => {
      expect(diagnostics({ "/clax.d.ts": read(`api.browser-${shape}.d.ts.golden`) })).toEqual([]);
    });
  }

  it("compile with scripts written as Playwright's docs write them", () => {
    expect(
      diagnostics({
        "/clax.d.ts": read("api.browser-url-artifact-content.d.ts.golden"),
        "/examples.ts": read("browser.examples.ts"),
      }),
    ).toEqual([]);
  });

  it("refuse what is not declared", () => {
    const wrong = "export async function f() { const page = await clax.browser.open({ url: 'x' }); await page.waitForTimeout(10); }";
    const errors = diagnostics({ "/clax.d.ts": read("api.browser-content.d.ts.golden"), "/wrong.ts": wrong });
    expect(errors.some((e) => e.includes("waitForTimeout"))).toBe(true);
    expect(errors.some((e) => e.includes("url"))).toBe(true);
  });
});
