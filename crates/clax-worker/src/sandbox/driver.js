// The main module of the dynamic Worker each ClaxBot script runs in
// (src/sandbox/mod.rs, LoaderSandbox). The sandbox writes before it the line
// that imports the script's own module (`import make from "<module>";`), a
// CommonJS module whose export makes the script's async function, compiled
// from clax_bot_sandbox::wrap(code) as sloppy-mode script code; the shared
// prelude (clax_bot_sandbox::PRELUDE) is the module clax-prelude.js.
//
// The Worker's instance calls `run(job, host)` once: `job` is the JSON of
// {ops, maxCalls, maxCallArgsBytes, maxLogBytes, truncated, file, nowMs?,
// seed?} (the last two for a determined script), `host` a
// function back into the instance that takes one JSON message and answers
// one JSON text. Messages:
//
//   {records, logs, call: {op, args, bytes}}  a host call, after the calls
//       the script made before it that never reach the host (records) and
//       the log lines since the last message; answered {ok, json}, or
//       {stop} once the instance has ended the run;
//   {final: {records, logs, done?: {ok, payload}, stop?, never?, gone?}}
//       how the script ended: its body settled (done, as the prelude's
//       done(ok, payload) reported it), it went over its host calls
//       (stop: "calls"), it awaited a promise nothing can settle (never),
//       or the host stopped answering (gone).
//
// Once the final message is sent the isolate is aborted (abortIsolate from
// cloudflare:workers), so no job the script queued runs after its end and a
// call over the limit ends it at once, as the native sandbox's do; where
// the runtime has no abortIsolate the run's own answer carries the final
// message instead. Every built-in used after the script starts is captured
// here, before it runs; nothing the script can reach is trusted by the
// instance, which checks every call itself.
import { WorkerEntrypoint } from "cloudflare:workers";
import * as workers from "cloudflare:workers";
import prelude from "clax-prelude.js";

const apply = Reflect.apply;
const parse = JSON.parse;
const stringify = JSON.stringify;
const defineProperty = Object.defineProperty;
const create = Object.create;
const getPrototypeOf = Object.getPrototypeOf;
const setPrototypeOf = Object.setPrototypeOf;
const hasOwn = Object.hasOwn;
const ObjectPrototype = Object.prototype;
const isArray = Array.isArray;
const push = Array.prototype.push;
const charCodeAt = String.prototype.charCodeAt;
const toStringFn = String;
const PromiseCtor = Promise;
const promiseResolve = Promise.resolve;
const then = Promise.prototype.then;
const TypeErrorCtor = TypeError;
const setTimeoutFn = globalThis.setTimeout;
const abortIsolate = typeof workers.abortIsolate === "function" ? workers.abortIsolate : undefined;

// Ambient I/O and Node.js globals the script does not get (it has `clax`).
const AMBIENT = [
  "fetch", "connect", "setTimeout", "setInterval", "clearTimeout", "clearInterval",
  "setImmediate", "clearImmediate", "scheduler", "caches", "WebSocket", "WebSocketPair",
  "EventSource", "XMLHttpRequest", "navigator", "process", "Buffer", "global", "require",
];

// serde_json's nesting limit: it refuses a 128th level of arrays and objects.
const MAX_DEPTH = 127;

const hexDigit = (c) => (c >= 48 && c <= 57 ? c - 48 : c >= 65 && c <= 70 ? c - 55 : c >= 97 && c <= 102 ? c - 87 : -1);

