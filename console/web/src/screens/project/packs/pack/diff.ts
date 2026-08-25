/** Aggregates a unified diff into per-directory add/remove counts. */
export type DiffDirectory = { path: string; files: number; added: number; removed: number };
export type DiffSummary = {
  files: { path: string; added: number; removed: number }[];
  directories: DiffDirectory[];
  added: number;
  removed: number;
};

const EMPTY: DiffSummary = { files: [], directories: [], added: 0, removed: 0 };

export function parseDiff(text: unknown): DiffSummary {
  if (typeof text !== "string" || text.trim() === "") return EMPTY;

  const files: { path: string; added: number; removed: number }[] = [];
  let current: { path: string; added: number; removed: number } | null = null;

  for (const line of text.split("\n")) {
    if (line.startsWith("+++ ")) {
      const path = line.slice(4).replace(/^b\//, "").trim();
      if (path && path !== "/dev/null") {
        current = { path, added: 0, removed: 0 };
        files.push(current);
      }
      continue;
    }
    if (line.startsWith("--- ") || line.startsWith("diff ") || line.startsWith("@@")) continue;
    if (!current) continue;
    if (line.startsWith("+")) current.added += 1;
    else if (line.startsWith("-")) current.removed += 1;
  }

  if (files.length === 0) return EMPTY;

  const byDirectory = new Map<string, DiffDirectory>();
  for (const file of files) {
    const segments = file.path.split("/");
    const key = segments.length > 1 ? `${segments.slice(0, -1).join("/")}/` : ".";
    const entry = byDirectory.get(key) ?? { path: key, files: 0, added: 0, removed: 0 };
    entry.files += 1;
    entry.added += file.added;
    entry.removed += file.removed;
    byDirectory.set(key, entry);
  }

  return {
    files,
    directories: [...byDirectory.values()].sort((a, b) => b.files - a.files),
    added: files.reduce((sum, file) => sum + file.added, 0),
    removed: files.reduce((sum, file) => sum + file.removed, 0),
  };
}
