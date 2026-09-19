import type { Extension } from "@codemirror/state";

/**
 * Renderer assets for the platform's `text_editor` presentation engine.
 *
 * Keys are the engine's own bounded grammar vocabulary, not contributed
 * identifiers. A presentation binding names one in its config — `{"grammar":
 * "rust"}` — and the Console loads the matching chunk. That keeps the direction
 * of knowledge right: the extension says which grammar its resources want, and
 * the Console owns what a grammar *is*, exactly as it owns what `text_editor`
 * is. Adding a grammar is additive platform work and changes no domain model.
 *
 * Nothing here decides anything. Classification, verification, capability
 * availability and every other semantic question is answered by Draft and
 * arrives in the read model. CodeMirror's identifiers stay in this file and
 * never appear in an extension manifest, the package format, Core or `draftd`.
 *
 * Modes load on demand: bundling them all eagerly would roughly double the
 * embedded Console bundle, so each is a separate same-origin chunk fetched the
 * first time a resource needs it.
 */
const grammars: Record<string, () => Promise<Extension[]>> = {
  javascript: async () => {
    const { javascript } = await import("@codemirror/lang-javascript");
    return [javascript({ typescript: true, jsx: true })];
  },
  json: async () => {
    const { json } = await import("@codemirror/lang-json");
    return [json()];
  },
  rust: async () => {
    const { rust } = await import("@codemirror/lang-rust");
    return [rust()];
  },
  python: async () => {
    const { python } = await import("@codemirror/lang-python");
    return [python()];
  },
  markdown: async () => {
    const { markdown } = await import("@codemirror/lang-markdown");
    return [markdown()];
  },
  css: async () => {
    const { css } = await import("@codemirror/lang-css");
    return [css()];
  },
  html: async () => {
    const { html } = await import("@codemirror/lang-html");
    return [html()];
  },
  sql: async () => {
    const { sql } = await import("@codemirror/lang-sql");
    return [sql()];
  },
  xml: async () => {
    const { xml } = await import("@codemirror/lang-xml");
    return [xml()];
  },
  java: async () => {
    const { java } = await import("@codemirror/lang-java");
    return [java()];
  },
  cpp: async () => {
    const { cpp } = await import("@codemirror/lang-cpp");
    return [cpp()];
  },
  /** Declared, so a binding that asks for one is honoured — as plain text. */
  plain: async () => [],
};

/** The grammar vocabulary this build can render. */
export function knownGrammars(): string[] {
  return Object.keys(grammars).sort();
}

/**
 * The grammar a resolved presentation asked for.
 *
 * An unresolved or neutral presentation, a binding with no grammar, and a
 * grammar this build does not have all produce plain text. That is deliberate:
 * an unhighlighted resource is correct, and one highlighted by guessing at its
 * name is not. There is no filename fallback, because inferring a language from
 * a suffix is exactly the domain knowledge that belongs in an extension.
 */
export async function loadGrammar(grammar: string | null | undefined): Promise<Extension[]> {
  if (!grammar) return [];
  const load = grammars[grammar];
  return load ? load() : [];
}
