// The browser layer of the ClaxBot prelude (clax_bot_sandbox::BROWSER_LAYER):
// statements spliced into prelude.js at its `// browser-layer` line, for a
// script that asks for it (clax_bot_sandbox::prelude_source), so they see
// the prelude's captured built-ins. Not a file of its own to run.
  // Browser layer (spec §6.9), rows as clax-bot's browser/layer.rs
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
  ["Page","record","page","record",["opts"]],
  ["Page","runTests","page","runTests",["paths"]],
  ["Page","share","page","share",["opts"]],
  ["Page","revoke","page","revoke",[]],
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
  const is = Object.is;
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
  // A narrowed page of the same session (spec §18.4), shown by what it allows.
  P.Page.share = async function (opts) {
    const p = S(this).p;
    const v = answer(p, await flat.page(p.session, "share", spread(create(null), opts)));
    const n = pageOf(v.page, p.url);
    S(n).shown = toText(v.shown);
    return n;
  };
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
  // A session's Page.
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
  // Shown opaque: a returned or logged Page never shows its capability.
  P.Page.toJSON = function () {
    const s = S(this);
    return (s && s.shown) || "<browser page>";
  };
  P.Locator.toJSON = () => "<browser locator>";
  // In state (spec §17.2): each object of the layer's classes as a handle
  // entry {page, url, of, steps?, not?, timeout?, shown?}, restored live, one Page
  // per handle.
  const classes = ["Page", "Locator", "Keyboard", "Mouse", "LocatorAssertions", "PageAssertions"];
  stateHooks.encode = (o) => {
    const s = S(o);
    if (s === undefined) return undefined;
    let of;
    for (let i = 0; i < classes.length; i++) if (P[classes[i]] === getPrototypeOf(o)) of = classes[i];
    if (of === undefined) return undefined;
    const h = create(null);
    h.page = s.p.session;
    h.url = s.p.url;
    h.of = of;
    if (s.steps !== undefined && s.p !== s) h.steps = s.steps;
    if (s.not === true) h.not = true;
    if (s.p.timeout !== undefined) h.timeout = s.p.timeout;
    if (s.p.shown !== undefined) h.shown = s.p.shown;
    return h;
  };
  const revived = create(null);
  stateHooks.revive = (h) => {
    if (typeof h.page !== "string") return null;
    let page = revived[h.page];
    if (page === undefined) {
      page = pageOf(h.page, typeof h.url === "string" ? h.url : "");
      if (typeof h.timeout === "number") S(page).timeout = h.timeout;
      if (typeof h.shown === "string") S(page).shown = h.shown;
      revived[h.page] = page;
    }
    if (h.of === "Page") return page;
    if (h.of === "Keyboard" || h.of === "Mouse") return page[h.of === "Keyboard" ? "keyboard" : "mouse"];
    if (h.of !== "Locator" && h.of !== "LocatorAssertions" && h.of !== "PageAssertions") return null;
    const n = create(null);
    n.p = S(page);
    if (isArray(h.steps)) n.steps = h.steps;
    if (h.not === true) n.not = true;
    return make(P[h.of], n);
  };
  const open = async (source, opts) => {
    const r = await (opts === undefined ? flat.open(source) : flat.open(source, opts));
    return pageOf(r.page, r.url);
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
    defineProperty(globalThis, "expect", { value: expect, writable: true, configurable: true });
  }
