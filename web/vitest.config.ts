import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
export default defineConfig({
  // svelteTesting() resolves Svelte's browser build under jsdom and unmounts after each test.
  plugins: [svelte(), svelteTesting()],
  // The unit tests run the shell's clock with its test hook (shell/src/clock.ts).
  define: { __CLAX_TEST_CLOCK__: "true" },
  // The bridge loads its lazy parts from source here (parts-static.ts).
  resolve: { alias: { "clax-bridge-parts": fileURLToPath(new URL("./bridge/src/parts-static.ts", import.meta.url)) } },
  test: { environment: "jsdom", setupFiles: ["shell/test-setup.ts"], include: ["bridge/test/**/*.test.ts", "extension/src/**/*.test.ts", "shell/src/**/*.test.ts", "scripts/**/*.test.ts"] },
});
