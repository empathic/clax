// The `sample` capability (contract 0.2.61 sample.d.ts), page side, in the
// bridge's lazy `sample` part (parts/sample.ts): argument
// checks, image preparation, and one call's lifecycle over the shell relay
// (web/shell/src/caps/sample.ts): the request leaves on the next microtask,
// `onText` gets the whole text so far a few times a second, page tools run
// here and their results go back, an abort rejects `cancelled` with the text
// the page may keep. Every failure is one rejected {code, message, text?}.
import type { Rpc } from "../rpc";

export const MAX_PROMPT_BYTES = 65_536;
export const TEXT_INTERVAL_MS = 100;
export const TOOL_TIMEOUT_MS = 150_000;
export const MAX_TOOL_RESULT_BYTES = 32_768;
export const MAX_TOOL_DESCRIPTION_BYTES = 1024;
export const MAX_TOOL_SCHEMA_BYTES = 4096;
export const MAX_TOOLS = 16;
export const TARGET_PIXELS = 1_200_000;
const MAX_GC_MS = 86_400_000;
const IMAGE_TYPES = ["image/jpeg", "image/png", "image/webp", "image/gif"];
const MAX_INPUT_IMAGE_BYTES = 20_000_000;
const MAX_SIDE = 10_000;
const MAX_IMAGE_PIXELS = 64_000_000;
const TOOL_NAME = /^[A-Za-z0-9_-]{1,128}$/;
const TIERS = new Set(["default", "complex", "quick"]);
const KNOWN_OPTIONS = new Set(["onText", "signal", "tools", "images", "modelTier", "cache"]);

export type SampleError = { code: string; message: string; text?: string };
type Tool = { name: string; description: string; inputSchema?: Record<string, unknown>; execute: (input: Record<string, unknown>, ctx: { signal: AbortSignal }) => unknown };
type WireRequest = {
  input: string | { role: string; content: string }[];
  verb: "text" | "json";
  model_tier: string;
  tools: { name: string; description: string; input_schema?: unknown }[];
  images: { media_type: string; data: string }[];
  cache: boolean | { gc_time_ms?: number; refresh?: boolean };
};
type Checked = { req: WireRequest; onText?: (u: { text: string; delta: string }) => unknown; signal?: AbortSignal; tools: Tool[]; images: Blob[] };
type Frame = { call: string; event: "start" | "text" | "tool_call" | "done" | "error"; data: Record<string, unknown> };

const bytes = (s: string) => new TextEncoder().encode(s).length;
const err = (code: string, message: string, text?: string): SampleError => (text ? { code, message, text } : { code, message });
const bad = (message: string) => err("invalid_request", message);

function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}

let warnedUnknown = false;

