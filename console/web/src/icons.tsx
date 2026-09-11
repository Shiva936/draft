/**
 * Draft Console icon set.
 *
 * Paths are derived from Lucide (https://lucide.dev), ISC/MIT licensed — see
 * console/web/LICENSES.md. They are checked in rather than pulled from a
 * package so the embedded Console build stays deterministic and offline.
 *
 * Every glyph is drawn on a 24x24 grid with `currentColor` strokes, so an icon
 * always inherits the colour of the control it belongs to.
 */

export type IconName = keyof typeof paths;
export type IconSize = 12 | 14 | 16 | 18 | 20 | 22 | 24 | 26 | 28;

const paths = {
  home: "M3 10.5 12 3l9 7.5M5 9.5V21h14V9.5",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z",
  inbox: "M3 12h5l1.5 3h5L16 12h5M3 12l3-7h12l3 7v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z",
  stethoscope: "M6 3v5a4 4 0 0 0 8 0V3M4 3h3M13 3h3M10 12v3a5 5 0 0 0 10 0v-2M20 9a2 2 0 1 1 0 4 2 2 0 0 1 0-4Z",
  plug: "M9 3v5M15 3v5M7 8h10v3a5 5 0 0 1-10 0V8ZM12 16v5",
  puzzle: "M9 3h3a1.5 1.5 0 0 1 0 3h2v3a1.5 1.5 0 0 0 3 0h3v10H9a1.5 1.5 0 0 0 0-3H6V9h3a1.5 1.5 0 0 0 0-3H6V3Z",
  settings: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.6 1.6 0 0 0 1.5-1 1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1Z",
  search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16ZM21 21l-4.3-4.3",
  plus: "M12 5v14M5 12h14",
  bell: "M18 8a6 6 0 1 0-12 0c0 7-3 9-3 9h18s-3-2-3-9M13.7 21a2 2 0 0 1-3.4 0",
  help: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17h.01",
  sun: "M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10ZM12 1v2M12 21v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M1 12h2M21 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4",
  moon: "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z",
  monitor: "M4 4h16a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1ZM8 21h8M12 16v5",
  "chevron-down": "m6 9 6 6 6-6",
  "chevron-right": "m9 6 6 6-6 6",
  "chevron-left": "m15 6-6 6 6 6",
  "chevron-up": "m6 15 6-6 6 6",
  "check-circle": "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM8.5 12.2l2.4 2.4 4.6-4.9",
  check: "m4.5 12.5 5 5 10-11",
  "alert-triangle": "M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0ZM12 9v4M12 17h.01",
  "x-circle": "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM15 9l-6 6M9 9l6 6",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 16v-4M12 8h.01",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 7v5l3 2",
  pause: "M9 4v16M15 4v16",
  play: "M7 4l12 8-12 8Z",
  file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8ZM14 3v5h5",
  "file-text": "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8ZM14 3v5h5M9 13h6M9 17h4",
  "folder-open": "M4 8h16l-2 11H6ZM4 8V6a2 2 0 0 1 2-2h3l2 2.5h5a2 2 0 0 1 2 2V8",
  package: "m12 2 9 5v10l-9 5-9-5V7ZM3 7l9 5 9-5M12 12v10",
  layers: "m12 3 9 5-9 5-9-5ZM3 13l9 5 9-5M3 17l9 5 9-5",
  refresh: "M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6",
  download: "M12 3v12M7.5 10.5 12 15l4.5-4.5M4 20h16",
  upload: "M12 15V3M7.5 7.5 12 3l4.5 4.5M4 20h16",
  shield: "M12 21s8-3.5 8-9V5.5L12 3 4 5.5V12c0 5.5 8 9 8 9Z",
  activity: "M3 12h4l2.5-7 5 15 2.5-8h4",
  "list-checks": "M4 6.5 5.5 8 8.5 5M4 17.5 5.5 19l3-3M12 7h9M12 17h9",
  "external-link": "M14 4h6v6M20 4l-8.5 8.5M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5",
  copy: "M9 9h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V10a1 1 0 0 1 1-1ZM5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1",
  trash: "M4 7h16M10 11v6M14 11v6M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13M9 7V4h6v3",
  pencil: "M4 20h4L20 8l-4-4L4 16ZM14.5 5.5l4 4",
  "more-vertical": "M12 6h.01M12 12h.01M12 18h.01",
  filter: "M3 5h18l-7 8v6l-4 2v-8Z",
  "arrow-up-down": "M7 4v16M4 7l3-3 3 3M17 20V4M14 17l3 3 3-3",
  star: "m12 3 2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9Z",
  x: "M18 6 6 18M6 6l12 12",
  "panel-left": "M4 4h16a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1ZM9.5 4v16",
  users: "M16 20v-1.5a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4V20M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8ZM22 20v-1.5a4 4 0 0 0-3-3.9M16 3.1a4 4 0 0 1 0 7.8",
  database: "M12 8c4.4 0 8-1.1 8-2.5S16.4 3 12 3 4 4.1 4 5.5 7.6 8 12 8ZM4 5.5v13C4 19.9 7.6 21 12 21s8-1.1 8-2.5v-13M4 12c0 1.4 3.6 2.5 8 2.5s8-1.1 8-2.5",
  terminal: "m5 7 5 5-5 5M12 19h7",
  zap: "M13 2 4 14h7l-1 8 9-12h-7Z",
  key: "M15.5 3a5.5 5.5 0 1 1-4.9 8L3 18.5V21h3v-2h2v-2h2l1.6-1.6A5.5 5.5 0 0 1 15.5 3ZM17 7.5h.01",
  user: "M12 12a4.5 4.5 0 1 0 0-9 4.5 4.5 0 0 0 0 9ZM3.5 21a8.5 8.5 0 0 1 17 0",
  "corner-down-left": "M18 5v6a3 3 0 0 1-3 3H5M9 10l-4 4 4 4",
  "rotate-ccw": "M3 12a9 9 0 1 0 2.6-6.4M3 3v6h6",
  "square-check": "M5 4h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1ZM8.5 12.2l2.4 2.4 4.6-4.9",
  square: "M5 4h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1Z",
  circle: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z",
  dot: "M12 14.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z",
  minus: "M5 12h14",
  eye: "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7-10-7-10-7ZM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z",
  "shield-check": "M12 21s8-3.5 8-9V5.5L12 3 4 5.5V12c0 5.5 8 9 8 9ZM8.8 11.8l2.3 2.3 4.1-4.4",
  scale: "M12 3v18M7 21h10M6 7h12M6 7 3 14h6ZM18 7l-3 7h6Z",
  wrench: "M15.5 3a5.5 5.5 0 0 0-5 7.7L3 18.2V21h2.8l7.5-7.5A5.5 5.5 0 1 0 15.5 3Z",
  hash: "M5 9h14M5 15h14M10 3 8 21M16 3l-2 18",
} as const;

/** Glyphs whose paths are closed shapes that should be filled, not stroked. */
const filled = new Set<IconName>(["play", "dot"]);

type IconProps = {
  name: IconName;
  size?: IconSize;
  className?: string;
  /** Supply when the icon carries meaning on its own; omit when it is decorative. */
  label?: string;
};

export function Icon({ name, size = 16, className, label }: IconProps) {
  const decorative = label === undefined;
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill={filled.has(name) ? "currentColor" : "none"}
      stroke={filled.has(name) ? "none" : "currentColor"}
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={decorative || undefined}
      role={decorative ? undefined : "img"}
      aria-label={label}
      focusable="false"
    >
      <path d={paths[name]} />
    </svg>
  );
}
