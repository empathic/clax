import { defineConfig } from "vitest/config";
import preact from "@preact/preset-vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
export default defineConfig({
  // svelteTesting() resolves Svelte's browser build under jsdom and unmounts after each test.
  plugins: [preact(), svelte(), svelteTesting()],
  test: { environment: "jsdom", setupFiles: ["shell/test-setup.ts"], include: ["bridge/test/**/*.test.ts", "shell/src/**/*.test.{ts,tsx}"] },
});
