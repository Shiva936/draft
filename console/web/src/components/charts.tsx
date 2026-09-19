import React from "react";

/**
 * Chart primitives. Every one of these returns null when it has no real data
 * behind it, so a card degrades to its empty state rather than drawing a
 * decorative shape over nothing.
 */

export type Segment = { label: string; value: number; color: string };

export function Donut({
  segments,
  size = 62,
  thickness = 9,
  center,
}: {
  segments: Segment[];
  size?: number;
  thickness?: number;
  center?: { value: React.ReactNode; label?: string };
}) {
  const total = segments.reduce((sum, segment) => sum + Math.max(0, segment.value), 0);
  if (total <= 0) return null;
  const radius = (size - thickness) / 2;
  const circumference = 2 * Math.PI * radius;
  let offset = 0;
  return (
    <div className="donut-figure" style={{ width: size, height: size }}>
      <svg className="donut" width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
        <circle className="donut-track" cx={size / 2} cy={size / 2} r={radius} strokeWidth={thickness} />
        {segments.map((segment) => {
          const length = (Math.max(0, segment.value) / total) * circumference;
          const circle = (
            <circle
              key={segment.label}
              cx={size / 2}
              cy={size / 2}
              r={radius}
              stroke={segment.color}
              strokeWidth={thickness}
              strokeDasharray={`${length} ${circumference - length}`}
              strokeDashoffset={-offset}
            />
          );
          offset += length;
          return circle;
        })}
      </svg>
      {center && (
        <span className="donut-center">
          <strong>{center.value}</strong>
          {center.label && <span>{center.label}</span>}
        </span>
      )}
    </div>
  );
}

export function Legend({ segments }: { segments: Segment[] }) {
  return (
    <div className="legend">
      {segments.map((segment) => (
        <div key={segment.label}>
          <span className="swatch" style={{ background: segment.color }} />
          <strong>{segment.value}</strong>
          <span>{segment.label}</span>
        </div>
      ))}
    </div>
  );
}

/** Needs at least two real points; a single sample is not a trend. */
export function Sparkline({
  points,
  width = 120,
  height = 40,
}: {
  points: number[];
  width?: number;
  height?: number;
}) {
  if (points.length < 2) return null;
  const max = Math.max(...points);
  const min = Math.min(...points);
  const span = max - min || 1;
  const step = width / (points.length - 1);
  const coordinates = points.map((point, index) => {
    const x = index * step;
    const y = height - ((point - min) / span) * (height - 4) - 2;
    return `${x.toFixed(2)},${y.toFixed(2)}`;
  });
  const line = `M${coordinates.join("L")}`;
  const area = `${line}L${width},${height}L0,${height}Z`;
  return (
    <svg className="sparkline" width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      <path className="area" d={area} />
      <path d={line} />
    </svg>
  );
}

export function MiniBars({ points, color }: { points: number[]; color?: string }) {
  if (points.length === 0 || points.every((point) => point === 0)) return null;
  const max = Math.max(...points);
  return (
    <div className="mini-bars" aria-hidden="true">
      {points.map((point, index) => (
        <span
          key={index}
          style={{ height: `${Math.max(4, (point / max) * 100)}%`, background: color }}
        />
      ))}
    </div>
  );
}

export function Meter({ segments }: { segments: Segment[] }) {
  const total = segments.reduce((sum, segment) => sum + Math.max(0, segment.value), 0);
  if (total <= 0) return null;
  return (
    <div className="meter" aria-hidden="true">
      {segments.map((segment) => (
        <span
          key={segment.label}
          title={`${segment.label}: ${segment.value}`}
          style={{ width: `${(segment.value / total) * 100}%`, background: segment.color }}
        />
      ))}
    </div>
  );
}

/** Added/removed proportion bar used by the change change overview. */
export function DiffBar({ added, removed, width = 70 }: { added: number; removed: number; width?: number }) {
  const total = added + removed;
  if (total <= 0) return null;
  return (
    <span className="diff-bar" style={{ width }} aria-hidden="true">
      <span className="added" style={{ flex: added || 0.0001 }} />
      <span className="removed" style={{ flex: removed || 0.0001 }} />
    </span>
  );
}

export const chartColors = {
  success: "var(--color-status-success)",
  running: "var(--color-status-running)",
  review: "var(--color-status-review)",
  warning: "var(--color-status-warning)",
  danger: "var(--color-status-danger)",
  neutral: "var(--color-status-neutral)",
  accent: "var(--color-accent-default)",
} as const;
