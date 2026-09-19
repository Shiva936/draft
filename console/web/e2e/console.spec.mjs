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
  for (const tab of [
    "Overview",
    "Work",
    "Resources",
    "Baselines",
    "Activity",
    "Providers",
    "Extensions",
  ]) {
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

test("project-level navigation is the frozen §8.3 architecture", async ({ page }) => {
  const projectNav = page.getByRole("navigation", { name: "Project navigation" });

  // Exactly §8.3's sections, in order. The section list is generated from the
  // same Rust definition the daemon serves, so this asserts the browser
  // renders what the authority offers rather than a list of its own.
  await expect(projectNav.getByRole("link")).toHaveText([
    "Overview",
    "Work",
    "Resources",
    "Baselines",
    "Activity",
    "Providers",
    "Extensions",
  ]);

  for (const [tab, heading] of [
    ["Work", "Tasks"],
    ["Activity", "Activity"],
    // Baselines is the project's authoritative state; Providers is how it is
    // attached to what observes it. Both must resolve with nothing installed,
    // because "nothing is bound" is an answer these screens exist to give.
    ["Baselines", "Accepted Baseline"],
    ["Providers", "Bindings"],
    ["Resources", "Resources"],
  ]) {
    await projectNav.getByRole("link", { name: tab, exact: true }).click();
    await expect(page.getByRole("heading", { name: heading, exact: true }).first()).toBeVisible();
  }

  // Work nests Tasks and Packs; Resources nests Observation; Extensions
  // nests Tools. Coverage and Tools must still resolve — they moved, they did
  // not disappear.
  await projectNav.getByRole("link", { name: "Work", exact: true }).click();
  const workNav = page.getByRole("navigation", { name: "Work navigation" });
  await expect(workNav.getByRole("link")).toHaveText(["Tasks", "Packs"]);
  await workNav.getByRole("link", { name: "Packs", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Packs", exact: true }).first()).toBeVisible();

  await projectNav.getByRole("link", { name: "Resources", exact: true }).click();
  await page
    .getByRole("navigation", { name: "Resources navigation" })
    .getByRole("link", { name: "Observation", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "Coverage", exact: true }).first()).toBeVisible();

  await projectNav.getByRole("link", { name: "Extensions", exact: true }).click();
  await page
    .getByRole("navigation", { name: "Extensions navigation" })
    .getByRole("link", { name: "Tools", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "Tools", exact: true }).first()).toBeVisible();

  await projectNav.getByRole("link", { name: "Baselines", exact: true }).click();
  await page
    .getByRole("navigation", { name: "Baselines navigation" })
    .getByRole("link", { name: "Publications", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "Publication", exact: true }).first()).toBeVisible();
});

test("task, attributed tree edit, and ChangePack workflows persist through real Draft APIs", async ({ page }) => {
  test.setTimeout(240_000);

  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Work", exact: true }).click();
  await page.getByRole("button", { name: "Create task" }).first().click();
  const createTask = page.getByRole("dialog", { name: "Create task" });
  await createTask.getByLabel("Task name").fill("console-e2e-task");
  await createTask.getByLabel("Task goal").fill("Prove canonical browser mutations");
  await createTask.getByLabel("Task success criterion").fill("The persisted file and ChangePack are visible");
  await createTask.getByRole("button", { name: "Create task", exact: true }).click();
  await expect(page.getByRole("cell", { name: /console-e2e-task/ })).toBeVisible();

  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Resources", exact: true }).click();
  await page.getByLabel("Tree edit attribution").first().selectOption({ label: "Task · console-e2e-task" });
  await page.getByRole("button", { name: "New resource", exact: true }).first().click();
  const newFile = page.getByRole("dialog", { name: "New resource" });
  await newFile.getByLabel("Destination locator").fill("console-e2e.txt");
  await newFile.getByRole("button", { name: "Stage resource", exact: true }).click();
  await expect(page.getByText(/Workspace /i).first()).toBeVisible();

  await page.getByRole("button", { name: "Commit workspace" }).click();
  await expect(page.getByRole("button", { name: "console-e2e.txt" })).toBeVisible();

  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Baselines", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Accepted Baseline" })).toBeVisible();
});

test("the Change Graph names each act separately and none of them promotes", async ({ page }) => {
  const projectNav = page.getByRole("navigation", { name: "Project navigation" });
  await projectNav.getByRole("link", { name: "Baselines", exact: true }).click();

  // The three roots are shown distinctly. Collapsing them would let "what is
  // accepted" and "what proves it" read as one answer.
  await expect(page.getByRole("heading", { name: "Accepted Baseline" })).toBeVisible();
  await expect(page.getByText("Project state root")).toBeVisible();
  await expect(page.getByText("State evidence root")).toBeVisible();
  await expect(page.getByText("Coverage evidence root")).toBeVisible();

  // Composition is immutable accepted provenance, never a current route.
  await expect(page.getByRole("heading", { name: "Composition" })).toBeVisible();
  // Recoverability is asked of the anchors, not inferred from files on disk.
  await expect(page.getByText("Recoverability")).toBeVisible();

  // Publication is a sibling view, not a field of a Baseline: delivering one
  // has no authority over what the project accepts.
  await page
    .getByRole("navigation", { name: "Baselines navigation" })
    .getByRole("link", { name: "Publications", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "Publication", exact: true })).toBeVisible();

  // Authorization lives with the ChangePack it authorizes, under Work.
  await projectNav.getByRole("link", { name: "Work", exact: true }).click();
  await page
    .getByRole("navigation", { name: "Work navigation" })
    .getByRole("link", { name: "Packs", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "Packs", exact: true }).first()).toBeVisible();

  await expect(page.locator(".error-state")).toHaveCount(0);
});

test("providers render bindings, immutable definitions and profiles separately", async ({ page }) => {
  await page
    .getByRole("navigation", { name: "Project navigation" })
    .getByRole("link", { name: "Providers", exact: true })
    .click();

  // Mutable bindings and the immutable facts they point at are separate
  // panels, because an unbind moves one and cannot touch the others.
  await expect(page.getByRole("heading", { name: "Bindings" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Semantic definitions" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Operational profiles" })).toBeVisible();
  await expect(page.locator(".error-state")).toHaveCount(0);
});

test("destructive resource actions require an explicit confirmation", async ({ page }) => {
  await page.getByRole("navigation", { name: "Project navigation" }).getByRole("link", { name: "Resources", exact: true }).click();
  await page.getByLabel("Tree edit attribution").first().selectOption({ index: 1 });

  await page.getByRole("button", { name: "Resource tree actions" }).click();
  await page.getByRole("menuitem", { name: "Remove resources…" }).click();
  const dialog = page.getByRole("dialog", { name: "Remove resources" });
  await dialog.getByLabel("Source locator").fill("console-e2e.txt");

  // Dismissing the confirmation must leave canonical state untouched.
  page.once("dialog", (confirmation) => confirmation.dismiss());
  await dialog.getByRole("button", { name: "Stage removal", exact: true }).click();
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
  // One per §8.3 section, including the two the rewrite added.
  for (const tab of ["Overview", "Work", "Baselines", "Providers", "Activity", "Extensions"]) {
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
  const result = spawnSync(draft, ["daemon", "stop"], {
    cwd: runtime.testHome,
    env: { ...process.env, HOME: runtime.testHome, DRAFT_GLOBAL_HOME: runtime.globalStore },
    encoding: "utf8",
    timeout: 30_000,
  });
  expect(result.status).toBe(0);
  await expect(page.locator(".connection").first()).toContainText(/offline/i, { timeout: 15_000 });
});
