import { describe, expect, it } from "vitest";
import { WEB_LINES, errorDetail as detail, meBot, run, week } from "./fixtures";
import { OWN_KEY_SPEND, actorLabel, after, applyEvent, availability, banner, callCost, credits, errorDetail, instanceProblemWords, stepLine, stopWords, fromText, margins, meter, meterWords, money, parseEvent, perUnitMilli, percent, rangeFrom, resetDay, runCost, runStatus, sections, tail, tokens, turnActivity, unavailableFix, unavailableText, webPrices, webSource } from "./model";

describe("ClaxBot's numbers", () => {
  it("shows milli-credits as credits, micro-USD as dollars and basis points as percentages", () => {
    expect(credits(1_690_000)).toBe("1,690");
    expect(credits(760)).toBe("0.76");
    expect(credits(2_500)).toBe("2.5");
    expect(money(34_100_000)).toBe("$34.10");
    expect(money(4000)).toBe("$0.0040");
    expect(money(0)).toBe("$0.00");
    expect(percent(4000)).toBe("40%");
    expect(percent(1250)).toBe("12.5%");
    expect(tokens(38_200)).toBe("38k");
    expect(tokens(9_400_000)).toBe("9.4M");
    expect(tokens(4_200)).toBe("4.2k");
  });

  it("charges a cost line its cost with its margin over the credit price, rounded up to the milli-credit", () => {
    // $3.00 per million input tokens at 40 % over $0.01 a credit: 420 credits.
    expect(perUnitMilli({ cost_micro_usd: 3_000_000, margin_bp: 4000 }, 10_000)).toBe(420_000);
    // $0.0040 per CPU·GB-s at 90 %: 0.76 credits.
    expect(perUnitMilli({ cost_micro_usd: 4000, margin_bp: 9000 }, 10_000)).toBe(760);
    // Rounded up: $0.0001 at 0 % over $0.03 is 3.33… milli-credits.
    expect(perUnitMilli({ cost_micro_usd: 100, margin_bp: 0 }, 30_000)).toBe(4);
    // A fixed line keeps its own rate.
    expect(perUnitMilli({ cost_micro_usd: 0, margin_bp: 0, fixed_credits_milli: 1000 }, 10_000)).toBe(1000);
  });

  it("gives the gross margin before and after the free tier", () => {
    expect(margins({ revenue_micro_usd: 34_100_000, cost_micro_usd: 22_900_000, free_cost_micro_usd: 4_600_000 })).toEqual({ before: 3284, after: 1935 });
    expect(margins({ revenue_micro_usd: 0, cost_micro_usd: 1, free_cost_micro_usd: 1 })).toEqual({ before: null, after: null });
  });
});

