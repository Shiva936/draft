import type { Extension } from "@codemirror/state";
import { extensionOf } from "../../../lib/format";

/**
 * Language modes are loaded on demand.
 *
 * Bundling all of them eagerly would roughly double the embedded Console
 * bundle, so each mode is a separate chunk fetched the first time a file of
 * that type is opened. Chunks are same-origin, which the gateway CSP allows.
 */
export async function loadLanguage(path: string): Promise<Extension[]> {
  const extension = extensionOf(path);
  switch (extension) {
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
    case "mjs":
    case "cjs": {
      const { javascript } = await import("@codemirror/lang-javascript");
      const typescript = extension === "ts" || extension === "tsx";
      return [javascript({ typescript, jsx: extension.endsWith("x") })];
    }
    case "json": {
      const { json } = await import("@codemirror/lang-json");
      return [json()];
    }
    case "rs": {
      const { rust } = await import("@codemirror/lang-rust");
      return [rust()];
    }
    case "py": {
      const { python } = await import("@codemirror/lang-python");
      return [python()];
    }
    case "md":
    case "markdown": {
      const { markdown } = await import("@codemirror/lang-markdown");
      return [markdown()];
    }
    case "css":
    case "scss": {
      const { css } = await import("@codemirror/lang-css");
      return [css()];
    }
    case "html":
    case "htm": {
      const { html } = await import("@codemirror/lang-html");
      return [html()];
    }
    case "sql": {
      const { sql } = await import("@codemirror/lang-sql");
      return [sql()];
    }
    case "xml":
    case "svg": {
      const { xml } = await import("@codemirror/lang-xml");
      return [xml()];
    }
    case "java": {
      const { java } = await import("@codemirror/lang-java");
      return [java()];
    }
    case "c":
    case "h":
    case "cc":
    case "cpp":
    case "hpp": {
      const { cpp } = await import("@codemirror/lang-cpp");
      return [cpp()];
    }
    default:
      return [];
  }
}
