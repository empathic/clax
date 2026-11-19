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

  // The browser layer (spec §6.9): rows as clax-bot's browser/layer.rs.
  const table =
  // browser-layer-table:begin
  [
  ["Page","goto","page","goto",["url","opts"]],
  ["Page","reload","page","reload",["opts"]],
  ["Page","goBack","page","goBack",["opts"]],
  ["Page","goForward","page","goForward",["opts"]],
  ["Page","title","page","title",[]],
  ["Page","content","page","content",[]],
  ["Page","setViewportSize","page","setViewportSize",["size"]],
  ["Page","waitForLoadState","page","waitForLoadState",["state","opts"]],
  ["Page","waitForURL","page","waitForURL",["url","opts"]],
  ["Page","evaluate","page","evaluate",["fn","arg"]],
  ["Page","screenshot","page","screenshot",["opts"]],
  ["Page","consoleMessages","page","consoleMessages",[]],
  ["Page","pageErrors","page","pageErrors",[]],
  ["Page","failedRequests","page","failedRequests",[]],
  ["Page","runTests","page","runTests",["paths"]],
  ["Page","close","close",null,[]],
  ["Keyboard","down","page","keyboard.down",["key"]],
  ["Keyboard","up","page","keyboard.up",["key"]],
  ["Keyboard","press","page","keyboard.press",["key"]],
  ["Keyboard","type","page","keyboard.type",["text"]],
  ["Keyboard","insertText","page","keyboard.insertText",["text"]],
  ["Mouse","move","page","mouse.move",["x","y","opts"]],
  ["Mouse","down","page","mouse.down",["opts"]],
  ["Mouse","up","page","mouse.up",["opts"]],
  ["Mouse","click","page","mouse.click",["x","y","opts"]],
  ["Mouse","dblclick","page","mouse.dblclick",["x","y","opts"]],
  ["Mouse","wheel","page","mouse.wheel",["deltaX","deltaY"]],
  ["Locator","click","act","click",["opts"]],
  ["Locator","dblclick","act","dblclick",["opts"]],
  ["Locator","fill","act","fill",["value","opts"]],
  ["Locator","press","act","press",["key","opts"]],
  ["Locator","pressSequentially","act","pressSequentially",["text","opts"]],
  ["Locator","hover","act","hover",["opts"]],
  ["Locator","focus","act","focus",["opts"]],
  ["Locator","check","act","check",["opts"]],
  ["Locator","uncheck","act","uncheck",["opts"]],
  ["Locator","selectOption","act","selectOption",["values","opts"]],
  ["Locator","textContent","act","textContent",["opts"]],
  ["Locator","innerText","act","innerText",["opts"]],
  ["Locator","inputValue","act","inputValue",["opts"]],
  ["Locator","getAttribute","act","getAttribute",["name","opts"]],
  ["Locator","allTextContents","act","allTextContents",[]],
  ["Locator","isVisible","act","isVisible",[]],
  ["Locator","isHidden","act","isHidden",[]],
  ["Locator","isEnabled","act","isEnabled",["opts"]],
  ["Locator","isChecked","act","isChecked",["opts"]],
  ["Locator","count","act","count",[]],
  ["Locator","waitFor","act","waitFor",["opts"]],
  ["Locator","scrollIntoViewIfNeeded","act","scrollIntoViewIfNeeded",["opts"]],
  ["Locator","screenshot","act","screenshot",["opts"]],
  ["LocatorAssertions","toBeVisible","expect","toBeVisible",["opts"]],
  ["LocatorAssertions","toBeHidden","expect","toBeHidden",["opts"]],
  ["LocatorAssertions","toBeEnabled","expect","toBeEnabled",["opts"]],
  ["LocatorAssertions","toBeDisabled","expect","toBeDisabled",["opts"]],
  ["LocatorAssertions","toBeChecked","expect","toBeChecked",["opts"]],
  ["LocatorAssertions","toBeFocused","expect","toBeFocused",["opts"]],
  ["LocatorAssertions","toHaveText","expect","toHaveText",["expected","opts"]],
  ["LocatorAssertions","toContainText","expect","toContainText",["expected","opts"]],
  ["LocatorAssertions","toHaveValue","expect","toHaveValue",["value","opts"]],
  ["LocatorAssertions","toHaveAttribute","expect","toHaveAttribute",["name","value","opts"]],
  ["LocatorAssertions","toHaveCount","expect","toHaveCount",["count","opts"]],
  ["PageAssertions","toHaveURL","expect","toHaveURL",["url","opts"]],
  ["PageAssertions","toHaveTitle","expect","toHaveTitle",["title","opts"]]
  ]
  // browser-layer-table:end
  ;
  const flatNames = ["open", "page", "act", "expect", "close"];
  const B = clax.browser;
  const keys = Object.keys;
  const isArray = Array.isArray;
  const is = Object.is;
  const ObjectProto = Object.prototype;
  const hasOwn = ObjectProto.hasOwnProperty;
  const fnSource = Function.prototype.toString;
  const RegExpCtor = RegExp;
  const reSource = Object.getOwnPropertyDescriptor(RegExp.prototype, "source").get;
  const reFlags = Object.getOwnPropertyDescriptor(RegExp.prototype, "flags").get;
  const getTime = Date.prototype.getTime;
  const slice = String.prototype.slice;
  const arrayFrom = Array.from;
  const iterator = Symbol.iterator;
  const flat = create(null);
  const states = new WeakMapCtor();
  const S = (o) => apply(weakGet, states, [o]);
  const make = (proto, s) => {
    const o = create(proto);
    apply(weakSet, states, [o, s]);
    return o;
  };
  const P = create(null);
  for (const c of ["Page", "Locator", "Keyboard", "Mouse", "LocatorAssertions", "PageAssertions"]) P[c] = create(null);
  const put = (a, v) => defineProperty(a, a.length, { value: v, writable: true, enumerable: true, configurable: true });
  // RegExp as {regex, flags}, nested too.
  const conv = (x) => {
    if (x instanceof RegExpCtor) {
      const o = create(null);
      o.regex = apply(reSource, x, []);
      o.flags = apply(reFlags, x, []);
      return o;
    }
    if (isArray(x)) {
      const a = [];
      for (let i = 0; i < x.length; i++) put(a, conv(x[i]));
      return a;
    }
    if (x !== null && typeof x === "object") {
      const pr = getPrototypeOf(x);
      if (pr === null || pr === ObjectProto) return spread(create(null), x);
    }
    return x;
  };
  const spread = (out, o) => {
    if (o !== null && typeof o === "object") {
      const k = keys(o);
      for (let i = 0; i < k.length; i++) if (o[k[i]] !== undefined) out[k[i]] = conv(o[k[i]]);
    }
    return out;
  };
  const named = (name, message) => {
    const e = new ErrorCtor(message);
    if (name !== "Error") defineProperty(e, "name", { value: name, writable: true, configurable: true });
    return e;
  };
  // An answer's value; its failure thrown as Playwright's (an expect's as
  // the harness's Expected). page.url() follows each answer's url.
  const answer = (p, r, failName) => {
    if (typeof r.url === "string" && r.url !== "") p.url = r.url;
    if (r.ok !== true) throw named(failName || (r.timeout === true ? "TimeoutError" : "Error"), toText(r.error));
    return r.value;
  };
  const data = (d, names) => {
    const o = create(null);
    for (let i = 0; i < names.length; i++) o[names[i]] = () => d[names[i]];
    o.toJSON = () => d;
    return o;
  };
  const list = (v, f) => {
    if (!isArray(v)) return v;
    const a = [];
    for (let i = 0; i < v.length; i++) put(a, f(v[i]));
    return a;
  };
  const shape = (action, v) => {
    switch (action) {
      case "goto":
      case "reload":
      case "goBack":
      case "goForward":
        return v === null || v === undefined ? null : data(v, ["url", "status", "ok"]);
      case "consoleMessages":
        return list(v, (d) => data(d, ["type", "text", "location"]));
      case "pageErrors":
        return list(v, (d) => {
          const e = named("Error", toText(d.message));
          if (d.stack !== undefined) defineProperty(e, "stack", { value: d.stack, writable: true, configurable: true });
          defineProperty(e, "toJSON", { value: () => d, writable: true, configurable: true });
          return e;
        });
      default:
        return v;
    }
  };
  for (const [c, name, op, action, params] of table) {
    const withTimeout = op !== "expect" && params.indexOf("opts") >= 0;
    P[c][name] = async function (...a) {
      const s = S(this);
      const p = s.p;
      const args = create(null);
      for (let i = 0; i < params.length; i++) {
        if (params[i] === "opts") spread(args, a[i]);
        else if (a[i] !== undefined) args[params[i]] = conv(a[i]);
      }
      if (s.not === true) args.not = true;
      if (withTimeout && args.timeout === undefined && p.timeout !== undefined) args.timeout = p.timeout;
      if (op === "page") return shape(action, answer(p, await flat.page(p.session, action, args)));
      if (op === "act") return answer(p, await flat.act(p.session, s.steps, action, args));
      return answer(p, await flat.expect(p.session, s.steps || [], action, args), "Expected");
    };
  }
  P.Page.evaluate = async function (fn, arg) {
    const p = S(this).p;
    const args = create(null);
    if (typeof fn === "function") {
      const source = apply(fnSource, fn, []);
      if (apply(indexOf, source, ["[native code]"]) >= 0) {
        throw new TypeErrorCtor("page.evaluate: a native or bound function has no source to send");
      }
      args.source = source;
      if (arg !== undefined) args.arg = arg;
    } else if (typeof fn === "string") args.expression = fn;
    else throw new TypeErrorCtor("page.evaluate: expects a function or a string");
    return answer(p, await flat.page(p.session, "evaluate", args));
  };
  P.Page.close = function () { return flat.close(S(this).p.session); };
  P.Page.url = function () { return S(this).p.url; };
  P.Page.setDefaultTimeout = function (ms) { S(this).p.timeout = ms; };
  // Locators are data: each builder adds one step and makes no call.
  const chain = (self, k, v, o) => {
    const s = S(self);
    const step = create(null);
    step[k] = conv(v);
    spread(step, o);
    const steps = [];
    if (s.steps !== undefined) for (let i = 0; i < s.steps.length; i++) put(steps, s.steps[i]);
    put(steps, step);
    const n = create(null);
    n.p = s.p;
    n.steps = steps;
    return make(P.Locator, n);
  };
  for (const [name, k] of [["locator", "css"], ["getByRole", "role"], ["getByText", "text"], ["getByLabel", "label"], ["getByPlaceholder", "placeholder"], ["getByTestId", "testId"]]) {
    P.Page[name] = P.Locator[name] = function (v, o) { return chain(this, k, v, o); };
  }
  P.Locator.first = function () { return chain(this, "nth", 0); };
  P.Locator.last = function () { return chain(this, "nth", -1); };
  P.Locator.nth = function (i) { return chain(this, "nth", i); };
  P.Locator.filter = function (o) {
    let l = this;
    if (o && o.hasText !== undefined) l = chain(l, "hasText", o.hasText);
    if (o && o.hasNotText !== undefined) l = chain(l, "hasNotText", o.hasNotText);
    return l;
  };
  // A session's Page; clax.browser.session(id) in a later script.
  const pageOf = (session, url) => {
    const p = create(null);
    p.session = session;
    p.url = url;
    p.p = p;
    const page = make(P.Page, p);
    const sub = (c) => {
      const s = create(null);
      s.p = p;
      return { value: make(P[c], s) };
    };
    defineProperty(page, "keyboard", sub("Keyboard"));
    defineProperty(page, "mouse", sub("Mouse"));
    return page;
  };
  P.Page.toJSON = function () {
    const p = S(this).p;
    return { session: p.session, url: p.url };
  };
  const open = async (source, opts) => {
    const r = await (opts === undefined ? flat.open(source) : flat.open(source, opts));
    return pageOf(r.session, r.url);
  };
  for (const c of ["LocatorAssertions", "PageAssertions"]) {
    defineProperty(P[c], "not", {
      get() {
        const s = S(this);
        const n = create(null);
        n.p = s.p;
        n.steps = s.steps;
        n.not = s.not !== true;
        return make(P[c], n);
      },
    });
  }
  // expect of any other value: the saved-test harness's five matchers
  // (web/bridge/src/harness.ts), in the sandbox, no call.
  const cut = (s, n) => (s.length > n ? apply(slice, s, [0, n]) + "…" : s);
  const show = (v) => {
    if (typeof v === "string") return stringify(cut(v, 100));
    if (v === undefined || typeof v === "function" || typeof v === "symbol" || typeof v === "bigint") return toString(v);
    try {
      const j = stringify(v);
      return cut(j === undefined ? toString(v) : j, 100);
    } catch (_) {
      return toString(v);
    }
  };
  const defined = (o) => {
    const k = keys(o);
    const out = [];
    for (let i = 0; i < k.length; i++) if (o[k[i]] !== undefined) put(out, k[i]);
    return out;
  };
  const equal = (a, b) => {
    if (is(a, b)) return true;
    if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
    if (a instanceof RealDate || b instanceof RealDate) {
      return a instanceof RealDate && b instanceof RealDate && apply(getTime, a, []) === apply(getTime, b, []);
    }
    if (isArray(a) !== isArray(b)) return false;
    if (isArray(a)) {
      if (a.length !== b.length) return false;
      for (let i = 0; i < a.length; i++) if (!equal(a[i], b[i])) return false;
      return true;
    }
    const ka = defined(a);
    if (ka.length !== defined(b).length) return false;
    for (let i = 0; i < ka.length; i++) if (!apply(hasOwn, b, [ka[i]]) || !equal(a[ka[i]], b[ka[i]])) return false;
    return true;
  };
  const values = (actual, not) => {
    const check = (ok, what) => {
      if (ok === not) throw named("Expected", show(actual) + (not ? " not " : " ") + what);
    };
    const o = create(null);
    o.toBe = (x) => check(is(actual, x), "to be " + show(x));
    o.toEqual = (x) => check(equal(actual, x), "to equal " + show(x));
    o.toBeTruthy = () => check(!!actual, "to be truthy");
    o.toContain = (x) => {
      let ok = false;
      if (typeof actual === "string") ok = typeof x === "string" && apply(indexOf, actual, [x]) >= 0;
      else if (actual !== null && typeof actual === "object" && iterator in actual) {
        const a = isArray(actual) ? actual : arrayFrom(actual);
        for (let i = 0; i < a.length; i++) if (equal(a[i], x)) ok = true;
      }
      check(ok, "to contain " + show(x));
    };
    o.toBeGreaterThan = (x) => check(typeof actual === "number" && typeof x === "number" && actual > x, "to be greater than " + show(x));
    defineProperty(o, "not", { get: () => values(actual, !not) });
    return o;
  };
  const expect = (actual) => {
    const s = S(actual);
    if (s !== undefined && s.steps !== undefined && s.not === undefined) return make(P.LocatorAssertions, s);
    if (s !== undefined && s.p === s) return make(P.PageAssertions, s);
    return values(actual, false);
  };
  if (B !== undefined && flatNames.every((n) => typeof B[n] === "function")) {
    for (const n of flatNames) {
      flat[n] = B[n];
      delete B[n];
    }
    B.open = open;
    B.session = (id) => pageOf(toText(id), "");
    defineProperty(globalThis, "expect", { value: expect, writable: true, configurable: true });
  }

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
      done(ok, payload);
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
