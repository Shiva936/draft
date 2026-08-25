import { Icon } from "../icons";
import { humanize, riskLevel, statusTone, toneIcon, type Tone } from "../lib/format";

/**
 * The shared status vocabulary: a canonical Draft status word rendered as a
 * tinted pill with its tone icon, so status never reads by colour alone.
 */
export function StatusBadge({
  value,
  tone,
  label,
  plain = false,
}: {
  value: string | null | undefined;
  tone?: Tone;
  label?: string;
  plain?: boolean;
}) {
  if (!value) return <span className="empty-cell">—</span>;
  const resolved = tone ?? statusTone(value);
  return (
    <span className={`pill ${resolved}${plain ? " plain" : ""}`}>
      <Icon name={toneIcon(resolved)} size={14} />
      {label ?? humanize(value)}
    </span>
  );
}

/** Risk level from canonical task/pack risk state. */
export function RiskBadge({ value }: { value: string | null | undefined }) {
  const level = riskLevel(value);
  if (!level) return <span className="empty-cell">—</span>;
  return (
    <span className={`pill risk-${level}`}>
      <Icon name={level === "low" ? "dot" : "alert-triangle"} size={14} />
      {humanize(level)}
    </span>
  );
}

export function StatusDot({ tone }: { tone: Tone }) {
  return <span className={`status-dot ${tone}`} />;
}