/** Checks a call's arguments; a SampleError says what to fix. */
export function validate(input: unknown, options: unknown, verb: "text" | "json"): Checked | SampleError {
  if (options !== undefined && !isPlainObject(options)) {
    if (options instanceof AbortController) return bad("options must be a plain object: pass { signal: controller.signal }");
    if (options instanceof Blob) return bad("options must be a plain object: pass images as { images: file }");
    return bad("options must be a plain object");
  }
  const o = (options ?? {}) as Record<string, unknown>;
  const unknown = Object.keys(o).filter(k => !KNOWN_OPTIONS.has(k));
  if (unknown.length && !warnedUnknown) { warnedUnknown = true; console.warn(`sample(): ignoring unknown options ${unknown.join(", ")}`); }

  let wireInput: WireRequest["input"];
  if (typeof input === "string") {
    if (!input.trim()) return bad("input is empty");
    if (bytes(input) > MAX_PROMPT_BYTES) return err("prompt_too_large", `the input is ${bytes(input)} bytes; the limit is ${MAX_PROMPT_BYTES}`);
    wireInput = input;
  } else if (Array.isArray(input)) {
    if (!input.length) return bad("input has no turns");
    const turns: { role: string; content: string }[] = [];
    for (const [i, t] of input.entries()) {
      if (!isPlainObject(t) || (t.role !== "user" && t.role !== "assistant")) return bad(`input[${i}].role must be "user" or "assistant"`);
      if (typeof t.content !== "string" || !t.content) return bad(`input[${i}].content must be a non-empty string`);
      turns.push({ role: t.role, content: t.content });
    }
    if (turns[0].role !== "user" || turns.at(-1)!.role !== "user") return bad("input turns must start and end with a user turn");
    const total = turns.reduce((n, t) => n + bytes(t.content), 0);
    if (total > MAX_PROMPT_BYTES) return err("prompt_too_large", `the input is ${total} bytes; the limit is ${MAX_PROMPT_BYTES}`);
    wireInput = turns;
  } else {
    return bad("input is a prompt string or an array of {role, content} turns (the prompt goes first; there is no {prompt} object form)");
  }

  if (o.onText !== undefined && typeof o.onText !== "function") return bad("onText must be a function");
  if (o.signal !== undefined && !(o.signal instanceof AbortSignal)) return bad("signal must be an AbortSignal: pass controller.signal, not the controller");
  const tier = o.modelTier ?? "default";
  if (typeof tier !== "string" || !TIERS.has(tier)) return bad(`modelTier is "default", "complex" or "quick", not ${JSON.stringify(tier)}`);

  const tools: Tool[] = [];
  if (o.tools !== undefined) {
    if (!Array.isArray(o.tools)) return bad("tools must be an array of tools");
    if (o.tools.length > MAX_TOOLS) return bad(`at most ${MAX_TOOLS} tools per call`);
    for (const [i, t] of o.tools.entries()) {
      if (!isPlainObject(t)) return bad(`tools[${i}] must be a plain object`);
      if (typeof t.name !== "string" || !TOOL_NAME.test(t.name)) return bad(`tools[${i}].name must be 1-128 of A-Z a-z 0-9 _ -`);
      if (tools.some(x => x.name === t.name)) return bad(`tools[${i}].name '${t.name}' is used twice`);
      if (typeof t.description !== "string" || !t.description.trim() || bytes(t.description) > MAX_TOOL_DESCRIPTION_BYTES) return bad(`tools[${i}].description must be 1-${MAX_TOOL_DESCRIPTION_BYTES} bytes`);
      if (t.inputSchema !== undefined && (!isPlainObject(t.inputSchema) || t.inputSchema.type !== "object" || bytes(JSON.stringify(t.inputSchema)) > MAX_TOOL_SCHEMA_BYTES)) {
        return bad(`tools[${i}].inputSchema must be a JSON Schema object of type "object", at most ${MAX_TOOL_SCHEMA_BYTES} bytes`);
      }
      if (typeof t.execute !== "function") return bad(`tools[${i}].execute must be a function`);
      tools.push({ name: t.name, description: t.description, inputSchema: t.inputSchema as Record<string, unknown> | undefined, execute: t.execute as Tool["execute"] });
    }
  }

  let cache: WireRequest["cache"];
  if (o.cache === undefined) cache = tools.length === 0;
  else if (o.cache === true || o.cache === false) cache = o.cache;
  else if (isPlainObject(o.cache)) {
    const { gcTime, refresh } = o.cache;
    if (gcTime !== undefined && (typeof gcTime !== "number" || !Number.isFinite(gcTime) || gcTime <= 0)) return bad("cache.gcTime must be a number of milliseconds greater than zero");
    if (refresh !== undefined && typeof refresh !== "boolean") return bad("cache.refresh must be true or false");
    cache = { ...(gcTime === undefined ? {} : { gc_time_ms: Math.min(gcTime, MAX_GC_MS) }), ...(refresh === undefined ? {} : { refresh }) };
  } else return bad("cache must be true, false, or {gcTime?, refresh?}");
  if (tools.length && cache !== false) return bad("a call with tools is never cached: omit cache or pass false");

  let images: Blob[] = [];
  if (o.images !== undefined) {
    const list = o.images instanceof Blob ? [o.images] : Array.isArray(o.images) ? o.images : typeof FileList !== "undefined" && o.images instanceof FileList ? [...o.images] : null;
    if (!list || !list.every(b => b instanceof Blob)) return bad("images must be a Blob, a File, an array of them, or a FileList");
    images = list as Blob[];
  }

  return {
    req: {
      input: wireInput, verb, model_tier: tier, cache, images: [],
      tools: tools.map(t => (t.inputSchema === undefined ? { name: t.name, description: t.description } : { name: t.name, description: t.description, input_schema: t.inputSchema })),
    },
    onText: o.onText as Checked["onText"], signal: o.signal as AbortSignal | undefined, tools, images,
  };
}

