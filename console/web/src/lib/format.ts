/** Presentation helpers shared by every Console screen. */

export type Tone = "success" | "running" | "review" | "warning" | "danger" | "neutral";
export type RiskLevel = "low" | "medium" | "high" | "critical";

/** Placeholder rendered wherever Draft has no value for a field. */
export const NONE = "—";

export function humanize(value: string | null | undefined): string {
  return String(value ?? "")
    .replaceAll("_", " ")
    .replaceAll("-", " ")
    .replaceAll(".", " ")
    // Canonical event names arrive in PascalCase as well as snake_case.
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

/** Maps a canonical Draft status word onto the shared status vocabulary. */
export function statusTone(value: string | null | undefined): Tone {
  const key = String(value ?? "").toLowerCase();
  if (!key) return "neutral";
  if (/fail|error|corrupt|conflict|reject|blocked|unavailable|offline|missing|cancel|overdue/.test(key)) return "danger";
  if (/warn|attention|stale|degraded|expired|untrusted|invalid|paused|drift/.test(key)) return "warning";
  if (/review|approv|submitted|pending/.test(key)) return "review";
  if (/running|progress|queued|reconnect|working|active/.test(key)) return "running";
  if (/healthy|ready|pass|verified|complete|connected|fresh|enabled|trusted|usable|success|done|ok\b/.test(key)) return "success";
  return "neutral";
}

const toneIcons = {
  success: "check-circle",
  running: "clock",
  review: "eye",
  warning: "alert-triangle",
  danger: "x-circle",
  neutral: "dot",
} as const;

export function toneIcon(tone: Tone) {
  return toneIcons[tone];
}

export function riskLevel(value: string | null | undefined): RiskLevel | null {
  const key = String(value ?? "").toLowerCase();
  if (key.includes("critical")) return "critical";
  if (key.includes("high")) return "high";
  if (key.includes("medium") || key.includes("moderate")) return "medium";
  if (key.includes("low")) return "low";
  return null;
}

/** Two-letter initials for an identity, or null when there is no real name. */
export function initials(name: string | null | undefined): string | null {
  const trimmed = String(name ?? "").trim();
  if (!trimmed || trimmed.toLowerCase() === "unknown") return null;
  const words = trimmed.split(/[\s_.-]+/).filter(Boolean);
  if (words.length === 0) return null;
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[words.length - 1][0]).toUpperCase();
}

export function relative(value: string | null | undefined): string {
  if (!value) return NONE;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return NONE;
  const seconds = Math.round((Date.now() - date.getTime()) / 1000);
  if (seconds < 0) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86_400)}d ago`;
}

/** Days remaining against a due date, negative once overdue. */
export function daysUntil(value: string | null | undefined): number | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return Math.ceil((date.getTime() - Date.now()) / 86_400_000);
}

export function isOverdue(value: string | null | undefined): boolean {
  const days = daysUntil(value);
  return days !== null && days < 0;
}

const dateFormat = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
  second: "2-digit",
});

export function formatDate(value: string | null | undefined): string {
  if (!value) return NONE;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? NONE : dateFormat.format(date);
}

export function formatDateTime(value: string | null | undefined): string {
  if (!value) return NONE;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? NONE : dateTimeFormat.format(date);
}

export function formatBytes(bytes: number | null | undefined): string {
  if (typeof bytes !== "number" || Number.isNaN(bytes)) return NONE;
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export function formatCount(value: number | null | undefined): string {
  return typeof value === "number" && Number.isFinite(value) ? value.toLocaleString() : NONE;
}

/**
 * A pack's revision is a canonical revision id, not a version number, so it is
 * rendered as a shortened identifier rather than a `v1`-style label.
 */
export function packRevisionLabel(revision: number | string | null | undefined): string | null {
  if (revision == null) return null;
  if (typeof revision === "number") return `v${revision}`;
  const trimmed = revision.trim();
  if (!trimmed) return null;
  return /^\d+$/.test(trimmed) ? `v${trimmed}` : shortDigest(trimmed);
}

export function shortDigest(value: string | null | undefined): string {
  if (!value) return NONE;
  return value.length > 20 ? `${value.slice(0, 12)}…${value.slice(-6)}` : value;
}

/** Reads a numeric field from an untyped canonical status map. */
export function numberFrom(value: Record<string, unknown> | undefined, ...keys: string[]): number | null {
  for (const key of keys) {
    const candidate = value?.[key];
    if (typeof candidate === "number" && Number.isFinite(candidate)) return candidate;
  }
  return null;
}

export function basename(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function dirname(path: string): string {
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  return parts.join("/");
}

const languageByExtension: Record<string, string> = {
  ts: "TypeScript",
  tsx: "TypeScript",
  js: "JavaScript",
  jsx: "JavaScript",
  mjs: "JavaScript",
  cjs: "JavaScript",
  json: "JSON",
  rs: "Rust",
  py: "Python",
  go: "Go",
  java: "Java",
  c: "C",
  h: "C",
  cc: "C++",
  cpp: "C++",
  hpp: "C++",
  css: "CSS",
  scss: "CSS",
  html: "HTML",
  htm: "HTML",
  xml: "XML",
  md: "Markdown",
  markdown: "Markdown",
  sql: "SQL",
  yml: "YAML",
  yaml: "YAML",
  toml: "TOML",
  sh: "Shell",
  bash: "Shell",
};

export function extensionOf(path: string): string {
  const name = basename(path);
  const index = name.lastIndexOf(".");
  return index > 0 ? name.slice(index + 1).toLowerCase() : "";
}

export function languageOf(path: string): string {
  return languageByExtension[extensionOf(path)] ?? "Plain text";
}

/** Detects the newline convention of a file so the editor status bar can report it. */
export function lineEnding(content: string): "LF" | "CRLF" {
  return content.includes("\r\n") ? "CRLF" : "LF";
}
