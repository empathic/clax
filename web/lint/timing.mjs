// clax-timing: oxlint rules for the test-timing rule (AGENTS.md: no sleeps
// standing in for a condition, no wall-clock budgets in any check). They run
// on test files only (lint/timing.json says which) and refuse what lets a
// test's result depend on how fast the machine is:
//
//   timer-delay      setTimeout/setInterval with a non-zero delay that is no
//                    hang guard, and the promise sleeps of
//                    node:timers/promises (a sleep); not in a file on
//                    vi.useFakeTimers, whose timers run on fake time
//   literal-timeout  a numeric `timeout:`, test.setTimeout(n),
//                    setDefaultTimeout(n) or a test's or hook's numeric last
//                    argument (a budget), unless it is 0 or a named hang
//                    guard
//   clock-arithmetic an elapsed time (Date.now()/performance.now() minus
//                    another time) that an if, a loop or an assertion reads,
//                    or the clock compared with a time that is no hang
//                    guard's deadline (a budget); a time only logged is fine
//   disabled-test    test.fixme, an unconditional skip, xit/xdescribe
//   counted-turns    a loop of N bare turns, or settle(N): a guess at how
//                    many turns the work takes
//
// Playwright's waitForTimeout is refused by the built-in
// no-restricted-properties rule beside these.
//
// The rule API is ESLint's (oxlint's JS plugins); lint/timing.test.ts runs
// each rule over passing and failing code with oxlint's RuleTester.

/** Hooks, whose last argument may be a timeout. */
const HOOKS = new Set(["beforeAll", "afterAll", "beforeEach", "afterEach"]);

/** Names that are hang guards: generous bounds that end a stuck test. */
const HANG_GUARDS = new Set(["HANG_MS", "WAIT_MS", "MOUNT_TIMEOUT_MS", "CHILD_TIMEOUT_MS"]);

/** Calls that yield one turn of the event loop (or of fake time). */
const TURNS = new Set(["tick", "turn", "flush", "settle", "settled", "nextTick", "flushPromises", "flushMicrotasks", "setImmediate", "queueMicrotask"]);

/** A settle helper whose first numeric argument would be a turn count. */
const SETTLERS = new Set(["settle", "settled", "turns", "ticks", "flush", "flushTurns"]);

/** What a test-runner call is rooted at (test.skip, it.only, describe.skip, …). */
const RUNNERS = new Set(["test", "it", "describe", "suite", "bench"]);

/** The expression under TypeScript's `as`, `!` and `satisfies`. */
function bare(node) {
  while (node && (node.type === "TSAsExpression" || node.type === "TSNonNullExpression" || node.type === "TSSatisfiesExpression" || node.type === "ParenthesizedExpression")) {
    node = node.expression;
  }
  return node;
}

/** The dotted path of an identifier or a chain of plain member accesses ("test.describe.skip"), or null. */
function path(node) {
  node = bare(node);
  if (!node) return null;
  if (node.type === "Identifier") return node.name;
  if (node.type === "ThisExpression") return "this";
  if (node.type === "MemberExpression" && !node.computed && node.property.type === "Identifier") {
    const object = path(node.object);
    return object === null ? node.property.name : `${object}.${node.property.name}`;
  }
  if (node.type === "CallExpression") return path(node.callee);
  return null;
}

/** The last name a call is made through: `setTimeout` for `window.setTimeout(...)`. */
function calleeName(callee) {
  callee = bare(callee);
  if (callee.type === "Identifier") return callee.name;
  if (callee.type === "MemberExpression" && !callee.computed && callee.property.type === "Identifier") return callee.property.name;
  return null;
}

function isNumberLiteral(node) {
  return node?.type === "Literal" && typeof node.value === "number";
}

/** Whether a hang guard is part of `node` (`HANG_MS`, `WAIT_MS * 2`, `t.HANG_MS`). */
function hasHangGuard(node) {
  node = bare(node);
  if (!node) return false;
  if (node.type === "Identifier") return HANG_GUARDS.has(node.name);
  if (node.type === "MemberExpression") return !node.computed && node.property.type === "Identifier" && HANG_GUARDS.has(node.property.name);
  if (node.type === "BinaryExpression") return hasHangGuard(node.left) || hasHangGuard(node.right);
  if (node.type === "UnaryExpression") return hasHangGuard(node.argument);
  return false;
}

/**
 * What a file's declarations say about time: every `const NAME = <number>`
 * (so `{ timeout: LONG }` reads as the number it is), and every name bound
 * to a hang guard's deadline (`const end = Date.now() + HANG_MS`), which a
 * polling helper may compare the clock with: the condition it polls
 * decides, and the deadline only ends a stuck test.
 */