async function base64(blob: Blob): Promise<string> {
  const buf = new Uint8Array(await blob.arrayBuffer());
  let s = "";
  for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode(...buf.subarray(i, i + 0x8000));
  return btoa(s);
}

/** Downsizes each image to about 1.2 megapixels, applies orientation, keeps an
 * animation's first frame, and drops metadata (a canvas re-encode). */
async function prepareImages(list: Blob[]): Promise<WireRequest["images"]> {
  const out: WireRequest["images"] = [];
  for (const b of list) {
    if (!IMAGE_TYPES.includes(b.type)) throw err("image_rejected", `images are JPEG, PNG, WebP or GIF, not '${b.type || "unknown"}'`);
    if (b.size > MAX_INPUT_IMAGE_BYTES) throw err("image_rejected", "an image is over 20 MB");
    let bmp: ImageBitmap;
    try { bmp = await createImageBitmap(b, { imageOrientation: "from-image" }); } catch { throw err("image_rejected", "an image could not be decoded"); }
    if (bmp.width > MAX_SIDE || bmp.height > MAX_SIDE || bmp.width * bmp.height > MAX_IMAGE_PIXELS) throw err("image_rejected", "an image is over 10,000 px a side or 64 megapixels");
    const scale = Math.min(1, Math.sqrt(TARGET_PIXELS / (bmp.width * bmp.height)));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bmp.width * scale));
    canvas.height = Math.max(1, Math.round(bmp.height * scale));
    canvas.getContext("2d")!.drawImage(bmp, 0, 0, canvas.width, canvas.height);
    const type = b.type === "image/png" || b.type === "image/gif" ? "image/png" : "image/jpeg";
    const encoded = await new Promise<Blob | null>(r => canvas.toBlob(r, type, 0.9));
    if (!encoded) throw err("image_rejected", "an image could not be re-encoded");
    out.push({ media_type: type, data: await base64(encoded) });
  }
  return out;
}

