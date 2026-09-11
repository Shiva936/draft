import { describe, expect, it } from "vitest";
import { classificationSummary, resourceClassLabels, resourceLabel } from "./format";
import { knownGrammars, loadGrammar } from "../screens/project/resources/viewers/text-grammars";
import { grammarOf } from "../screens/project/resources/presentation";

/**
 * Draft classifies resources; the Console renders that classification.
 *
 * These tests exist to keep a language table from creeping back into the
 * browser. What a resource *is* comes from the authoritative read model, which
 * gets it from an installed extension's contribution.
 */
describe("resource classification", () => {
  it("reports every class a resource carries, not one", () => {
    // The whole point of the set-valued model: a Rust file genuinely is both a
    // text document and a language source, and picking one would discard a
    // correct assignment.
    expect(
      resourceClassLabels({
        classes: ["draft.text.document/document", "draft.language.rust/source"],
      }),
    ).toEqual(["draft.text.document/document", "draft.language.rust/source"]);
  });

  it("does not guess a class from the locator body", () => {
    // With no extension installed there is no class, and inventing one here
    // would be the Console deciding domain semantics.
    expect(resourceClassLabels(undefined)).toEqual([]);
    expect(classificationSummary({ classes: [] })).toBe("Unclassified");
  });

  it("says so when installed extensions disagree about a class", () => {
    // Scoped: the surviving class is still shown, and the dispute is reported
    // alongside rather than blanking the whole classification.
    expect(
      classificationSummary({
        classes: ["draft.language.rust/source"],
        class_collisions: ["draft.text.document/document"],
      }),
    ).toBe("draft.language.rust/source (1 disputed)");
  });
});

describe("locator labelling", () => {
  it("shows the trailing segment for a file locator", () => {
    expect(resourceLabel({ scheme: "file", body: "src/main.rs" })).toBe("main.rs");
  });

  it("never splits a body belonging to another scheme", () => {
    // A catalog SKU is not a path, and treating it as one would show the user
    // a fragment of an identifier the Console does not understand.
    expect(resourceLabel({ scheme: "example.catalog", body: "sku/A-100" })).toBe("sku/A-100");
  });
});

describe("renderer asset selection", () => {
  it("loads the grammar the presentation binding named", async () => {
    // The extension says which grammar its resources want; the Console owns
    // what a grammar is. Neither knows the other's vocabulary.
    expect((await loadGrammar("json")).length).toBeGreaterThan(0);
    expect((await loadGrammar("rust")).length).toBeGreaterThan(0);
  });

  it("only offers grammars this build actually has", () => {
    // A bounded platform vocabulary, like the engine ids themselves.
    expect(knownGrammars()).toContain("rust");
    expect(knownGrammars()).not.toContain("draft.language.rust/source");
  });

  it("degrades to plain text rather than guessing", async () => {
    // No filename fallback at all: inferring a language from a suffix is
    // exactly the domain knowledge that belongs in an extension. An
    // unhighlighted resource is correct; a wrongly highlighted one is not.
    expect(await loadGrammar(null)).toEqual([]);
    expect(await loadGrammar("")).toEqual([]);
    expect(await loadGrammar("a-grammar-this-build-does-not-have")).toEqual([]);
  });

  it("takes no grammar from a presentation that resolved to nothing", () => {
    // A neutral fallback renders intrinsic facts, not a guessed language.
    expect(
      grammarOf({
        resource_id: "res_1",
        locator: { scheme: "file", body: "a" },
        state: "fallback",
        engine: "metadata_summary",
      }),
    ).toBeNull();
    // An ambiguous binding is a person's decision, never resolved by rendering.
    expect(
      grammarOf({
        resource_id: "res_1",
        locator: { scheme: "file", body: "a" },
        state: "ambiguous",
        candidates: [],
        fallback: "metadata_summary",
      }),
    ).toBeNull();
  });
});
