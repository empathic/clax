import { describe, expect, it } from "vitest";
import { meBot, run, week } from "./fixtures";
import { actorLabel, after, applyEvent, availability, banner, credits, fromText, margins, meter, money, parseEvent, perUnitMilli, percent, rangeFrom, resetDay, runCost, runStatus, sections, tokens, unavailableFix, unavailableText } from "./model";

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
    expect(unavailableFix("not_configured", true)).toEqual({ text: "Set a model key under Admin → Provider", href: "/admin?tab=provider" });
    expect(unavailableFix("not_configured", false)).toEqual({ text: "Use your own Anthropic key under You", href: "/me" });
    expect(unavailableFix("model_not_priced", true)?.href).toBe("/admin?tab=pricing");
    expect(unavailableFix("no_flag", true)?.href).toBe("/admin?tab=people");
    expect(unavailableFix("no_credits", true)?.href).toBe("/admin?tab=people");
    expect(unavailableFix("no_credits", false)?.href).toBe("/me");
    expect([unavailableFix("model_not_priced", false), unavailableFix("no_flag", false), unavailableFix(null, true)]).toEqual([null, null, null]);
  });
});

describe("a run's events", () => {
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
