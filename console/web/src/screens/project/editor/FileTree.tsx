import { useMemo, useState } from "react";
import type { EditorFile } from "../../../contracts";
import { Icon } from "../../../icons";

export type TreeNode = {
  name: string;
  path: string;
  directory: boolean;
  protectedPath: boolean;
  bytes: number;
  children: TreeNode[];
};

/** Builds the nested tree the editor renders from the flat canonical file list. */
export function buildTree(files: EditorFile[]): TreeNode[] {
  const root: TreeNode = { name: "", path: "", directory: true, protectedPath: false, bytes: 0, children: [] };
  const directories = new Map<string, TreeNode>([["", root]]);

  const ensureDirectory = (path: string): TreeNode => {
    const existing = directories.get(path);
    if (existing) return existing;
    const segments = path.split("/");
    const name = segments[segments.length - 1];
    const parent = ensureDirectory(segments.slice(0, -1).join("/"));
    const node: TreeNode = { name, path, directory: true, protectedPath: false, bytes: 0, children: [] };
    directories.set(path, node);
    parent.children.push(node);
    return node;
  };

  for (const file of [...files].sort((a, b) => a.path.localeCompare(b.path))) {
    const segments = file.path.split("/").filter(Boolean);
    if (segments.length === 0) continue;
    if (file.kind === "directory") {
      const node = ensureDirectory(segments.join("/"));
      node.protectedPath = file.protected;
      continue;
    }
    const parent = ensureDirectory(segments.slice(0, -1).join("/"));
    parent.children.push({
      name: segments[segments.length - 1],
      path: file.path,
      directory: false,
      protectedPath: file.protected,
      bytes: file.bytes,
      children: [],
    });
  }

  const sort = (node: TreeNode) => {
    node.children.sort((a, b) => {
      if (a.directory !== b.directory) return a.directory ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    node.children.forEach(sort);
  };
  sort(root);
  return root.children;
}

/** Single-letter canonical change markers, matching the legend below the tree. */
export type ChangeKind = "modified" | "added" | "deleted" | "renamed" | "untracked";

const changeLetters: Record<ChangeKind, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  untracked: "U",
};

export function FileTree({
  files,
  changes,
  selected,
  filter,
  onSelect,
}: {
  files: EditorFile[];
  changes: Map<string, ChangeKind>;
  selected: string | null;
  filter: string;
  onSelect: (path: string) => void;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const tree = useMemo(() => buildTree(files), [files]);
  const needle = filter.trim().toLowerCase();

  // A search collapses the hierarchy to matching files so results stay visible.
  if (needle) {
    const matches = files.filter((file) => file.kind !== "directory" && file.path.toLowerCase().includes(needle));
    return (
      <div className="file-tree">
        {matches.length === 0 ? (
          <p className="muted" style={{ padding: "var(--space-3)" }}>
            No file matches this search.
          </p>
        ) : (
          matches.map((file) => (
            <button
              key={file.path}
              className={selected === file.path ? "tree-node selected" : "tree-node"}
              disabled={file.protected}
              title={file.protected ? "Protected control path" : file.path}
              onClick={() => onSelect(file.path)}
            >
              <Icon name="file" size={14} />
              <span className="name">{file.path}</span>
              <ChangeMarker kind={changes.get(file.path)} />
            </button>
          ))
        )}
      </div>
    );
  }

  const toggle = (path: string) =>
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const render = (nodes: TreeNode[], depth: number): React.ReactNode =>
    nodes.map((node) => {
      const open = expanded.has(node.path);
      return (
        <div key={node.path}>
          <button
            className={selected === node.path ? "tree-node selected" : "tree-node"}
            style={{ paddingLeft: `calc(var(--space-2) + ${depth * 14}px)` }}
            disabled={!node.directory && node.protectedPath}
            title={node.protectedPath ? "Protected control path" : node.path}
            aria-expanded={node.directory ? open : undefined}
            onClick={() => (node.directory ? toggle(node.path) : onSelect(node.path))}
          >
            {node.directory ? (
              <Icon name="chevron-right" size={12} className={open ? "chevron open" : "chevron"} />
            ) : (
              <Icon name="chevron-right" size={12} className="chevron leaf-space" />
            )}
            <Icon name={node.directory ? (open ? "folder-open" : "folder") : "file"} size={14} />
            <span className="name">{node.name}</span>
            <ChangeMarker kind={changes.get(node.path)} />
          </button>
          {node.directory && open && render(node.children, depth + 1)}
        </div>
      );
    });

  return <div className="file-tree">{render(tree, 0)}</div>;
}

function ChangeMarker({ kind }: { kind: ChangeKind | undefined }) {
  if (!kind) return null;
  return (
    <span className={`file-status ${kind}`} title={kind}>
      {changeLetters[kind]}
    </span>
  );
}

export function TreeLegend() {
  return (
    <div className="tree-legend">
      {(Object.keys(changeLetters) as ChangeKind[]).map((kind) => (
        <span key={kind}>
          <span className={`file-status ${kind}`}>{changeLetters[kind]}</span>
          {kind.charAt(0).toUpperCase() + kind.slice(1)}
        </span>
      ))}
    </div>
  );
}

/** Maps the canonical status scan onto the tree's change markers. */
export function changeMap(status: unknown): Map<string, ChangeKind> {
  const changes = (status as any)?.changes;
  const map = new Map<string, ChangeKind>();
  if (!Array.isArray(changes)) return map;
  for (const change of changes) {
    const path = typeof change.path === "string" ? change.path : change.path?.path;
    if (typeof path !== "string") continue;
    const kind = String(change.change_kind ?? "").toLowerCase();
    map.set(
      path,
      kind.includes("add") || kind.includes("creat")
        ? "added"
        : kind.includes("delet") || kind.includes("remov")
          ? "deleted"
          : kind.includes("renam") || kind.includes("mov")
            ? "renamed"
            : kind.includes("untrack")
              ? "untracked"
              : "modified",
    );
  }
  return map;
}