describe("the week's meter", () => {
  it("warns from 80 % and is exhausted at the allowance", () => {
    expect(meter(week())).toMatchObject({ spent: 310_000, total: 2_000_000, left: 1_690_000, warn: false, exhausted: false });
    expect(meter(week({ spent_milli: 1_600_000 })).warn).toBe(true);
    expect(meter(week({ spent_milli: 2_000_000 }))).toMatchObject({ warn: false, exhausted: true, left: 0, ratio: 1 });
    expect(meter(week({ exhausted: true, spent_milli: 10 })).exhausted).toBe(true);
  });

  it("resets on the week's end, a Monday in UTC", () => {
    expect(resetDay("2026-10-26T00:00:00Z")).toBe("Monday, Oct 26");
  });

  it("bans nothing below 80 %, warns from it, and pauses at the limit unless unenforced", () => {
    expect(banner(meBot())).toBeNull();
    expect(banner(meBot({ enabled: false, week: week({ spent_milli: 2_000_000 }) }))).toBeNull();
    expect(banner(meBot({ week: week({ spent_milli: 1_760_000 }) }))).toMatchObject({ kind: "warn", text: "You've used 1,760 of your 2,000 credits this week." });
    const out = banner(meBot({ week: week({ spent_milli: 2_000_000 }) }))!;
    expect(out.kind).toBe("exhausted");
    expect(out.detail).toContain("paused until Monday, Oct 26");
    // Unenforced: the numbers, never the pause.
    expect(banner(meBot({ unenforced: true, week: week({ spent_milli: 2_100_000 }) }))!.kind).toBe("warn");
  });

  // On their own key, tokens charge no credits but the lines charged on
  // every key (the run fee, compute, storage, web) do, and the limit still
  // pauses ClaxBot: the meter stays, saying whose key and what it counts.
  const own = (o: Parameters<typeof meBot>[0] = {}) => meBot({ key: { own: { set: true, last4: "4f2c", set_at: null, last_ok_at: null }, use_own: true }, ...o });

  it("says whose key the meter counts for, and what credits pay for on the person's own", () => {
    expect(meterWords(meBot())).toEqual({ own: false, label: "ClaxBot: 310 of 2,000 credits used this week", title: "Your ClaxBot credits this week" });
    const w = meterWords(own({ week: week({ spent_milli: 3_120 }) }));
    expect(w.own).toBe(true);
    expect(w.label).toBe(`ClaxBot on your key: 3.12 of 2,000 credits used this week, for ${OWN_KEY_SPEND}`);
    expect(w.title).toContain("your key, which pays for your tokens");
    // A key set but not in use is the instance's.
    expect(meterWords(meBot({ key: { own: { set: true, last4: "4f2c", set_at: null, last_ok_at: null }, use_own: false } })).own).toBe(false);
  });

  it("tells a person on their own key that the banner's credits are not tokens", () => {
    expect(banner(own({ week: week({ spent_milli: 1_760_000 }) }))!.detail).toBe(`ClaxBot keeps working until you reach it. Resets Monday, Oct 26. On your key, credits pay for ${OWN_KEY_SPEND}, not tokens.`);
    const out = banner(own({ week: week({ spent_milli: 2_000_000 }) }))!;
    expect(out.kind).toBe("exhausted");
    expect(out.detail).toContain("paused until Monday, Oct 26. On your key, credits pay for");
    expect(banner(meBot({ week: week({ spent_milli: 1_760_000 }) }))!.detail).not.toContain("your key");
  });
});

describe("the Send menu's ClaxBot line", () => {
  it("says whose key and how many credits are left, or why it is unavailable", () => {
    expect(availability(true, null, meBot())).toBe("Runs in Clax · instance key · 1,690 credits left");
    expect(availability(true, null, meBot({ key: { own: { set: true, last4: "4f2c", set_at: null, last_ok_at: null }, use_own: true } }))).toBe("Runs in Clax · your key · 1,690 credits left");
    expect(availability(false, "no_credits", meBot())).toBe("Weekly credits used · resets Mon Oct 26");
    expect(availability(false, "no_flag", null)).toBe(unavailableText("no_flag"));
    expect(availability(false, "not_configured", null)).toBe("Not set up on this Clax yet");
  });

  it("sends an admin to the Admin tab that fixes why it cannot run, anyone else to what is theirs", () => {
    expect(unavailableFix("not_configured", true)).toEqual({ text: "Set a model key under Settings → Model provider", href: "/settings/provider" });
    expect(unavailableFix("not_configured", false)).toEqual({ text: "Use your own Anthropic key under Settings → Your ClaxBot", href: "/settings/claxbot#me-k" });
    expect(unavailableFix("model_not_priced", true)?.href).toBe("/settings/pricing");
    expect(unavailableFix("no_flag", true)?.href).toBe("/settings/roles");
    expect(unavailableFix("no_credits", true)?.href).toBe("/settings/roles");
    expect(unavailableFix("no_credits", false)?.href).toBe("/settings/claxbot");
    expect([unavailableFix("model_not_priced", false), unavailableFix("no_flag", false), unavailableFix(null, true)]).toEqual([null, null, null]);
  });
});