function declarations(program) {
  const found = new Map();
  const deadlines = new Set();
  const visit = (node) => {
    if (!node || typeof node.type !== "string") return;
    if (node.type === "VariableDeclaration") {
      for (const d of node.declarations) {
        const init = bare(d.init);
        if (d.id.type !== "Identifier" || !init) continue;
        if (node.kind === "const" && numeric(init, found) !== null) found.set(d.id.name, numeric(init, found));
        if (init.type === "BinaryExpression" && init.operator === "+" && isNow(init.left) && hasHangGuard(init.right)) deadlines.add(d.id.name);
      }
    }
    for (const key in node) {
      if (key === "parent") continue;
      const child = node[key];
      if (Array.isArray(child)) child.forEach(visit);
      else if (child && typeof child === "object") visit(child);
    }
  };
  visit(program);
  return { consts: found, deadlines };
}

/**
 * The value of a numeric expression made of literals and known constants
 * (`30_000`, `60 * 1000`, `LONG`), or null when it is not one, or when a
 * hang guard is part of it (`WAIT_MS * 2` is still a hang guard).
 */
function numeric(node, consts) {
  node = bare(node);
  if (!node) return null;
  if (isNumberLiteral(node)) return node.value;
  if (node.type === "Identifier") return HANG_GUARDS.has(node.name) ? null : (consts.get(node.name) ?? null);
  if (node.type === "UnaryExpression" && (node.operator === "-" || node.operator === "+")) {
    const v = numeric(node.argument, consts);
    return v === null ? null : node.operator === "-" ? -v : v;
  }
  if (node.type === "BinaryExpression" && ["+", "-", "*", "/"].includes(node.operator)) {
    const a = numeric(node.left, consts);
    const b = numeric(node.right, consts);
    if (a === null || b === null) return null;
    return { "+": a + b, "-": a - b, "*": a * b, "/": a / b }[node.operator];
  }
  return null;
}

/** Whether `node` is a budget: a non-zero number that is no hang guard. */
function isBudget(node, consts) {
  const v = numeric(node, consts);
  return v !== null && v !== 0;
}

/** Whether `node` reads the wall or monotonic clock: `Date.now()`, `performance.now()`. */
function isNow(node) {
  node = bare(node);
  if (node?.type !== "CallExpression" || node.arguments.length !== 0) return false;
  const p = path(node.callee);
  return p === "Date.now" || p === "performance.now" || p === "window.performance.now" || p === "globalThis.performance.now";
}

/** Whether an awaited expression only yields a turn: `tick()`, `Promise.resolve()`, `new Promise(r => setTimeout(r, 0))`. */
function isTurn(node) {
  node = bare(node);
  if (!node) return false;
  if (node.type === "Literal" || (node.type === "Identifier" && node.name === "undefined")) return true;
  if (node.type === "CallExpression") {
    const p = path(node.callee);
    if (p === "Promise.resolve" && node.arguments.length === 0) return true;
    if (p === "vi.advanceTimersByTimeAsync" && node.arguments.length === 1 && isNumberLiteral(node.arguments[0]) && node.arguments[0].value === 0) return true;
    return TURNS.has(calleeName(node.callee)) && node.arguments.every((a) => isNumberLiteral(a) && a.value === 0);
  }
  if (node.type === "NewExpression" && path(node.callee) === "Promise" && node.arguments.length === 1) {
    const executor = bare(node.arguments[0]);
    if (executor.type === "Identifier") return executor.name === "setImmediate" || executor.name === "queueMicrotask";
    if (executor.type !== "ArrowFunctionExpression" && executor.type !== "FunctionExpression") return false;
    const body = executor.body.type === "BlockStatement" && executor.body.body.length === 1 && executor.body.body[0].type === "ExpressionStatement" ? executor.body.body[0].expression : executor.body;
    if (bare(body)?.type !== "CallExpression") return false;
    const name = calleeName(bare(body).callee);
    const args = bare(body).arguments;
    if (name === "setImmediate" || name === "queueMicrotask") return true;
    return name === "setTimeout" && (args.length < 2 || (isNumberLiteral(args[1]) && args[1].value === 0));
  }
  return false;
}

/** The statements of a loop body. */
function statements(body) {
  return body.type === "BlockStatement" ? body.body : [body];
}

