import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const webRoot = resolve(import.meta.dirname, "..");
const runtime = JSON.parse(readFileSync(resolve(webRoot, ".playwright-runtime.json"), "utf8"));
const repoRoot = resolve(webRoot, "../..");
const draft = resolve(repoRoot, "target", "debug", process.platform === "win32" ? "draft.exe" : "draft");

/** `--color-border-default` in each theme, used to fill masked regions. */
const MASK_LIGHT = "#e2e8f0";
const MASK_DARK = "#1f2a3c";


test.describe.configure({ mode: "serial" });

/** Switches the stored display preference and reloads so it applies everywhere. */
async function useTheme(page, theme) {
  await page.evaluate((value) => window.localStorage.setItem("draft-console-theme", value), theme);
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
}

test.beforeEach(async ({ page }) => {
  await page.goto(runtime.origin);
  await expect(page.getByRole("heading", { name: runtime.projectName })).toBeVisible();
});

test("real canonical project navigation and keyboard workflow", async ({ page }) => {
  await page.getByRole("navigation", { name: "System navigation" }).getByRole("link", { name: "Overview" }).click();
  await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();

  await page.getByRole("navigation", { name: "System navigation" }).getByRole("link", { name: "Projects" }).click();
  await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();

  await page.getByRole("cell", { name: runtime.projectName }).first().click();
  await page.getByRole("button", { name: "Open project" }).click();
  await expect(page.getByRole("heading", { name: runtime.projectName })).toBeVisible();

  const projectNav = page.getByRole("navigation", { name: "Project navigation" });
  for (const tab of ["Overview", "Tasks", "Editor / Files", "Events", "Packs"]) {
    await expect(projectNav.getByRole("link", { name: tab, exact: true })).toBeVisible();
  }

  await page.keyboard.press(process.platform === "darwin" ? "Meta+K" : "Control+K");
  await expect(page.getByRole("dialog", { name: "Search and commands" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Search and commands" })).toBeHidden();
});

test("project switching moves between the registry and a project context", async ({ page }) => {
  const switcher = page.getByRole("button", { name: "Selected project" });
  await switcher.click();
  await page.getByRole("menuitem", { name: "All projects" }).click();
  await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();

  await switcher.click();
  await page.getByRole("menuitem", { name: runtime.projectName }).click();
  await expect(page.getByRole("heading", { name: runtime.projectName })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Project navigation" })).toBeVisible();
});

test("project-level navigation reaches every canonical section", async ({ page }) => {
  const projectNav = page.getByRole("navigation", { name: "Project navigation" });
  for (const [tab, heading] of [
    ["Tasks", "Tasks"],
    ["Events", "Events"],
    ["Packs", "Packs"],
  ]) {
    await projectNav.getByRole("link", { name: tab, exact: true }).click();
    await expect(page.getByRole("heading", { name: heading, exact: true }).first()).toBeVisible();
  }
  await projectNav.getByRole("link", { name: "Editor / Files", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Files" })).toBeVisible();
});

test("task, attributed tree edit, and pack workflows persist through real Draft APIs", async ({ page }) => {
  test.setTimeout(240_000);

  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Tasks", exact: true }).click();
  await page.getByRole("button", { name: "Create task" }).first().click();
  const createTask = page.getByRole("dialog", { name: "Create task" });
  await createTask.getByLabel("Task name").fill("console-e2e-task");
  await createTask.getByLabel("Task goal").fill("Prove canonical browser mutations");
  await createTask.getByLabel("Task success criterion").fill("The persisted file and pack are visible");
  await createTask.getByRole("button", { name: "Create task", exact: true }).click();
  await expect(page.getByRole("cell", { name: /console-e2e-task/ })).toBeVisible();

  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Editor / Files", exact: true }).click();
  await page.getByLabel("Tree edit attribution").first().selectOption({ label: "Task · console-e2e-task" });
  await page.getByRole("button", { name: "New file", exact: true }).first().click();
  const newFile = page.getByRole("dialog", { name: "New file" });
  await newFile.getByLabel("Destination path").fill("console-e2e.txt");
  await newFile.getByRole("button", { name: "Stage file", exact: true }).click();
  await expect(page.getByText(/Session /i).first()).toBeVisible();

  await page.getByRole("button", { name: "Commit to context" }).click();
  await expect(page.getByRole("button", { name: "console-e2e.txt" })).toBeVisible();

  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Packs", exact: true }).click();
  await page.getByRole("button", { name: "New pack" }).click();
  const createPack = page.getByRole("dialog", { name: "Create pack" });
  await createPack.getByLabel("New pack name").fill("console-e2e-pack");
  await createPack.getByRole("button", { name: "Create pack", exact: true }).click();
  await expect(page.getByRole("button", { name: /console-e2e-pack/ })).toBeVisible();
});

test("pack lifecycle surfaces only the actions core reports as valid", async ({ page }) => {
  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Packs", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Pack actions" })).toBeVisible();

  // Evidence tabs are canonical and must all resolve without a client error.
  for (const tab of ["Diff", "Verify", "Risk", "Review", "Approvals", "Submit", "Receipts", "Rollback", "Summary"]) {
    await page.getByRole("navigation", { name: "Pack views" }).getByRole("link", { name: tab, exact: true }).click();
    await expect(page.getByRole("navigation", { name: "Pack views" }).getByRole("link", { name: tab, exact: true })).toHaveClass(/active/);
    await expect(page.locator(".error-state")).toHaveCount(0);
  }
});

test("destructive file actions require an explicit confirmation", async ({ page }) => {
  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Editor / Files", exact: true }).click();
  await page.getByLabel("Tree edit attribution").first().selectOption({ index: 1 });

  await page.getByRole("button", { name: "File tree actions" }).click();
  await page.getByRole("menuitem", { name: "Delete tree…" }).click();
  const dialog = page.getByRole("dialog", { name: "Delete tree" });
  await dialog.getByLabel("Source path").fill("console-e2e.txt");

  // Dismissing the confirmation must leave canonical state untouched.
  page.once("dialog", (confirmation) => confirmation.dismiss());
  await dialog.getByRole("button", { name: "Stage deletion", exact: true }).click();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("button", { name: "console-e2e.txt" })).toBeVisible();
});

test("empty, loading, and error states are explicit rather than blank", async ({ page }) => {
  await page.getByRole("navigation", { name: "System navigation" }).getByRole("link", { name: "Extensions" }).click();
  await expect(page.getByText("No extensions are installed.")).toBeVisible();

  await page.getByRole("tab", { name: /Catalog sources/ }).click();
  await expect(page.getByText("No extension sources are configured.")).toBeVisible();

  // A failing canonical read renders a retryable error state, never a blank pane.
  await page.route("**/api/v1/doctor", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ schema_version: 1, error: { code: "DAEMON_UNAVAILABLE", message: "draftd is offline", details: null } }),
    }),
  );
  await page.getByRole("navigation", { name: "System navigation" }).getByRole("link", { name: "Doctor" }).click();
  await expect(page.getByText("Draft daemon is offline")).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await page.unroute("**/api/v1/doctor");
});

