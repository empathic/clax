// The prelude every ClaxBot sandbox runs before a script (clax_bot_sandbox::PRELUDE).
// Called once as prelude(ops, call, log, done, options); returns
// { check, lockdown, start, settle }. Every built-in it uses after the
// script starts is captured here, before the script can replace it.
(ops, call, log, done, options) => {
  "use strict";
  const apply = Reflect.apply;
  const stringify = JSON.stringify;
  const parse = JSON.parse;
  const toString = String;
  const ErrorCtor = Error;
  const TypeErrorCtor = TypeError;
  const EvalErrorCtor = EvalError;
  const PromiseCtor = Promise;
  const then = Promise.prototype.then;
  const create = Object.create;
  const defineProperty = Object.defineProperty;
  const getPrototypeOf = Object.getPrototypeOf;
  const WeakMapCtor = WeakMap;
  const weakGet = WeakMap.prototype.get;
  const weakSet = WeakMap.prototype.set;
  const indexOf = String.prototype.indexOf;
  const charCodeAt = String.prototype.charCodeAt;
  const split = String.prototype.split;
  const toWellFormed = String.prototype.toWellFormed;
  const regexpTest = RegExp.prototype.test;
  const FunctionCtor = Function;
  const AsyncFunction = getPrototypeOf(async function () {}).constructor;
  const GeneratorFunction = getPrototypeOf(function* () {}).constructor;
  const AsyncGeneratorFunction = getPrototypeOf(async function* () {}).constructor;
  const construct = Reflect.construct;
  const imul = Math.imul;
  const RealDate = Date;
  const dateToString = Date.prototype.toString;
  const isArray = Array.isArray;
  const keys = Object.keys;
  const ObjectProto = Object.prototype;
  const setPrototypeOf = Object.setPrototypeOf;
  const objectTag = Object.prototype.toString;
  const maxArgs = options.maxCallArgsBytes;

  // The sandbox's own host errors, by identity: what they carry is read from
  // here, never from the object, so neither a Proxy nor a look-alike passes.
  const hostErrors = new WeakMapCtor();
  const pending = create(null);
  let finished = false;
  let file = "";

  const wellFormed = (s) => (toWellFormed === undefined ? s : apply(toWellFormed, s, []));
  const toText = (x) => {
    if (typeof x === "string") return wellFormed(x);
    let s;
    try {
      s = stringify(x);
    } catch (_) {
      s = undefined;
    }
    if (s === undefined) {
      try {
        s = toString(x);
      } catch (_) {
        s = "[unprintable]";
      }
    }
    return wellFormed(s);
  };

  const hostError = (code, message, retryable, capability) => {
    const e = new ErrorCtor(message);
    defineProperty(e, "name", { value: "HostError", writable: true, configurable: true });
    defineProperty(e, "code", { value: code, writable: true, configurable: true });
    defineProperty(e, "retryable", { value: retryable === true, writable: true, configurable: true });
    if (typeof capability === "string") {
      defineProperty(e, "capability", { value: capability, writable: true, configurable: true });
    }
    const info = create(null);
    info.code = code;
    info.message = message;
    apply(weakSet, hostErrors, [e, info]);
    return e;
  };

  // clax: one null-prototype namespace per segment, one function per op.
  const segment = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
  const opFunction = (op) => (...args) =>
    new PromiseCtor((resolve, reject) => {
      if (finished) {
        reject(hostError("script_finished", "the script has finished; this call was not made"));
        return;
      }
      let json;
      try {
        json = stringify(args);
      } catch (_) {
        json = undefined;
      }
      if (typeof json === "string" && json.length > maxArgs) json = json.length;
      const id = call(op, json);
      const p = create(null);
      p.resolve = resolve;
      p.reject = reject;
      pending[id] = p;
    });
  // The segments no operation name may have (RESERVED_SEGMENTS in
  // lib.rs): names JavaScript calls on an object by itself.
  const reservedSegment = (part) => {
    switch (part) {
      case "__proto__":
      case "constructor":
      case "prototype":
      case "then":
      case "toJSON":
      case "toString":
      case "toLocaleString":
      case "valueOf":
      case "hasOwnProperty":
      case "isPrototypeOf":
      case "propertyIsEnumerable":
      case "__defineGetter__":
      case "__defineSetter__":
      case "__lookupGetter__":
      case "__lookupSetter__":
        return true;
      default:
        return false;
    }
  };
  const clax = create(null);
  for (let i = 0; i < ops.length; i++) {
    const op = ops[i];
    const parts = apply(split, op, ["."]);
    let at = clax;
    for (let j = 0; j < parts.length; j++) {
      const part = parts[j];
      if (!apply(regexpTest, segment, [part]) || reservedSegment(part)) {
        throw new TypeErrorCtor("not a usable operation name: " + op);
      }
      if (j === parts.length - 1) {
        if (at[part] !== undefined) throw new TypeErrorCtor("operation listed twice or also a namespace: " + op);
        at[part] = opFunction(op);
      } else {
        if (at[part] === undefined) at[part] = create(null);
        else if (typeof at[part] !== "object") throw new TypeErrorCtor("operation is also a namespace: " + op);
        at = at[part];
      }
    }
  }
  defineProperty(globalThis, "clax", { value: clax, writable: false, configurable: false });
  const consoleObject = create(null);
  consoleObject.log = (...args) => {
    let line = "";
    for (let i = 0; i < args.length; i++) line += (i === 0 ? "" : " ") + toText(args[i]);
    log(line);
  };
  defineProperty(globalThis, "console", { value: consoleObject, writable: true, configurable: true });

  // What the browser layer gives state (spec §17.2): encode(x), a handle
  // entry for an object of its own, and revive(entry), the object again.
  const stateHooks = create(null);
  // browser-layer

  // state (spec §17.1): the one global that outlives a script, restored
  // from options.state ({value, handles}, handles as {"$clax": i}).
  let current = {};
  if (typeof options.state === "string") {
    const saved = parse(options.state);
    const handles = isArray(saved.handles) ? saved.handles : [];
    const back = (v) => {
      if (v === null || typeof v !== "object") return v;
      const k = keys(v);
      if (!isArray(v) && k.length === 1 && k[0] === "$clax") {
        const h = handles[v.$clax];
        return h !== undefined && stateHooks.revive !== undefined ? stateHooks.revive(h) : null;
      }
      for (let i = 0; i < k.length; i++) v[k[i]] = back(v[k[i]]);
      return v;
    };
    if (saved.value !== null && typeof saved.value === "object" && !isArray(saved.value)) current = back(saved.value);
  }
  defineProperty(globalThis, "state", {
    get: () => current,
    set: (v) => {
      const pr = v !== null && typeof v === "object" && !isArray(v) ? getPrototypeOf(v) : undefined;
      if (pr !== ObjectProto && pr !== null) throw new TypeErrorCtor("state must be a plain object");
      current = v;
    },
    configurable: false,
  });
  // What the script left in state, as {value, handles, dropped, more}: what
  // cannot be kept is left out and listed with its path, never silently.
  const plain = () => setPrototypeOf([], null);
  const putIn = (a, v) => defineProperty(a, a.length, { value: v, writable: true, enumerable: true, configurable: true });
  const saveState = () => {
    const handles = plain();
    const made = plain();
    const dropped = plain();
    const stack = plain();
    let more = 0;
    const drop = (path, why) => {
      if (dropped.length >= 20) {
        more++;
        return;
      }
      const d = create(null);
      d.path = path;
      d.why = why;
      putIn(dropped, d);
    };
    const enc = (x, path, depth) => {
      const t = typeof x;
      if (x === null || t === "string" || t === "boolean") return x;
      if (t === "number") {
        if (x - x === 0) return x;
        drop(path, "a number JSON cannot hold");
        return undefined;
      }
      if (t === "undefined") return undefined;
      if (t !== "object") {
        drop(path, t === "function" ? "a function" : "a " + t);
        return undefined;
      }
      for (let i = 0; i < made.length; i++) {
        if (made[i] === x) {
          const m = create(null);
          m.$clax = i;
          return m;
        }
      }
      const h = stateHooks.encode === undefined ? undefined : stateHooks.encode(x);
      if (h !== undefined) {
        putIn(made, x);
        putIn(handles, h);
        const m = create(null);
        m.$clax = made.length - 1;
        return m;
      }
      for (let i = 0; i < stack.length; i++) {
        if (stack[i] === x) {
          drop(path, "a cycle");
          return undefined;
        }
      }
      if (depth >= 64) {
        drop(path, "nested deeper than 64 levels");
        return undefined;
      }
      try {
        let out;
        putIn(stack, x);
        if (isArray(x)) {
          out = plain();
          for (let i = 0; i < x.length; i++) {
            const v = enc(x[i], path + "[" + i + "]", depth + 1);
            putIn(out, v === undefined ? null : v);
          }
        } else {
          const pr = getPrototypeOf(x);
          if (pr !== ObjectProto && pr !== null) {
            drop(path, "not plain data: " + apply(objectTag, x, []));
            return undefined;
          }
          out = create(null);
          const k = keys(x);
          for (let i = 0; i < k.length; i++) {
            const at = apply(regexpTest, segment, [k[i]]) ? path + "." + k[i] : path + "[" + stringify(k[i]) + "]";
            if (k[i] === "$clax") drop(at, "a member named $clax, which state keeps for handles");
            else {
              const v = enc(x[k[i]], at, depth + 1);
              if (v !== undefined) out[k[i]] = v;
            }
          }
        }
        return out;
      } catch (_) {
        drop(path, "reading it threw");
        return undefined;
      } finally {
        stack.length = stack.length - 1;
      }
    };
    let value = enc(current, "state", 0);
    if (value === null || typeof value !== "object" || isArray(value)) value = create(null);
    const out = create(null);
    out.value = value;
    out.handles = handles;
    out.dropped = dropped;
    out.more = more;
    return stringify(out);
  };

  const lineOf = (stack) => {
    if (typeof stack !== "string") return undefined;
    const key = file + ":";
    for (let i = apply(indexOf, stack, [key]); i >= 0; i = apply(indexOf, stack, [key, i + 1])) {
      let n = 0;
      let digits = 0;
      for (let k = i + key.length; k < stack.length; k++) {
        const c = apply(charCodeAt, stack, [k]);
        if (c < 48 || c > 57) break;
        n = n * 10 + (c - 48);
        digits++;
      }
      // The wrapper's first line is its own (clax_bot_sandbox::WRAP_PREFIX).
      if (digits > 0) return n > 1 ? n - 1 : undefined;
    }
    return undefined;
  };
  const describe = (e) => {
    const out = create(null);
    out.name = "";
    out.message = "";
    out.host = false;
    try {
      const info = typeof e === "object" && e !== null ? apply(weakGet, hostErrors, [e]) : undefined;
      if (info !== undefined) {
        out.name = "HostError";
        out.message = info.message;
        out.code = info.code;
        out.host = true;
      } else if (e instanceof ErrorCtor) {
        out.name = toText(e.name === undefined ? "" : e.name);
        out.message = toText(e.message === undefined ? "" : e.message);
        const line = lineOf(e.stack);
        if (line !== undefined) out.line = line;
      } else {
        out.message = toText(e);
      }
    } catch (_) {
      out.name = "";
      out.message = "[a thrown value that could not be read]";
    }
    return stringify(out);
  };

  const check = (code) => {
    new AsyncFunction(code);
  };

  const lockdown = () => {
    const refuse = () => {
      throw new EvalErrorCtor("code generation from strings is disabled");
    };
    for (const F of [FunctionCtor, AsyncFunction, GeneratorFunction, AsyncGeneratorFunction]) {
      const thrower = function () {
        refuse();
      };
      defineProperty(thrower, "prototype", { value: F.prototype, writable: false });
      defineProperty(F.prototype, "constructor", { value: thrower, writable: true, configurable: true });
      if (F === FunctionCtor) defineProperty(globalThis, "Function", { value: thrower, writable: true, configurable: true });
    }
    const noEval = function () {
      refuse();
    };
    defineProperty(globalThis, "eval", { value: noEval, writable: true, configurable: true });
    delete globalThis.InternalError;
    // A run made again sees what it saw: a frozen clock, seeded randomness.
    if (typeof options.nowMs === "number") {
      const frozen = options.nowMs;
      const FrozenDate = function Date(...args) {
        if (new.target === undefined) return apply(dateToString, construct(RealDate, [frozen]), []);
        return construct(RealDate, args.length === 0 ? [frozen] : args, new.target);
      };
      defineProperty(FrozenDate, "prototype", { value: RealDate.prototype, writable: false });
      defineProperty(RealDate.prototype, "constructor", { value: FrozenDate, writable: true, configurable: true });
      FrozenDate.now = () => frozen;
      FrozenDate.parse = RealDate.parse;
      FrozenDate.UTC = RealDate.UTC;
      defineProperty(globalThis, "Date", { value: FrozenDate, writable: true, configurable: true });
    }
    if (typeof options.seed === "number") {
      let state = options.seed >>> 0;
      const random = () => {
        state = (state + 0x6d2b79f5) >>> 0;
        let t = state;
        t = imul(t ^ (t >>> 15), t | 1);
        t ^= t + imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      };
      defineProperty(Math, "random", { value: random, writable: true, configurable: true });
    }
  };

  const start = (fn, name) => {
    file = name;
    const ended = (ok, payload) => {
      if (finished) return;
      finished = true;
      let saved;
      try {
        saved = saveState();
      } catch (_) {
        saved = undefined;
      }
      done(ok, payload, saved);
    };
    let p;
    try {
      p = fn();
    } catch (e) {
      ended(false, describe(e));
      return;
    }
    apply(then, p, [
      (v) => {
        let j;
        try {
          j = stringify(v);
        } catch (e) {
          ended(false, describe(e));
          return;
        }
        ended(true, j);
      },
      (e) => ended(false, describe(e)),
    ]);
  };

  const settle = (id, ok, json) => {
    const p = pending[id];
    if (p === undefined) return;
    delete pending[id];
    if (ok) {
      p.resolve(parse(json));
      return;
    }
    const h = parse(json);
    p.reject(hostError(h.code, h.message, h.retryable, h.capability));
  };

  const api = create(null);
  api.check = check;
  api.lockdown = lockdown;
  api.start = start;
  api.settle = settle;
  return api;
};
