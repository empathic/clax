// A scripted stand-in for the Anthropic Messages API, for the browser
// tests that run ClaxBot on a real daemon (claxbot-daemon.spec.ts): on a
// loopback port the system picks, `POST /v1/messages` answers each request
// with the next scripted turn as the streaming format's server-sent events
// (a 400 once the script is spent), and `GET /v1/models` lists two models.
// No model is called; every request body is kept.
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

type Block = { type: "text"; text: string } | { type: "tool_use"; id: string; name: string; input: unknown };
/** One scripted assistant turn. */
export type Turn = { blocks: Block[]; stop: "end_turn" | "tool_use" };

/** A turn that narrates, then runs a script. */
export const scriptTurn = (id: string, title: string, code: string, narration = `Running ${title}.`): Turn => ({
  blocks: [{ type: "text", text: narration }, { type: "tool_use", id, name: "run_script", input: { title, code } }],
  stop: "tool_use",
});
/** A turn that finishes with `summary`. */
export const finishTurn = (summary: string): Turn => ({
  blocks: [{ type: "tool_use", id: "tu_finish", name: "finish", input: { summary } }],
  stop: "tool_use",
});

function sse(t: Turn): string {
  const out: string[] = [];
  const ev = (name: string, data: unknown) => out.push(`event: ${name}\ndata: ${JSON.stringify(data)}\n\n`);
  ev("message_start", { type: "message_start", message: { id: "msg_fake", model: "claude-sonnet-5-5", usage: { input_tokens: 1000, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, output_tokens: 1 } } });
  t.blocks.forEach((b, i) => {
    if (b.type === "text") {
      ev("content_block_start", { type: "content_block_start", index: i, content_block: { type: "text", text: "" } });
      ev("content_block_delta", { type: "content_block_delta", index: i, delta: { type: "text_delta", text: b.text } });
    } else {
      ev("content_block_start", { type: "content_block_start", index: i, content_block: { type: "tool_use", id: b.id, name: b.name, input: {} } });
      ev("content_block_delta", { type: "content_block_delta", index: i, delta: { type: "input_json_delta", partial_json: JSON.stringify(b.input) } });
    }
    ev("content_block_stop", { type: "content_block_stop", index: i });
  });
  ev("message_delta", { type: "message_delta", delta: { stop_reason: t.stop }, usage: { output_tokens: 200 } });
  ev("message_stop", { type: "message_stop" });
  return out.join("");
}

const MODELS = { data: [
  { type: "model", id: "claude-sonnet-5-5", display_name: "Claude Sonnet 5.5", created_at: "2026-09-01T00:00:00Z" },
  { type: "model", id: "claude-opus-5-5", display_name: "Claude Opus 5.5", created_at: "2026-09-01T00:00:00Z" },
], has_more: false, first_id: "claude-sonnet-5-5", last_id: "claude-opus-5-5" };

/** Starts the fake API with `turns`; more can be added with `push`. */
export async function fakeMessages(turns: Turn[] = []) {
  const queue = [...turns];
  const requests: unknown[] = [];
  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    const chunks: Buffer[] = [];
    req.on("data", c => chunks.push(c as Buffer));
    req.on("end", () => {
      if (req.method === "GET" && req.url?.startsWith("/v1/models")) {
        res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(MODELS));
        return;
      }
      if (req.method === "POST" && req.url?.startsWith("/v1/messages")) {
        try { requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8"))); } catch { requests.push(null); }
        const t = queue.shift();
        if (!t) {
          res.writeHead(400, { "content-type": "application/json" }).end(JSON.stringify({ type: "error", error: { type: "invalid_request_error", message: "no scripted turn left" } }));
          return;
        }
        res.writeHead(200, { "content-type": "text/event-stream" }).end(sse(t));
        return;
      }
      res.writeHead(404).end();
    });
  });
  await new Promise<void>(r => server.listen(0, "127.0.0.1", r));
  const { port } = server.address() as AddressInfo;
  return {
    base: `http://127.0.0.1:${port}`,
    requests,
    push: (t: Turn) => queue.push(t),
    left: () => queue.length,
    stop: () => new Promise<void>(r => server.close(() => r())),
  };
}