test("desktop light and dark compositions are deterministic", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole("navigation", { name: "System navigation" }).getByRole("link", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();

  const dynamicIdentity = page.locator(".definitions dd").filter({ hasText: /act_|key_|draft-console-playwright-/ });

  await useTheme(page, "light");
  await expect(page).toHaveScreenshot("console-settings-light.png", {
    fullPage: true,
    mask: [dynamicIdentity],
    maskColor: MASK_LIGHT,
  });

  await useTheme(page, "dark");
  await expect(page).toHaveScreenshot("console-settings-dark.png", {
    fullPage: true,
    mask: [dynamicIdentity],
    maskColor: MASK_DARK,
  });

  await useTheme(page, "light");
});

test("tablet and mobile layouts retain fixed information architecture", async ({ page }) => {
  const dynamic = page.locator("tbody td small, tbody td.muted");

  await page.setViewportSize({ width: 900, height: 1100 });
  await page.getByRole("navigation", { name: "System navigation" }).getByRole("link", { name: "Projects" }).click();
  await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();
  await expect(page).toHaveScreenshot("console-projects-tablet.png", {
    fullPage: true,
    mask: [dynamic],
    maskColor: MASK_LIGHT,
  });

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole("navigation", { name: "System navigation" }).getByRole("link")).toHaveCount(6);
  await expect(page).toHaveScreenshot("console-projects-mobile.png", {
    fullPage: true,
    mask: [dynamic],
    maskColor: MASK_LIGHT,
  });
});

test("every system screen has no serious accessibility violations", async ({ page }) => {
  const nav = page.getByRole("navigation", { name: "System navigation" });
  for (const [link, heading] of [
    ["Overview", "Overview"],
    ["Projects", "Projects"],
    ["Inbox", "Inbox"],
    ["Doctor", "Doctor"],
    ["Extensions", "Extensions"],
    ["Settings", "Settings"],
  ]) {
    await nav.getByRole("link", { name: link }).click();
    await expect(page.getByRole("heading", { name: heading, exact: true }).first()).toBeVisible();
    const results = await new AxeBuilder({ page }).analyze();
    expect(
      results.violations.filter((violation) => ["serious", "critical"].includes(violation.impact)),
      `${link} accessibility violations`,
    ).toEqual([]);
  }
});

test("project screens have no serious accessibility violations", async ({ page }) => {
  const projectNav = page.getByRole("navigation", { name: "Project navigation" });
  for (const tab of ["Overview", "Tasks", "Events", "Packs"]) {
    await projectNav.getByRole("link", { name: tab, exact: true }).click();
    await page.waitForTimeout(500);
    const results = await new AxeBuilder({ page }).analyze();
    expect(
      results.violations.filter((violation) => ["serious", "critical"].includes(violation.impact)),
      `${tab} accessibility violations`,
    ).toEqual([]);
  }
});

test("daemon loss transitions the shell to its reconnectable offline state", async ({ page }) => {
  const result = spawnSync(draft, ["service", "stop"], {
    cwd: runtime.testHome,
    env: { ...process.env, HOME: runtime.testHome, DRAFT_GLOBAL_HOME: runtime.globalStore },
    encoding: "utf8",
    timeout: 30_000,
  });
  expect(result.status).toBe(0);
  await expect(page.locator(".connection").first()).toContainText(/offline/i, { timeout: 15_000 });
});
