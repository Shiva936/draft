import { describe, expect, it } from "vitest";
import extensionsSource from "./Extensions.tsx?raw";
import trustSourcesSource from "./TrustSourcesPanel.tsx?raw";

/**
 * A secondary regression detector, not the primary proof.
 *
 * `components/actions.test.tsx` proves behaviourally that a control appears
 * only because `draftd` issued it, using deliberately contradictory fixtures.
 * This file catches the one thing a behavioural test cannot: a *new* control
 * added later that bypasses the action model entirely.
 *
 * It deliberately does not ban reading `enabled`, `trusted`, `builtin`,
 * `pending_authorization` or `authorized_permissions`. Those are authoritative
 * state and the Console is expected to display them. What must not come back is
 * a mutation control whose existence or availability the Console decided.
 */
const screens = [
  { name: "Extensions.tsx", source: extensionsSource },
  { name: "TrustSourcesPanel.tsx", source: trustSourcesSource },
];

describe("the Extensions screen renders authority rather than deciding it", () => {
  it("routes every mutation through the generic action renderer", () => {
    for (const { name, source } of screens) {
      // The only way to reach `console.action.invoke` from this screen is
      // through `ActionButton`, which takes a server-issued presentation.
      expect(source, name).toContain("ActionButton");
      // No screen-local mutation helper: no direct posts, and no bespoke
      // confirm-then-POST path of the kind the REST flow used.
      expect(source, name).not.toMatch(/\bmutate\s*[(<]/);
      expect(source, name).not.toMatch(/window\.confirm/);
      expect(source, name).not.toMatch(/"\/api\/v1\/extensions\/[^"]*\/\$\{/);
    }
  });

  it("keeps the eligibility rules that used to live here deleted", () => {
    const forbidden: [RegExp, string][] = [
      [/isInert\s*\(/, "a local re-derivation of withheld capability"],
      [/enabled\s*\?\s*"Disable"/, "the client choosing between Enable and Disable"],
      [/current\s*\?\s*"update"\s*:\s*"install"/, "the client choosing between Update and Install"],
      [/disabled=\{!\s*\w*[Tt]rusted/, "the client gating refresh on trust"],
      [/builtin\s*===\s*true/, "the client gating removal on built-in state"],
      [/startsWith\("https:\/\//, "the client validating a source location"],
      [/missing_permissions\s*\)\s*=>/, "the client proposing a permission set"],
    ];
    for (const { name, source } of screens) {
      for (const [pattern, description] of forbidden) {
        expect(pattern.test(source), `${name}: ${description}`).toBe(false);
      }
    }
  });

  it("still displays authoritative state as status", () => {
    const [extensions, sources] = screens;
    // Presentation is allowed and expected; it simply never selects a control.
    expect(extensions.source).toContain("pending_authorization");
    expect(sources.source).toContain("item.trusted");
    expect(sources.source).toContain("item.source.builtin");
  });
});
