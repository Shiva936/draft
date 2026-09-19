import { defineConfig, devices } from "@playwright/test";
import { resolve } from "node:path";

const here = import.meta.dirname;

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000, toHaveScreenshot: { animations: "disabled", caret: "hide", maxDiffPixelRatio: 0.01 } },
  globalSetup: resolve(here, "e2e/global-setup.mjs"),
  outputDir: resolve(here, "test-results"),
  reporter: process.env.CI ? [["line"], ["html", { open: "never" }]] : "line",
  use: {
    ...devices["Desktop Chrome"],
    storageState: resolve(here, ".playwright-auth.json"),
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure"
  }
});
