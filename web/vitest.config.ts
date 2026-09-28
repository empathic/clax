import { defineConfig } from "vitest/config";
export default defineConfig({ test: { environment: "jsdom", include: ["bridge/test/**/*.test.ts", "shell/src/**/*.test.ts"] } });
