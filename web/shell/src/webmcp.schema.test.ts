import { describe, it, expect, afterEach } from "vitest";
import type { Thread } from "./threads";
import { type WebMcpView, registerWebMcp } from "./webmcp";
import schemas from "./webmcp.schemas.json";

// webmcp.ts imports webmcp.schemas.json directly for each tool's
// `inputSchema`, so there is nothing here to hand-duplicate; this test
// guards the wiring (every tool name has a schema, every registered
// `inputSchema` is exactly that file's entry), not a copy of the values.
// The file itself is checked against the schemars-derived Rust types in
// crates/clax-mcp/src/webmcp_schema.rs.

const view: WebMcpView = {
  id: "a1",
  state: { get: () => ({ data: { artifact: { id: "a1", title: "Plan", updated_at: "u", current_version: 2 } as never }, threads: [] as Thread[] }) },
};

type Registered = { name: string; inputSchema: unknown };
afterEach(() => { delete (document as never as Record<string, unknown>).modelContext; });

describe("webmcp tool schemas", () => {
  it("registers each tool with exactly its committed JSON Schema, and nothing uncommitted", () => {
    const registered: Registered[] = [];
    Object.defineProperty(document, "modelContext", { value: { registerTool: (t: Registered) => registered.push(t) }, configurable: true });
    registerWebMcp(window, view);

    expect(registered.map(t => t.name).sort()).toEqual(Object.keys(schemas).sort());
    for (const tool of registered) {
      expect(tool.inputSchema).toEqual((schemas as Record<string, unknown>)[tool.name]);
    }
  });
});
