import { describe, expect, it } from "vitest";
import { CONSOLE_NAVIGATION } from "../../contracts";
import { WORK_ROUTES } from "./work/routes";
import { RESOURCE_ROUTES } from "./resources/routes";
import { BASELINE_ROUTES } from "./baselines/routes";
import { EXTENSION_ROUTES } from "./extensions/routes";

/**
 * The browser renders §8.3's sections, and it does not keep its own list.
 *
 * `CONSOLE_NAVIGATION` is generated from the same Rust definition `draftd`
 * serves, so these tests are about the *routes* — every authoritative view has
 * somewhere to go, and nothing here invents a view the authority never offered.
 * A section added in Rust fails here until the browser gives it a home, which
 * is the failure we want: the alternative is a tab that silently disappears.
 */
describe("project navigation", () => {
  it("is exactly the frozen §8.3 section list, in order", () => {
    expect(CONSOLE_NAVIGATION.PROJECT.map((section) => section.label)).toEqual([
      "Overview",
      "Work",
      "Resources",
      "Baselines",
      "Activity",
      "Providers",
      "Extensions",
    ]);
  });

  it("nests Tasks and Packs under Work rather than beside it", () => {
    const work = CONSOLE_NAVIGATION.PROJECT.find((section) => section.label === "Work");
    expect(work?.children).toEqual(["Tasks", "Packs"]);
  });

  it("gives every nested authoritative view a route", () => {
    const routes: Record<string, Record<string, { to: string }>> = {
      Work: WORK_ROUTES,
      Resources: RESOURCE_ROUTES,
      Baselines: BASELINE_ROUTES,
      Extensions: EXTENSION_ROUTES,
    };
    for (const section of CONSOLE_NAVIGATION.PROJECT) {
      if (section.children.length === 0) continue;
      const map = routes[section.label];
      expect(map, `no route map for the '${section.label}' section`).toBeDefined();
      for (const view of section.children) {
        expect(map[view], `no route for '${section.label} > ${view}'`).toBeDefined();
      }
      // And nothing extra: a route with no authoritative view behind it is a
      // tab the server never offered.
      expect(Object.keys(map).sort()).toEqual([...section.children].sort());
    }
  });

  it("keeps the retired consolidated sections out of the top level", () => {
    const labels = CONSOLE_NAVIGATION.PROJECT.map((section) => section.label);
    for (const retired of ["Graph", "Authorization", "Events", "Tools", "Observation"]) {
      expect(labels).not.toContain(retired);
    }
  });

  it("carries the fourteen frozen ChangePack views and no retired ones", () => {
    expect(CONSOLE_NAVIGATION.CHANGE.map((section) => section.label)).toEqual([
      "Summary",
      "Intent",
      "Scope",
      "Revisions",
      "Impact",
      "Representations",
      "Evidence",
      "Assessments",
      "Review",
      "Decisions",
      "Gates",
      "Promotion",
      "Receipts",
      "Recovery",
    ]);
    const serialized = JSON.stringify(CONSOLE_NAVIGATION);
    for (const retired of ["Submit", "Approvals", "Risk", "Rollback", "Verify", "Changes"]) {
      expect(serialized).not.toContain(retired);
    }
  });

  it("keeps a Baseline's three roots as three separate views", () => {
    const labels = CONSOLE_NAVIGATION.BASELINE.map((section) => section.label);
    expect(labels).toContain("State root");
    expect(labels).toContain("Evidence root");
    expect(labels).toContain("Coverage");
    // Publications is a view of a Baseline, never a rename of one.
    expect(labels).toContain("Publications");
    expect(labels).not.toContain("Publication baseline");
  });
});