export function makeSample(rpc: Pick<Rpc, "call" | "on">): Record<string, (...args: never[]) => unknown> {
  let seq = 0;

  function run(verb: "text" | "json", input: unknown, options: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const checked = validate(input, options, verb);
      if ("code" in checked) { reject(checked); return; }
      const v = checked;
      const call = `s${++seq}`;
      const tools = new Map(v.tools.map(t => [t.name, t]));
      const running = new Set<AbortController>();
      let text = "";
      let shown = "";
      let settled = false;
      let started = false;
      let timer: ReturnType<typeof setTimeout> | null = null;

      const kept = () => (v.onText ? shown : text) || undefined;
      const flushText = () => {
        timer = null;
        if (settled || !v.onText || text === shown || !text.trim()) return;
        const delta = text.slice(shown.length);
        shown = text;
        try {
          const r = v.onText({ text, delta }) as { then?: unknown; catch?: (f: (e: unknown) => void) => void } | undefined;
          if (r && typeof r.then === "function" && typeof r.catch === "function") r.catch(e => console.error(e));
        } catch (e) { console.error(e); }
      };
      const finish = (ok: boolean, value: unknown) => {
        if (settled) return;
        if (ok) flushText();
        settled = true;
        if (timer) clearTimeout(timer);
        off();
        v.signal?.removeEventListener("abort", onAbort);
        for (const c of running) c.abort();
        if (ok) resolve(value); else reject(value);
      };
      const onAbort = () => {
        if (started) void rpc.call("sample", "cancel", [call]).catch(() => {});
        finish(false, err("cancelled", "the call's signal aborted", kept()));
      };

      async function runTool(tc: { id: string; name: string; input: unknown }) {
        const ctl = new AbortController();
        running.add(ctl);
        const t = setTimeout(() => ctl.abort(), TOOL_TIMEOUT_MS);
        let content = "";
        let isError = false;
        try {
          const tool = tools.get(tc.name);
          if (!tool) throw new Error(`no tool named ${tc.name}`);
          const arg = isPlainObject(tc.input) ? tc.input : {};
          const out = await tool.execute(arg, { signal: ctl.signal });
          content = typeof out === "string" ? out : JSON.stringify(out ?? null);
          if (bytes(content) > MAX_TOOL_RESULT_BYTES) { content = `Error: the tool's result is over ${MAX_TOOL_RESULT_BYTES} bytes`; isError = true; }
        } catch (e) {
          content = `Error: ${e instanceof Error ? e.message : String(e)}`;
          isError = true;
        } finally {
          clearTimeout(t);
          running.delete(ctl);
        }
        if (!settled) void rpc.call("sample", "toolResult", [call, tc.id, content, isError]).catch(() => {});
      }

      const off = rpc.on("sample", "frame", d => {
        const f = d as Frame;
        if (f.call !== call || settled) return;
        switch (f.event) {
          case "text":
            text += String(f.data.delta ?? "");
            if (!timer) timer = setTimeout(flushText, TEXT_INTERVAL_MS);
            return;
          case "tool_call":
            void runTool(f.data as { id: string; name: string; input: unknown });
            return;
          case "done":
            text = String(f.data.text ?? text);
            finish(true, verb === "json" ? f.data.value : { text, truncated: f.data.truncated === true, modelTierApplied: f.data.model_tier_applied });
            return;
          case "error": {
            const code = String(f.data.code);
            const keep = code === "refused" ? undefined : code === "invalid_json" ? text || undefined : kept();
            finish(false, err(code, String(f.data.message), keep));
            return;
          }
          default:
            return;
        }
      });

      if (v.signal?.aborted) { finish(false, err("cancelled", "the signal was already aborted")); return; }
      v.signal?.addEventListener("abort", onAbort, { once: true });
      queueMicrotask(async () => {
        if (settled) return;
        let images: WireRequest["images"] = [];
        try { images = await prepareImages(v.images); } catch (e) { finish(false, e); return; }
        if (settled) return;
        started = true;
        rpc.call("sample", "run", [call, { ...v.req, images }]).catch((e: { code?: unknown; message?: unknown }) =>
          finish(false, err(String(e?.code ?? "upstream_error"), String(e?.message ?? e), kept())));
      });
    });
  }

  return {
    call: (input: unknown, options?: unknown) => run("text", input, options),
    json: (input: unknown, options?: unknown) => run("json", input, options),
    limits: () => rpc.call("sample", "limits", []),
  } as unknown as Record<string, (...args: never[]) => unknown>;
}

/** The members of the `sample` namespace besides the call itself (checked
 * against sample.d.ts in bridge/test/capabilities.test.ts). */
export const SAMPLE_METHODS = ["json", "limits"] as const;

/** The `sample` namespace `claude.use("sample")` resolves: a frozen function
 * (the call) carrying `json` and `limits`. */
export function sampleNamespace(rpc: Pick<Rpc, "call" | "on">): unknown {
  const m = makeSample(rpc) as Record<string, (...args: unknown[]) => unknown>;
  const ns = Object.assign((...args: unknown[]) => m.call(...args), { json: m.json, limits: m.limits });
  return Object.freeze(ns);
}
