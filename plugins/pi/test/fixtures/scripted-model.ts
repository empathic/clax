// A Pi extension that registers a model provider, `scripted`, whose one
// model (`script`) calls the tools `CLAX_SCRIPT` names, in order, one per
// turn (a JSON array of [tool name, arguments]), then answers "done". It
// stands in for a real model so a test can drive the real Pi (`pi --mode
// rpc`) through the Clax extension's tools without a network or a key.
import { createAssistantMessageEventStream, type AssistantMessage, type Context, type Model } from "@mariozechner/pi-ai";
import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

type Call = [string, Record<string, unknown>];

const script: Call[] = JSON.parse(process.env.CLAX_SCRIPT ?? "[]");

/** The tool results since the last user message: how far into the script
 * this run is. */
function done(context: Context): number {
  let n = 0;
  for (const m of context.messages) {
    if (m.role === "user") n = 0;
    else if (m.role === "toolResult") n++;
  }
  return n;
}

function stream(model: Model<any>, context: Context) {
  const s = createAssistantMessageEventStream();
  const out: AssistantMessage = {
    role: "assistant",
    content: [],
    api: model.api,
    provider: model.provider,
    model: model.id,
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
    stopReason: "stop",
    timestamp: Date.now(),
  };
  queueMicrotask(() => {
    s.push({ type: "start", partial: out });
    const i = done(context);
    const next = script[i];
    if (next) {
      const toolCall = { type: "toolCall" as const, id: `script-${i}`, name: next[0], arguments: next[1] };
      out.content.push(toolCall);
      out.stopReason = "toolUse";
      s.push({ type: "toolcall_start", contentIndex: 0, partial: out });
      s.push({ type: "toolcall_end", contentIndex: 0, toolCall, partial: out });
      s.push({ type: "done", reason: "toolUse", message: out });
    } else {
      out.content.push({ type: "text", text: "done" });
      s.push({ type: "text_start", contentIndex: 0, partial: out });
      s.push({ type: "text_end", contentIndex: 0, content: "done", partial: out });
      s.push({ type: "done", reason: "stop", message: out });
    }
    s.end();
  });
  return s;
}

export default function scriptedModel(pi: ExtensionAPI): void {
  pi.registerProvider("scripted", {
    baseUrl: "http://127.0.0.1:9",
    apiKey: "none",
    api: "scripted",
    models: [{
      id: "script", name: "Script", reasoning: false, input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100_000, maxTokens: 1_000,
    }],
    streamSimple: stream as never,
  });
}
