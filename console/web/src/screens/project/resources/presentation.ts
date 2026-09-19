/**
 * How a resource would be presented, and by whom.
 *
 * Draft resolves the binding by specificity and refuses to break a tie: an
 * `ambiguous` result is a decision for a person, not something the Console
 * settles by picking the first candidate. `fallback` is the universal neutral
 * presentation, which is always available and needs no installed extension —
 * so an unknown domain always renders as *something*.
 */
export type PresentationBinding =
  | {
      resource_id: string;
      locator: { scheme: string; body: string };
      state: "resolved";
      presentation_id: string;
      engine: string;
      config: unknown;
      contributors: string[];
    }
  | {
      resource_id: string;
      locator: { scheme: string; body: string };
      state: "ambiguous";
      candidates: { presentation_id: string; engine: string; contributed_by: string }[];
      fallback: string;
    }
  | {
      resource_id: string;
      locator: { scheme: string; body: string };
      state: "fallback";
      engine: string;
    };

/** The binding for one resource, or `null` when nothing has been loaded yet. */
export function presentationFor(
  bindings: PresentationBinding[] | undefined,
  resourceId: string | null | undefined,
): PresentationBinding | null {
  if (!bindings || !resourceId) return null;
  return bindings.find((binding) => binding.resource_id === resourceId) ?? null;
}

/** The grammar a resolved `text_editor` binding asked for, if it named one. */
export function grammarOf(binding: PresentationBinding | null): string | null {
  if (!binding || binding.state !== "resolved" || binding.engine !== "text_editor") return null;
  const config = binding.config as { grammar?: unknown } | null;
  return typeof config?.grammar === "string" ? config.grammar : null;
}