describe("a run's events", () => {
  it("keeps a turn's words, reasoning and tool input apart, and says what a running turn is doing", () => {
    const at = "2026-10-21T14:02:00Z";
    let r = applyEvent(run(), { event: "step_started", data: { step: { n: 1, kind: "turn", status: "running", started_at: at, credits_milli: 0 } } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, type: "thinking", text: "Plan the map " } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, type: "thinking", text: "first." } });
    const t0 = Date.parse(at);
    expect(turnActivity(r.steps[0], t0 + 65_000)).toBe("Thinking · 1:05");
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, type: "text", text: "Drawing it." } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, type: "tool_input", text: '{"title":"Draw \\u201cthe\\u201d map","code":"const a = 1;\\nconst' } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, type: "tool_input", text: ' b = 2;' } });
    expect([r.steps[0].thinking, r.steps[0].text]).toEqual(["Plan the map first.", "Drawing it."]);
    expect(turnActivity(r.steps[0], t0 + 5_000)).toBe("Writing “Draw “the” map” · 2 lines · 0:05");
    // Before the title is written, and a delta from before typed deltas.
    expect(turnActivity({ input: '{"ti', started_at: at }, t0)).toBe("Writing a script · 0:00");
    expect(applyEvent(r, { event: "turn_delta", data: { step: 1, text: " Done." } }).steps[0].text).toBe("Drawing it. Done.");
    expect(tail("one two three four", 9)).toBe("…four");
    expect(tail("short", 9)).toBe("short");
  });

  it("builds the steps as their events arrive, and ends the run", () => {
    let r = run();
    const at = "2026-10-21T14:02:01Z";
    r = applyEvent(r, { event: "step_started", data: { step: { n: 1, kind: "turn", status: "running", started_at: at, credits_milli: 0 } } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, text: "Reading the page " } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 1, text: "first." } });
    expect(r.steps[0].text).toBe("Reading the page first.");
    r = applyEvent(r, { event: "step_done", data: { step: { n: 1, kind: "turn", status: "done", started_at: at, credits_milli: 0, text: "Reading the page first." }, run: { credits_milli: 2500 } } });
    // A call before its script's start starts it.
    r = applyEvent(r, { event: "call", data: { step: 2, call: { op: "files.read", target: "index.html", ok: true, ms: 12 } } });
    r = applyEvent(r, { event: "script", data: { step: 2, title: "Read the page", code: "return 1" } });
    r = applyEvent(r, { event: "call", data: { step: 2, call: { op: "db.list", target: "items", ok: true, ms: 22 } } });
    expect(r.steps[1]).toMatchObject({ kind: "script", status: "running", title: "Read the page", code: "return 1" });
    expect(r.steps[1].calls!.map(c => c.op)).toEqual(["files.read", "db.list"]);
    expect(r.credits_milli).toBe(2500);
    r = applyEvent(r, { event: "run_end", data: { run: { status: "stopped", reason: "credits_exhausted", draft: { url: "/a/7q3k9mzx2b4t?draft=claxbot" } } } });
    expect(r.status).toBe("stopped");
    expect(r.steps.every(s => s.status === "done")).toBe(true);
    expect(runStatus(r)).toBe("Stopped: this week's credits ran out");
  });

  it("parses the events it knows and skips the rest", () => {
    expect(parseEvent({ event: "call", data: '{"step":1,"call":{"op":"x","ok":true,"ms":1}}' })).toEqual({ event: "call", data: { step: 1, call: { op: "x", ok: true, ms: 1 } } });
    expect(parseEvent({ event: "ping", data: "{}" })).toBeNull();
    expect(parseEvent({ event: "call", data: "{" })).toBeNull();
  });

  it("says what the run spent", () => {
    expect(runCost(run())).toBe("This run: 2 credits · 38k tokens on your key");
    expect(runCost(run({ key_source: "instance", tokens: undefined, credits_milli: 760 }))).toBe("This run: 0.76 credits");
  });
});

