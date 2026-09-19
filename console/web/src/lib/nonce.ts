import { EditorView } from "@codemirror/view";
import type { Extension } from "@codemirror/state";

/**
 * The gateway stamps a per-response CSP nonce into the served document so the
 * editor can register its stylesheet under `style-src 'self' 'nonce-…'`
 * without the policy needing `unsafe-inline`.
 */
export function cspNonce(): Extension[] {
  const value = document
    .querySelector<HTMLMetaElement>('meta[name="csp-nonce"]')
    ?.content?.trim();
  // An unsubstituted placeholder means the document was not served by the
  // gateway; there is no nonce to honour in that case.
  return value && value !== "__CSP_NONCE__" ? [EditorView.cspNonce.of(value)] : [];
}
