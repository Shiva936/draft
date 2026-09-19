import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    exclude: ["e2e/**", "node_modules/**", "../dist/**"],
    // No test here asserts latency; they assert behaviour. The runner timeout
    // is therefore only a hang detector, and it must not double as a measure of
    // how loaded the machine is. The suite finishes in under two seconds idle,
    // but the renderer-asset tests import real CodeMirror grammar packages on
    // demand, and transforming those the first time is fixture cost that scales
    // with whatever else the machine is doing. The default five seconds turned
    // that into a failure when the suite ran beside a full workspace build.
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
});
