// Reads a `text/event-stream` response body (the sample route's), yielding one
// {event, data} per event; comments (keep-alives) are skipped.

export type SseMessage = { event: string; data: string };

export function parseBlock(block: string): SseMessage | null {
  let event = "message";
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith(":")) continue;
    if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
  }
  return data.length ? { event, data: data.join("\n") } : null;
}

export async function* readSse(body: ReadableStream<Uint8Array>): AsyncGenerator<SseMessage> {
  const reader = body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buf = (buf + dec.decode(value, { stream: true })).replace(/\r\n/g, "\n");
      let i: number;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        const m = parseBlock(buf.slice(0, i));
        buf = buf.slice(i + 2);
        if (m) yield m;
      }
    }
  } finally {
    reader.releaseLock();
  }
}