const timerDelay = {
  meta: {
    type: "problem",
    docs: { description: "A timer with a non-zero delay in a test is a sleep standing in for a condition." },
    messages: {
      delay: "{{name}} with a delay in a test: wait on the condition itself (expect.poll, vi.waitFor, until), or drive a clock (vi.useFakeTimers, claxTestClock.advance, page.clock)",
    },
  },
  create(context) {
    // A file on vi.useFakeTimers runs its timers by advancing fake time.
    if (context.sourceCode.text.includes("useFakeTimers(")) return {};
    // Local names of node:timers/promises' sleeps (`import { setTimeout as sleep } from "node:timers/promises"`).
    const sleeps = new Set();
    return {
      ImportDeclaration(node) {
        if (!/^(node:)?timers\/promises$/.test(node.source.value)) return;
        for (const s of node.specifiers) {
          if (s.type === "ImportSpecifier" && ["setTimeout", "setInterval"].includes(s.imported.name ?? s.imported.value)) sleeps.add(s.local.name);
        }
      },
      CallExpression(node) {
        const callee = bare(node.callee);
        if (callee.type === "Identifier" && sleeps.has(callee.name)) {
          const delay = node.arguments[0];
          if (delay && !(isNumberLiteral(bare(delay)) && bare(delay).value === 0) && !hasHangGuard(delay)) context.report({ node, messageId: "delay", data: { name: callee.name } });
          return;
        }
        const p = path(callee);
        const timer = ["setTimeout", "setInterval"].find((t) => p === t || p === `window.${t}` || p === `globalThis.${t}` || p === `self.${t}` || p === `global.${t}`);
        if (!timer || node.arguments.length < 2) return;
        const delay = bare(node.arguments[1]);
        // A zero delay is one turn; a hang guard's only ends a stuck test.
        if ((isNumberLiteral(delay) && delay.value === 0) || hasHangGuard(delay)) return;
        context.report({ node, messageId: "delay", data: { name: timer } });
      },
    };
  },
};

const literalTimeout = {
  meta: {
    type: "problem",
    docs: { description: "A numeric timeout in a test is a wall-clock budget." },
    messages: {
      budget: "a numeric timeout in a test is a budget the machine's load can break: leave the hang-guard default, or name one (HANG_MS, WAIT_MS, MOUNT_TIMEOUT_MS); 0 waits without a bound",
    },
  },
  create(context) {
    let consts = new Map();
    return {
      Program(node) {
        consts = declarations(node).consts;
      },
      Property(node) {
        const key = node.key.type === "Identifier" ? node.key.name : node.key.value;
        if (key === "timeout" && !node.computed && isBudget(node.value, consts)) context.report({ node, messageId: "budget" });
      },
      CallExpression(node) {
        const name = calleeName(node.callee);
        const p = path(node.callee);
        // test.setTimeout(n), testInfo.setTimeout(n), page.setDefaultTimeout(n): one argument, a budget.
        // A timer's setTimeout takes a callback first, and is timer-delay's.
        if ((name === "setTimeout" && node.arguments.length === 1) || name === "setDefaultTimeout" || name === "setDefaultNavigationTimeout") {
          if (node.arguments[0] && isBudget(node.arguments[0], consts)) context.report({ node, messageId: "budget" });
          return;
        }
        // it("name", fn, 20_000), beforeAll(fn, 60_000): vitest's per-test
        // and per-hook timeout as a last argument.
        const root = p?.split(".")[0];
        if ((RUNNERS.has(root) && node.arguments.length >= 3) || (HOOKS.has(root) && node.arguments.length >= 2)) {
          const last = node.arguments[node.arguments.length - 1];
          if (isBudget(last, consts)) context.report({ node: last, messageId: "budget" });
        }
      },
    };
  },
};

/** Calls whose arguments decide a test's result. */
function isAssertion(call) {
  const root = path(call.callee)?.split(".")[0];
  return root === "expect" || root === "assert";
}

/**
 * Whether `node` takes part in a decision: inside an `if`, loop or `?:`
 * test, or an `expect(...)`/`assert(...)` argument. A time only logged or
 * reported decides nothing.
 */
function decides(node) {
  for (let child = node, parent = node.parent; parent; child = parent, parent = parent.parent) {
    if (["IfStatement", "WhileStatement", "DoWhileStatement", "ForStatement", "ConditionalExpression"].includes(parent.type) && parent.test === child) return true;
    if (parent.type === "CallExpression" && parent.arguments.includes(child) && isAssertion(parent)) return true;
    if (/Statement$|Declaration$/.test(parent.type) || parent.type.startsWith("Function") || parent.type === "ArrowFunctionExpression") return false;
  }
  return false;
}

/** The identifiers in a subtree, by name. */
function identifiers(root, names, found = []) {
  if (!root || typeof root.type !== "string") return found;
  if (root.type === "Identifier" && names.has(root.name)) found.push(root);
  for (const key in root) {
    if (key === "parent") continue;
    const child = root[key];
    if (Array.isArray(child)) child.forEach((c) => identifiers(c, names, found));
    else if (child && typeof child === "object") identifiers(child, names, found);
  }
  return found;
}