describe("run event IDs", () => {
  it("order by step, then attempt, then seq, and never repeat across a step begun again", () => {
    expect(after("1.1:1", "1.1:0")).toBe(true);
    expect(after("1.1:0", "1.1:0")).toBe(false);
    expect(after("1.1:9", "2.1:0")).toBe(false);
    expect(after("10.1:0", "9.1:12")).toBe(true);
    // The step begun again after a crash: its new attempt comes after every event of the old one.
    expect(after("2.2:0", "2.1:40")).toBe(true);
    expect(after("2.1:41", "2.2:0")).toBe(false);
    expect(after("3.1:0", null)).toBe(true);
    // An event without an ID of this shape is taken as new.
    expect(after(undefined, "1.1:0")).toBe(true);
    expect(after("1:0", "1.1:0")).toBe(true);
  });

  it("takes a step begun again as starting over, not as more of the same step", () => {
    let r = run();
    r = applyEvent(r, { event: "step_started", data: { step: { n: 2, kind: "turn", status: "running", started_at: "a1", credits_milli: 0 } } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 2, text: "Half a thought" } });
    r = applyEvent(r, { event: "step_started", data: { step: { n: 2, kind: "turn", status: "running", started_at: "a2", credits_milli: 0 } } });
    r = applyEvent(r, { event: "turn_delta", data: { step: 2, text: "A whole one." } });
    expect(r.steps).toHaveLength(1);
    expect(r.steps[0]).toMatchObject({ started_at: "a2", text: "A whole one." });
  });
});

describe("the audit's words", () => {
  it("names ClaxBot acting for a person", () => {
    expect(actorLabel({ actor: { id: "agent:claxbot", name: null }, on_behalf_of: { id: "p_sam", name: "Sam" } })).toBe("ClaxBot for Sam");
    expect(actorLabel({ actor: { id: "p_alex", name: "Alex" }, on_behalf_of: null })).toBe("Alex");
  });

  it("starts each time range before now, the week on Monday at 00:00 UTC", () => {
    const now = new Date("2026-10-22T15:00:00Z");
    expect(rangeFrom("24h", now)).toBe("2026-10-21T15:00:00.000Z");
    expect(rangeFrom("week", now)).toBe("2026-10-19T00:00:00.000Z");
    expect(rangeFrom("week", new Date("2026-10-19T00:30:00Z"))).toBe("2026-10-19T00:00:00.000Z");
    expect(rangeFrom("all", now)).toBeUndefined();
  });
});

describe("the settings", () => {
  it("groups fields by their section and reads their inputs back in their own type", () => {
    const s = sections([
      { key: "serve.port", value: 7480, source: "default", editable: true, restart_required: true, env: "CLAX_SERVE__PORT" },
      { key: "bin", value: "/x", source: "file", editable: false, restart_required: false, env: null },
      { key: "bot.models.default", value: "m", source: "env:CLAX_BOT__MODELS__DEFAULT", editable: false, restart_required: false, env: "CLAX_BOT__MODELS__DEFAULT" },
    ]);
    expect(s.map(x => [x.name, x.fields.map(f => f.label)])).toEqual([["serve", ["port"]], ["general", ["bin"]], ["bot", ["models.default"]]]);
    expect(fromText("7481", 7480)).toBe(7481);
    expect(fromText("x", 7480)).toBe(7480);
    expect(fromText("[1,2]", [])).toEqual([1, 2]);
    expect(fromText("", null)).toBeNull();
    expect(fromText("a", "b")).toBe("a");
  });
});

describe("web calls", () => {
  it("words a fetch's source and a call's cost", () => {
    expect(webSource("comment")).toBe("from the sender's comment");
    expect(webSource("search #2")).toBe("from search #2");
    expect(webSource("page #5")).toBe("from page #5");
    expect(webSource(undefined)).toBeNull();
    expect(callCost(250)).toBe("0.25 credits");
    expect(callCost(1000)).toBe("1 credit");
    expect(callCost(0)).toBeNull();
    expect(callCost(undefined)).toBeNull();
  });

  it("reads the web prices from the pricing lines, per thousand requests", () => {
    expect(webPrices(WEB_LINES, 10_000)).toEqual({ search: { ceramic: 25_000, exa: 700_000, linkup: 500_000 }, fetch: 50_000 });
    expect(webPrices([], 10_000)).toEqual({ search: {}, fetch: null });
  });
});

