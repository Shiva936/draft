
/**
 * Canonical payload rendered verbatim.
 *
 * The design samples specify only the pack Summary surface; the remaining pack
 * evidence tabs whose payload has no designed presentation fall back to this
 * readable, tokenised panel rather than an invented layout.
 */
export function DataView({ value }: { value: unknown }) {
  return (
    <pre className="data-view">{typeof value === "string" ? value : JSON.stringify(value, null, 2)}</pre>
  );
}
