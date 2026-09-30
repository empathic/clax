import { defineConfig } from "vitest/config";
import preact from "@preact/preset-vite";
export default defineConfig({
  plugins: [preact()],
  test: { environment: "jsdom", setupFiles: ["shell/test-setup.ts"], include: ["bridge/test/**/*.test.ts", "shell/src/**/*.test.{ts,tsx}"] },
});