const clockArithmetic = {
  meta: {
    type: "problem",
    docs: { description: "Elapsed wall-clock time deciding a test's result is a budget." },
    messages: {
      elapsed: "elapsed time decides a test's result: assert the outcome, wait on a condition (expect.poll, vi.waitFor, until), or drive a clock instead of measuring one",
    },
  },
  create(context) {
    let program = null;
    let consts = new Map();
    let deadlines = new Set();
    // Elapsed times bound to a name (`const ms = Date.now() - t0`): reported
    // once the whole file is read, if the name is part of a decision.
    const measured = new Map();
    return {
      Program(node) {
        program = node;
        ({ consts, deadlines } = declarations(node));
      },
      BinaryExpression(node) {
        const left = isNow(node.left);
        const right = isNow(node.right);
        if (!left && !right) return;
        const other = bare(left ? node.right : node.left);
        if (node.operator === "-") {
          // Date.now() - 3_600_000 is a timestamp an hour ago, not a measurement.
          if (numeric(other, consts) !== null || isNow(other)) return;
          if (decides(node)) context.report({ node, messageId: "elapsed" });
          else if (node.parent?.type === "VariableDeclarator" && node.parent.id.type === "Identifier") measured.set(node.parent.id.name, node);
        } else if (["<", ">", "<=", ">="].includes(node.operator)) {
          // A hang guard's deadline ends a stuck test; it decides no result.
          if (hasHangGuard(other) || (other.type === "Identifier" && deadlines.has(other.name))) return;
          context.report({ node, messageId: "elapsed" });
        }
      },
      "Program:exit"() {
        if (measured.size === 0) return;
        const used = new Set(identifiers(program, new Set(measured.keys())).filter(decides).map((id) => id.name));
        for (const [name, node] of measured) if (used.has(name)) context.report({ node, messageId: "elapsed" });
      },
    };
  },
};

const disabledTest = {
  meta: {
    type: "problem",
    docs: { description: "A test switched off for good hides what it covers (often a flake)." },
    messages: {
      fixme: "test.fixme in a test file: fix the test, or record why it waits (qualifier) and delete it",
      skip: "an unconditional skip: make it conditional on what the test needs (test.skip(cond, why)), or fix or delete the test",
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        const p = path(node.callee);
        if (!p) return;
        const parts = p.split(".");
        if (["xit", "xtest", "xdescribe"].includes(parts[0]) && parts.length === 1) {
          context.report({ node, messageId: "skip" });
          return;
        }
        if (!RUNNERS.has(parts[0]) || parts.length < 2) return;
        const last = parts[parts.length - 1];
        if (last === "fixme") {
          context.report({ node, messageId: "fixme" });
          return;
        }
        if (last !== "skip") return;
        // Unconditional: test.skip(), test.skip(true, …), or a skipped declaration test.skip("name", fn).
        const first = bare(node.arguments[0]);
        if (!first || (first.type === "Literal" && (first.value === true || typeof first.value === "string")) || first.type === "TemplateLiteral") {
          context.report({ node, messageId: "skip" });
        }
      },
    };
  },
};

const countedTurns = {
  meta: {
    type: "problem",
    docs: { description: "A counted number of event-loop turns is a guess at how long the work takes." },
    messages: {
      loop: "{{count}} bare turns in a loop: wait for the condition the turns stand for (settled(), expect.poll, vi.waitFor), or name the one turn an ordering needs",
      count: "a settle with a turn count: wait for the condition the turns stand for (settled(), expect.poll, vi.waitFor)",
    },
  },
  create(context) {
    return {
      ForStatement(node) {
        const test = bare(node.test);
        if (test?.type !== "BinaryExpression" || !["<", "<="].includes(test.operator) || !isNumberLiteral(test.right)) return;
        const body = statements(node.body);
        const turns = body.length > 0 && body.every((s) => s.type === "ExpressionStatement" && bare(s.expression).type === "AwaitExpression" && isTurn(bare(s.expression).argument));
        if (turns && test.right.value >= 2) context.report({ node, messageId: "loop", data: { count: String(test.right.value) } });
      },
      CallExpression(node) {
        if (!SETTLERS.has(calleeName(node.callee))) return;
        const first = bare(node.arguments[0]);
        if (isNumberLiteral(first) && first.value >= 2) context.report({ node, messageId: "count" });
      },
    };
  },
};

export default {
  meta: { name: "clax-timing" },
  rules: {
    "timer-delay": timerDelay,
    "literal-timeout": literalTimeout,
    "clock-arithmetic": clockArithmetic,
    "disabled-test": disabledTest,
    "counted-turns": countedTurns,
  },
};
