// The SDK's generated types (sdk/src/contract.gen.ts) are what the API
// contract (sdk/openapi.json, which crates/clax-server generates from the
// routes) generates, and the fakes that stand for the routes in the web
// tests answer what the contract allows.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import Ajv2020 from "ajv/dist/2020";
import { CONTRACT, GENERATED, generate, typeOf } from "./contract-types.mjs";
import { report } from "../shell/src/queues/fixtures";

const contract = JSON.parse(readFileSync(CONTRACT, "utf8"));

describe("the SDK's contract types", () => {
  it("are what the contract generates (run `just contract` after a route change)", () => {
    expect(readFileSync(GENERATED, "utf8")).toBe(generate(contract));
  });

  it("map the schemas schemars writes", () => {
    expect(typeOf({ type: ["string", "null"] })).toBe("string | null");
    expect(typeOf({ anyOf: [{ $ref: "#/components/schemas/A" }, { type: "null" }] })).toBe("A | null");
    expect(typeOf({ enum: ["a", "b"], type: "string" })).toBe('"a" | "b"');
    expect(typeOf({ const: 1, type: "integer" })).toBe("1");
    expect(typeOf({ type: "array", items: { enum: ["a", "b"] } })).toBe('("a" | "b")[]');
    expect(typeOf({ type: "object", additionalProperties: { type: "number" } })).toBe("Record<string, number>");
    expect(typeOf({ description: "Opaque JSON." })).toBe("unknown");
    expect(typeOf({ type: "object", properties: { a: { type: "string" }, b: { type: "integer" } }, required: ["a"] }))
      .toBe("{\n  a: string;\n  b?: number;\n}");
    expect(() => typeOf({ $ref: "#/$defs/A" })).toThrow(/outside the components/);
  });

  it("give every operation its §4.1 extensions", () => {
    for (const item of Object.values(contract.paths) as Record<string, Record<string, unknown>>[]) {
      for (const o of Object.values(item)) {
        expect(typeof o["x-clax-retry"]).toBe("boolean");
        expect(Array.isArray(o["x-clax-channel"])).toBe(true);
        expect("x-clax-rights" in o).toBe(true);
        expect(typeof o["x-clax-stream"]).toBe("boolean");
      }
    }
  });
});

describe("the web tests' report fixtures", () => {
  // The contract's integer formats (uint32, …) and `double` are schemars'
  // names, not Ajv's; they constrain nothing Ajv checks.
  const ajv = new Ajv2020({ allErrors: true, strict: true });
  for (const f of ["double", "uint8", "uint32", "uint64"]) ajv.addFormat(f, true);
  // The schemas live where OpenAPI keeps them, under `components`.
  ajv.addKeyword("components");
  ajv.addSchema({ $id: "contract", components: contract.components });
  const validate = (name: string) => ajv.getSchema(`contract#/components/schemas/${name}`)!;

  it("are reports as the routes answer them", () => {
    const check = validate("Report");
    for (const r of [report(1), report(2, { claim: { holder_name: "agent", expires_at: "2026-10-29T08:00:00Z" } })]) {
      expect(check(r), JSON.stringify(check.errors)).toBe(true);
    }
  });

  it("refuse what the routes never answer", () => {
    const check = validate("Report");
    const { anchor: _, ...noAnchor } = report(1);
    expect(check(noAnchor)).toBe(false);
    expect(check({ ...report(1), state: "lost" })).toBe(false);
  });
});
