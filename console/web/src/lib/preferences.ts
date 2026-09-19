/**
 * Browser-stored Console preferences.
 *
 * Only presentation choices live here. Canonical Draft state — projects, tasks,
 * changes, identity, extension trust — is never cached in the browser.
 */
import { useCallback, useEffect, useState } from "react";

export type Theme = "light" | "dark" | "system";
export type StartupView = "last" | "overview" | "projects" | "inbox";

/**
 * Accent choices offered by the Settings screen, keyed by a stable id.
 *
 * Each entry carries a surface colour that white text is legible on, plus a
 * separate text colour per theme, so links and active navigation keep a WCAG AA
 * contrast ratio whichever accent is chosen.
 */
export const ACCENTS = {
  blue: { label: "Blue", surface: "#2563eb", hover: "#1f54c8", text: "#1d4ed8", darkText: "#8ab4ff" },
  violet: { label: "Violet", surface: "#7c3aed", hover: "#6931c9", text: "#6d28d9", darkText: "#bda2f7" },
  indigo: { label: "Indigo", surface: "#4f46e5", hover: "#433cc3", text: "#4338ca", darkText: "#a7a2f2" },
  teal: { label: "Teal", surface: "#0b8278", hover: "#096e66", text: "#0a7a71", darkText: "#5cbdb4" },
  green: { label: "Green", surface: "#12863d", hover: "#0f7234", text: "#117e39", darkText: "#5fbd80" },
  amber: { label: "Amber", surface: "#ae5f05", hover: "#945104", text: "#a75b05", darkText: "#e0a04b" },
  rose: { label: "Rose", surface: "#e11d48", hover: "#bf193d", text: "#c81940", darkText: "#f08da2" },
} as const;

export type AccentName = keyof typeof ACCENTS;

const KEYS = {
  theme: "draft-console-theme",
  accent: "draft-console-accent",
  startup: "draft-console-startup",
  restore: "draft-console-restore-changes",
  sidebar: "draft-console-sidebar-collapsed",
  starred: "draft-console-starred",
} as const;

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    /* Private windows and blocked site data are not an error worth surfacing. */
  }
}

/** A single persisted preference with a validated fallback. */
export function usePreference<T extends string>(
  key: keyof typeof KEYS,
  fallback: T,
  allowed: readonly T[],
): [T, (value: T) => void] {
  const storageKey = KEYS[key];
  const [value, setValue] = useState<T>(() => {
    const stored = read(storageKey) as T | null;
    return stored && allowed.includes(stored) ? stored : fallback;
  });
  const update = useCallback(
    (next: T) => {
      setValue(next);
      write(storageKey, next);
    },
    [storageKey],
  );
  return [value, update];
}

export function useBooleanPreference(
  key: keyof typeof KEYS,
  fallback: boolean,
): [boolean, (value: boolean) => void] {
  const storageKey = KEYS[key];
  const [value, setValue] = useState(() => {
    const stored = read(storageKey);
    return stored === null ? fallback : stored === "true";
  });
  const update = useCallback(
    (next: boolean) => {
      setValue(next);
      write(storageKey, String(next));
    },
    [storageKey],
  );
  return [value, update];
}

const THEMES: readonly Theme[] = ["light", "dark", "system"];

export function useTheme(): [Theme, (theme: Theme) => void] {
  const [theme, setTheme] = usePreference("theme", "system", THEMES);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  return [theme, setTheme];
}

const ACCENT_NAMES = Object.keys(ACCENTS) as AccentName[];

export function useAccent(): [AccentName, (accent: AccentName) => void] {
  const [accent, setAccent] = usePreference("accent", "blue", ACCENT_NAMES);
  useEffect(() => {
    const root = document.documentElement;
    const value = ACCENTS[accent];
    const apply = () => {
      const dark =
        root.dataset.theme === "dark" ||
        (root.dataset.theme !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches);
      root.style.setProperty("--color-accent-default", value.surface);
      root.style.setProperty("--color-accent-hover", dark ? value.surface : value.hover);
      root.style.setProperty("--color-accent-text", dark ? value.darkText : value.text);
      root.style.setProperty(
        "--color-accent-subtle",
        `color-mix(in srgb, ${value.surface} ${dark ? "14%" : "8%"}, var(--color-bg-surface))`,
      );
      root.style.setProperty(
        "--color-accent-border",
        `color-mix(in srgb, ${value.surface} ${dark ? "40%" : "34%"}, var(--color-bg-surface))`,
      );
    };
    apply();
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", apply);
    const observer = new MutationObserver(apply);
    observer.observe(root, { attributes: true, attributeFilter: ["data-theme"] });
    return () => {
      media.removeEventListener("change", apply);
      observer.disconnect();
    };
  }, [accent]);
  return [accent, setAccent];
}

const STARTUP_VIEWS: readonly StartupView[] = ["last", "overview", "projects", "inbox"];

export function useStartupView(): [StartupView, (view: StartupView) => void] {
  return usePreference("startup", "last", STARTUP_VIEWS);
}

export function useSidebarCollapsed(): [boolean, (value: boolean) => void] {
  return useBooleanPreference("sidebar", false);
}

export function useRestoreChanges(): [boolean, (value: boolean) => void] {
  return useBooleanPreference("restore", true);
}

/** Projects the user has starred, kept per browser like every other display preference. */
export function useStarredProjects(): [Set<string>, (workspaceId: string) => void] {
  const [starred, setStarred] = useState<Set<string>>(() => {
    try {
      const parsed = JSON.parse(read(KEYS.starred) ?? "[]");
      return new Set(Array.isArray(parsed) ? parsed.filter((entry) => typeof entry === "string") : []);
    } catch {
      return new Set();
    }
  });
  const toggle = useCallback((workspaceId: string) => {
    setStarred((current) => {
      const next = new Set(current);
      if (next.has(workspaceId)) next.delete(workspaceId);
      else next.add(workspaceId);
      write(KEYS.starred, JSON.stringify([...next]));
      return next;
    });
  }, []);
  return [starred, toggle];
}

/** The view `Console startup` should open when no deep link was given. */
export function startupPath(view: StartupView, lastPath: string | null): string | null {
  switch (view) {
    case "overview":
      return "/";
    case "projects":
      return "/projects";
    case "inbox":
      return "/inbox";
    default:
      return lastPath;
  }
}
