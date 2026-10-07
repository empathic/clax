// Validates the Toolpath documents Clax's renderer writes against
// Toolpath's published schema (spec 2026-10-06-toolpath-audit-design §15).
//
// The Rust tests (`cargo test -p clax-core toolpath`) write the golden
// documents byte for byte: one export per redaction option set, covering
// every recorded kind; the sealed journal segment; and unrenderable steps.
// This test validates each with Ajv (draft 2020-12, formats asserted)
// against the schema vendored from Toolpath at the commit named in its
// SOURCE file.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import Ajv2020 from "ajv/dist/2020";
import addFormats from "ajv-formats";

// The unit tests run from web/.
const dir = join(process.cwd(), "../crates/clax-core/tests/toolpath/");
const read = (path: string): unknown => JSON.parse(readFileSync(dir + path, "utf8"));

// Strict, except `strictRequired`: the schema's own `artifactChange` names
// `raw` and `structural` as required inside an `anyOf` branch, which Ajv's
// strict mode refuses.
function validator() {
  const ajv = new Ajv2020({ allErrors: true, strict: true, strictRequired: false });
  addFormats(ajv);
  return ajv.compile(read("schema/toolpath.schema.json") as object);
}

const goldens = readdirSync(dir + "expected").filter((f) => f.endsWith(".path.json")).sort();

describe("Clax's Toolpath documents", () => {
  const validate = validator();

  it("include every golden the renderer writes", () => {
    expect(goldens).toEqual([
      "export.all.path.json",
      "export.no-names.path.json",
      "export.no-paths.path.json",
      "export.no-text.path.json",
      "export.none.path.json",
      "segment.sealed.path.json",
      "unrenderable.path.json",
    ]);
  });

  for (const name of goldens) {
    it(`${name} validates against the Toolpath schema`, () => {
      const ok = validate(read("expected/" + name));
      expect(validate.errors ?? [], name).toEqual([]);
      expect(ok).toBe(true);
    });
  }

  it("rejects documents that break the schema", () => {
    type Doc = {
      paths: { steps: { step: Record<string, unknown>; change: Record<string, { structural: Record<string, unknown> }> }[] }[];
    };
    const breakages: ((d: Doc) => void)[] = [
      (d) => (d.paths[0].steps[0].step.actor = "agent:a b"),
      (d) => (d.paths[0].steps[0].step.timestamp = "yesterday"),
      (d) => {
        const change = Object.values(d.paths[0].steps[0].change)[0];
        delete change.structural.type;
      },
    ];
    for (const [i, breakage] of breakages.entries()) {
      const doc = read("expected/export.none.path.json") as Doc;
      breakage(doc);
      expect(validate(doc), `breakage ${i}`).toBe(false);
    }
  });
});
