// The daemon's `/api/stream` as the stream hub sees it, for unit tests:
// streams the test writes SSE to, and subscription requests answered at
// once (`auto`) or by the test.
const enc = new TextEncoder();

export type Conn = { headers: Record<string, string>; aborted: boolean; push(text: string): void; end(): void };
export type Post = { url: string; body: { subscribe: string[]; unsubscribe: string[] }; answer(status: number, json: unknown): void };

/** The daemon as the hub sees it: streams it can write to, and the
 * subscription requests, answered at once (`auto`) or by the test. */
export class Net {
  conns: Conn[] = [];
  posts: Post[] = [];
  /** The topics each stream ID holds, for automatic answers. */
  held = new Map<string, Set<string>>();
  auto = true;
  refuse = new Map<string, [number, string]>();
  fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = String(input);
    if (url === "/api/token") return new Response("{}");
    if (url === "/api/stream") {
      let ctrl!: ReadableStreamDefaultController<Uint8Array>;
      const body = new ReadableStream<Uint8Array>({ start(c) { ctrl = c; } });
      const c: Conn = { headers: (init.headers ?? {}) as Record<string, string>, aborted: false, push: t => ctrl.enqueue(enc.encode(t)), end: () => { c.aborted = true; ctrl.close(); } };
      init.signal?.addEventListener("abort", () => { c.aborted = true; try { ctrl.error(new DOMException("aborted", "AbortError")); } catch { /* closed */ } });
      this.conns.push(c);
      return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
    }
    const id = url.slice("/api/stream/".length);
    const body = JSON.parse(String(init.body));
    return new Promise<Response>(resolve => {
      const p: Post = { url, body, answer: (status, json) => resolve(new Response(JSON.stringify(json), { status })) };
      this.posts.push(p);
      if (!this.auto) return;
      for (const t of body.subscribe) {
        const r = this.refuse.get(t);
        if (r) { p.answer(r[0], { error: { code: r[1] } }); return; }
      }
      const held = this.held.get(id) ?? new Set<string>();
      for (const t of body.unsubscribe) held.delete(t);
      for (const t of body.subscribe) held.add(t);
      this.held.set(id, held);
      p.answer(200, { seq: 0, topics: [...held] });
    });
  }) as typeof fetch;
  get open(): Conn[] { return this.conns.filter(c => !c.aborted); }
  ready(c: Conn, stream: string, resumed = false, topics: string[] = []) {
    c.push(`event: ready\ndata: ${JSON.stringify({ stream, seq: 0, resumed, topics })}\n\n`);
  }
  event(c: Conn, stream: string, seq: number, topic: string, name: string, data: object = {}) {
    c.push(`event: ${name}\ndata: ${JSON.stringify({ topic, ...data })}\nid: ${stream}:${seq}\n\n`);
  }
}

