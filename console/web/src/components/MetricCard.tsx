import React from "react";
import { Icon, type IconName } from "../icons";

/**
 * The headline figure card used across Overview, Doctor and project screens.
 * `chart` is optional by design: a card with no real metric behind it simply
 * renders the figure and no decoration.
 */
export function MetricCard({
  label,
  icon,
  value,
  unit,
  note,
  noteTone,
  chart,
  link,
}: {
  label: string;
  icon?: IconName;
  value: React.ReactNode;
  unit?: string;
  note?: React.ReactNode;
  noteTone?: "success" | "warning" | "danger";
  chart?: React.ReactNode;
  link?: React.ReactNode;
}) {
  return (
    <article className="metric-card">
      <header>
        {icon && <Icon name={icon} size={16} />}
        <strong>{label}</strong>
      </header>
      <div className="metric-body">
        <div className="metric-figure">
          <strong>{value}</strong>
          {unit && <span>{unit}</span>}
        </div>
        {chart}
      </div>
      {note && <div className={`metric-note${noteTone ? ` ${noteTone}` : ""}`}>{note}</div>}
      {link}
    </article>
  );
}
