// Runes mode everywhere; TypeScript in <script lang="ts"> is handled by Svelte itself.
// A production build fails on a compiler warning: under Bazel (//web:dist_all)
// a build that passes prints nothing, so a warning would otherwise go unseen.
// The dev server and the tests print warnings and go on.
export default {
  compilerOptions: { runes: true },
  onwarn(warning, handler) {
    if (process.env.NODE_ENV === "production") {
      const at = warning.start ? `:${warning.start.line}:${warning.start.column}` : "";
      throw new Error(`${warning.filename ?? "svelte"}${at} ${warning.code}: ${warning.message}`);
    }
    handler(warning);
  },
};