// What the instance's serde_json makes of JSON text that JSON.stringify
// wrote: its UTF-8 length, and whether it parses back (no level past
// MAX_DEPTH, no \u escape of an unpaired surrogate).
const scan = (text) => {
  let bytes = 0;
  let depth = 0;
  let ok = true;
  let inString = false;
  const n = text.length;
  const hexAt = (i) => {
    if (i + 4 > n) return -1;
    let v = 0;
    for (let k = 0; k < 4; k++) {
      const d = hexDigit(apply(charCodeAt, text, [i + k]));
      if (d < 0) return -1;
      v = v * 16 + d;
    }
    return v;
  };
  for (let i = 0; i < n; i++) {
    const c = apply(charCodeAt, text, [i]);
    if (c < 0x80) bytes += 1;
    else if (c < 0x800) bytes += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < n) {
      const d = apply(charCodeAt, text, [i + 1]);
      if (d >= 0xdc00 && d <= 0xdfff) {
        bytes += 4;
        i++;
        continue;
      }
      bytes += 3;
    } else bytes += 3;
    if (inString) {
      if (c === 34) inString = false;
      else if (c === 92) {
        const e = apply(charCodeAt, text, [i + 1]);
        bytes += 1;
        i++;
        if (e === 117) {
          const u = hexAt(i + 1);
          bytes += 4;
          i += 4;
          if (u >= 0xdc00 && u <= 0xdfff) ok = false;
          else if (u >= 0xd800 && u <= 0xdbff) {
            const low = apply(charCodeAt, text, [i + 1]) === 92 && apply(charCodeAt, text, [i + 2]) === 117 ? hexAt(i + 3) : -1;
            if (low >= 0xdc00 && low <= 0xdfff) {
              bytes += 6;
              i += 6;
            } else ok = false;
          }
        }
      }
    } else if (c === 34) inString = true;
    else if (c === 91 || c === 123) {
      depth++;
      if (depth > MAX_DEPTH) ok = false;
    } else if (c === 93 || c === 125) depth--;
  }
  const out = create(null);
  out.bytes = bytes;
  out.ok = ok;
  return out;
};

const utf8Length = (s) => scan(s).bytes;

// An array with no prototype: the script's Array.prototype (a toJSON, an
// index setter) reaches nothing the driver sends.
const list = () => setPrototypeOf([], null);

const record = (op, code, bytes) => {
  const r = create(null);
  r.op = op;
  r.code = code;
  r.bytes = bytes;
  return r;
};

