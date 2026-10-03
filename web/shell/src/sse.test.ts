import { describe, expect, it } from "vitest";
import { parseBlock, readSse } from "./sse";

function stream(chunks: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  return new ReadableStream({ start(c) { for (const s of chunks) c.enqueue(enc.encode(s)); c.close(); } });
}

describe("readSse", () => {
  it("yields whole events across chunk boundaries and skips comments", async () => {
    const got = [];
    for await (const e of readSse(stream(["event: start\nda", "ta: {\"a\":1}\n", "\n: keep-alive\n\nevent: text\r\ndata: x\r\n\r\n"]))) got.push(e);
    expect(got).toEqual([{ event: "start", data: "{\"a\":1}" }, { event: "text", data: "x" }]);
  });

  it("parses a block with the default event name", () => {
    expect(parseBlock("data: a\ndata: b")).toEqual({ event: "message", data: "a\nb" });
    expect(parseBlock(": only a comment")).toBeNull();
  });
});
