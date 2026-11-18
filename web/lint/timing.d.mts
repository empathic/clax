// The types of timing.mjs's plugin, for lint/timing.test.ts.
import type { RuleTester } from "oxlint/plugins-dev";

type Rule = Parameters<RuleTester["run"]>[1];

declare const plugin: {
  meta: { name: string };
  rules: Record<"timer-delay" | "literal-timeout" | "clock-arithmetic" | "disabled-test" | "counted-turns", Rule>;
};
export default plugin;