const begin = (job, host, resolveRun) => {
  const opSet = create(null);
  for (let i = 0; i < job.ops.length; i++) opSet[job.ops[i]] = true;
  const queue = list();
  let head = 0;
  let calls = 0;
  let nextId = 0;
  let done = false;
  let finished = false;
  let logs = list();
  let logBytes = 0;
  let logsFull = false;
  let api;

  const takeLogs = () => {
    const out = logs;
    logs = list();
    return out;
  };

  // Ends the run: the calls still queued never reach the host. `earlier`
  // are records already taken from the queue and not yet sent.
  const finish = (result, earlier) => {
    if (finished) return;
    finished = true;
    const records = earlier === undefined ? list() : earlier;
    for (; head < queue.length; head++) {
      const item = queue[head];
      apply(push, records, [item.call ? record(item.op, "script_finished", item.bytes) : item.record]);
    }
    result.records = records;
    result.logs = takeLogs();
    const message = create(null);
    message.final = result;
    const text = stringify(message);
    try {
      apply(host, undefined, [text]);
    } catch (_) {
      // The run's own answer still carries it.
    }
    if (abortIsolate !== undefined) abortIsolate("the script has ended");
    resolveRun(text);
  };

  const call = (op, json) => {
    if (opSet[op] !== true) throw new TypeErrorCtor("no operation " + stringify(toStringFn(op)));
    if (done || finished) throw new TypeErrorCtor("the script has finished; this call was not made");
    if (calls >= job.maxCalls) {
      const r = create(null);
      r.stop = "calls";
      finish(r);
      // Without abortIsolate: a call that never settles, the end already sent.
      return -1;
    }
    calls++;
    let problem;
    let bytes = 0;
    if (typeof json === "string") {
      const s = scan(json);
      bytes = s.bytes;
      if (bytes > job.maxCallArgsBytes) {
        problem = "the arguments of " + op + " are " + bytes + " bytes of JSON; the limit is " + job.maxCallArgsBytes;
      } else if (!s.ok) {
        problem = "the arguments of " + op + " do not parse back from their JSON";
      } else {
        let value;
        try {
          value = parse(json);
        } catch (_) {
          problem = "the arguments of " + op + " do not parse back from their JSON";
        }
        if (problem === undefined && !isArray(value)) problem = "the arguments of " + op + " do not serialise as an array";
      }
    } else if (typeof json === "number") {
      bytes = json;
      problem = "the arguments of " + op + " are at least " + json + " bytes of JSON; the limit is " + job.maxCallArgsBytes;
    } else {
      problem = "the arguments of " + op + " have no JSON";
    }
    if (problem !== undefined) {
      const item = create(null);
      item.call = false;
      item.record = record(op, "invalid_args", bytes);
      apply(push, queue, [item]);
      throw new TypeErrorCtor(problem);
    }
    const item = create(null);
    item.call = true;
    item.id = nextId++;
    item.op = op;
    item.args = json;
    item.bytes = bytes;
    apply(push, queue, [item]);
    return item.id;
  };

  const log = (line) => {
    if (logsFull) return;
    const text = typeof line === "string" ? line : "[unprintable]";
    const len = utf8Length(text);
    if (logBytes + len > job.maxLogBytes) {
      logsFull = true;
      apply(push, logs, [job.truncated]);
      return;
    }
    logBytes += len;
    apply(push, logs, [text]);
  };

  const ended = (ok, payload) => {
    if (done) return;
    done = true;
    const r = create(null);
    const d = create(null);
    d.ok = ok;
    d.payload = payload === undefined ? null : payload;
    r.done = d;
    finish(r);
  };

  // Sends what the script queued, up to its first call, once nothing of
  // the script's is left to run (a timer's turn comes after every queued
  // job), and settles that call with the host's answer.
  const step = () => {
    if (finished) return;
    const records = list();
    let next;
    while (head < queue.length) {
      const item = queue[head++];
      if (item.call) {
        next = item;
        break;
      }
      apply(push, records, [item.record]);
    }
    if (next === undefined) {
      const r = create(null);
      r.never = true;
      finish(r, records);
      return;
    }
    const message = create(null);
    message.records = records;
    message.logs = takeLogs();
    const c = create(null);
    c.op = next.op;
    c.args = next.args;
    c.bytes = next.bytes;
    message.call = c;
    const gone = (e) => {
      const r = create(null);
      r.gone = toStringFn(e);
      finish(r);
    };
    let answer;
    try {
      answer = apply(host, undefined, [stringify(message)]);
    } catch (e) {
      gone(e);
      return;
    }
    apply(then, apply(promiseResolve, PromiseCtor, [answer]), [
      (text) => {
        if (finished) return;
        let reply;
        try {
          reply = parse(text);
        } catch (e) {
          gone(e);
          return;
        }
        if (reply.stop !== undefined) {
          const r = create(null);
          r.gone = toStringFn(reply.stop);
          finish(r);
          return;
        }
        api.settle(next.id, reply.ok === true, reply.json);
        setTimeoutFn(step, 0);
      },
      gone,
    ]);
  };

  const options = create(null);
  options.maxCallArgsBytes = job.maxCallArgsBytes;
  // A determined script's frozen clock and seed, as the prelude takes them.
  if (typeof job.nowMs === "number") options.nowMs = job.nowMs;
  if (typeof job.seed === "number") options.seed = job.seed;
  api = prelude(job.ops, call, log, ended, options);
  api.lockdown();
  // Off the global object and every prototype above it (the runtime keeps
  // some on ServiceWorkerGlobalScope.prototype), so no path reaches them.
  for (let at = globalThis; at !== null && at !== ObjectPrototype; at = getPrototypeOf(at)) {
    for (let i = 0; i < AMBIENT.length; i++) {
      const name = AMBIENT[i];
      if (at !== globalThis && !hasOwn(at, name)) continue;
      try {
        if (at !== globalThis) delete at[name];
        if (at === globalThis || hasOwn(at, name)) {
          defineProperty(at, name, { value: undefined, writable: true, configurable: true });
        }
      } catch (_) {
        // A property the runtime will not let go of keeps its value; the
        // isolate has no network either way (globalOutbound: null).
      }
    }
  }
  const fn = make();
  setTimeoutFn(step, 0);
  api.start(fn, job.file);
};

export class Run extends WorkerEntrypoint {
  run(jobText, host) {
    const job = parse(jobText);
    return new PromiseCtor((resolve) => begin(job, host, resolve));
  }
}

export default {
  fetch() {
    return new Response(null, { status: 404 });
  },
};