describe("why a run stopped", () => {
  it("says every stop code in words, and an unknown one not at all", () => {
    const codes = ["cancelled", "request_gone", "disabled", "no_flag", "credits_exhausted", "not_configured", "model_not_priced", "run_cap", "key_rejected", "model_error", "model_timeout", "internal_error", "history_unreadable", "policy_error", "policy_history_2", "max_steps", "max_time"];
    for (const c of codes) {
      const w = stopWords(c);
      expect(w, c).toBeTruthy();
      expect(w, c).not.toContain("_");
    }
    expect(runStatus({ status: "stopped", reason: "model_error" })).toBe("Stopped: the model provider answered with an error");
    expect(runStatus({ status: "stopped", reason: "key_rejected" })).toBe("Stopped: the model provider refused the key");
    expect(runStatus({ status: "stopped", reason: "policy_history_3" })).toBe("Stopped: Clax could not put together what to send the model");
    expect(runStatus({ status: "stopped", reason: "some_new_code" })).toBe("Stopped");
    expect(runStatus({ status: "stopped", reason: "constructor" })).toBe("Stopped");
    expect(runStatus({ status: "stopped", reason: null })).toBe("Stopped");
    // On the instance's key, a model error is Clax's to say; on the person's own, the provider's.
    expect(runStatus({ status: "stopped", reason: "model_error", key_source: "instance" })).toBe("Stopped: ClaxBot could not use this Clax's model provider");
    expect(stopWords("model_error", "own")).toBe("the model provider answered with an error");
    expect(stopWords("key_rejected", "instance")).toBe("the model provider refused the key");
  });

  it("says a failure on the instance's key in Clax's own words, with the run to quote", () => {
    expect(instanceProblemWords({ busy: false }, "r_7k2")).toEqual({
      lead: "ClaxBot could not use this Clax's model provider.",
      body: "This is a problem with how this Clax is set up, not with your request. It has been recorded for the people who run this Clax; try again later.",
      ref: "If you ask an admin about it, quote run r_7k2.",
    });
    expect(instanceProblemWords({ busy: true }, "r_7k2")).toEqual({
      lead: "This Clax's model provider is busy.",
      body: "Try again in a little while.",
      ref: "If you ask an admin about it, quote run r_7k2.",
    });
    const step = { n: 3, kind: "stopped" as const, status: "done" as const, started_at: "x", credits_milli: 0, reason: "model_error" };
    const inst = run({ key_source: "instance" });
    expect(stepLine({ event: "step_done", data: { step: { ...step, instance_problem: { busy: false } } } }, inst)).toBe("Stopped: ClaxBot could not use this Clax's model provider.");
    expect(stepLine({ event: "step_done", data: { step: { ...step, instance_problem: { busy: true } } } }, inst)).toBe("Stopped: this Clax's model provider is busy.");
    expect(stepLine({ event: "run_end", data: { run: {} } }, { ...inst, status: "stopped", reason: "model_error" })).toBe("ClaxBot: Stopped: ClaxBot could not use this Clax's model provider.");
  });

  it("says a stopped step in the live line in words, with how the model call failed", () => {
    const step = { n: 3, kind: "stopped" as const, status: "done" as const, started_at: "x", credits_milli: 0, reason: "model_error" };
    expect(stepLine({ event: "step_done", data: { step } }, run())).toBe("Stopped: the model provider answered with an error.");
    expect(stepLine({ event: "step_done", data: { step: { ...step, error_detail: detail() } } }, run())).toBe("Stopped: the model provider answered with an error. The model provider refused the request as invalid.");
    expect(stepLine({ event: "step_done", data: { step: { ...step, kind: "failed", reason: "some_new_code" } } }, run())).toBe("Stopped.");
  });

  it("says a failed model call by its class, quotes the provider, and says what to do next", () => {
    const lead = (o: Parameters<typeof detail>[0]) => errorDetail(detail(o), false).lead;
    expect(lead({})).toBe("The model provider refused the request as invalid.");
    expect(lead({ class: "api", status: 404, type: "not_found_error" })).toBe("The model provider does not know this model, or this key may not use it.");
    expect(lead({ class: "api", status: 404, type: null })).toBe("The model provider does not know this model, or this key may not use it.");
    expect(lead({ class: "api", status: 500, type: "not_found_error" })).toBe("The model provider does not know this model, or this key may not use it.");
    expect(lead({ class: "auth", status: 401, type: "authentication_error" })).toBe("The model provider refused the key.");
    expect(lead({ class: "rate_limited", status: 429, type: "rate_limit_error" })).toBe("The model provider is limiting how often this key may call it.");
    expect(lead({ class: "overloaded", status: 529, type: "overloaded_error" })).toBe("The model provider is overloaded.");
    expect(lead({ class: "network", status: null, type: null })).toBe("The model provider could not be reached.");
    expect(lead({ class: "stream_cut", status: null })).toBe("The model provider's answer was cut off.");
    expect(lead({ class: "protocol" })).toBe("The model provider's answer could not be read.");
    expect(lead({ class: "unknown_event" })).toBe("The model provider's answer could not be read.");
    expect(lead({ class: "redirected", status: 302 })).toBe("The provider's base URL is not one Clax sends a key to.");
    expect(lead({ class: "bad_base_url", status: null })).toBe("The provider's base URL is not one Clax sends a key to.");
    expect(lead({ class: "api", status: 500, type: "api_error" })).toBe("The model provider answered with an error.");
    expect(lead({ class: "something_new", status: 500, type: "api_error" })).toBe("The model provider answered with an error.");

    // The invalid-request case: the provider's words quoted (a credit balance too low is shown, never read), its status, type and request.
    expect(errorDetail(detail(), true)).toEqual({
      lead: "The model provider refused the request as invalid.",
      said: "The provider said: “Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits.”",
      meta: "HTTP 400 · invalid_request_error · request req_011CXyz",
      next: { text: "Check the key and the model under Settings → Model provider.", href: "/settings/provider" },
    });
    expect(errorDetail(detail(), false).next).toEqual({ text: "An admin can check the key and the model under Settings → Model provider." });
    // Where no provider answered, the message is the client's own description.
    const net = errorDetail(detail({ class: "network", status: null, type: null, request_id: null, message: "connection refused" }), true);
    expect(net).toEqual({ lead: "The model provider could not be reached.", said: "Details: connection refused", meta: "", next: { text: "Try again in a little while." } });
    expect(errorDetail(detail({ class: "rate_limited", status: 429, type: "rate_limit_error", message: "  " }), false)).toMatchObject({ said: null, meta: "HTTP 429 · rate_limit_error · request req_011CXyz", next: { text: "Try again in a little while." } });
    for (const c of ["overloaded", "stream_cut"]) expect(errorDetail(detail({ class: c }), true).next).toEqual({ text: "Try again in a little while." });
    for (const c of ["auth", "api", "redirected", "bad_base_url", "protocol", "unknown_event"]) expect(errorDetail(detail({ class: c }), true).next!.href, c).toBe("/settings/provider");
    // On the person's own key, admin or not, they check it where they set it; what passes with time still just passes.
    for (const admin of [false, true]) {
      for (const c of ["auth", "invalid_request", "api", "redirected", "protocol"]) expect(errorDetail(detail({ class: c }), admin, "own").next, c).toEqual({ text: "Check your key under Settings → Your ClaxBot.", href: "/settings/claxbot#me-k" });
      expect(errorDetail(detail({ class: "rate_limited" }), admin, "own").next).toEqual({ text: "Try again in a little while." });
      expect(errorDetail(detail({ class: "auth" }), admin, "instance").next).toEqual(errorDetail(detail({ class: "auth" }), admin).next);
    }
  });
});
