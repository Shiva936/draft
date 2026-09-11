import { useMemo, useState } from "react";
import type { Resource } from "../../../contracts";
import { Icon } from "../../../icons";
import { FILE_SCHEME } from "../../../lib/format";

export type TreeNode = {
  name: string;
  /** The locator body this node stands for. Never parsed for meaning. */
  body: string;
  scheme: string;
  collection: boolean;
  protectedPath: boolean;
  bytes: number;
  children: TreeNode[];
};

/**
 * The nested view of a project's resources.
 *
 * Only `file`-scheme bodies are split into segments, because that adapter's
 * bodies genuinely are paths. Another scheme's resources are grouped under
 * their scheme and listed whole — a catalog SKU or a timeline event has no
 * containing folder, and inventing one would be the Console asserting a
 * hierarchy that does not exist.
 */
export function buildTree(resources: Resource[]): TreeNode[] {
  const root: TreeNode = {
    name: "",
    body: "",
    scheme: FILE_SCHEME,
    collection: true,
    protectedPath: false,
    bytes: 0,
    children: [],
  };
  const collections = new Map<string, TreeNode>([["", root]]);

  const ensureCollection = (body: string): TreeNode => {
    const existing = collections.get(body);
    if (existing) return existing;
    const segments = body.split("/");
    const name = segments[segments.length - 1];
    const parent = ensureCollection(segments.slice(0, -1).join("/"));
    const node: TreeNode = {
      name,
      body,
      scheme: FILE_SCHEME,
      collection: true,
      protectedPath: false,
      bytes: 0,
      children: [],
    };
    collections.set(body, node);
    parent.children.push(node);
    return node;
  };

  const sorted = [...resources].sort(
    (a, b) =>
      a.locator.scheme.localeCompare(b.locator.scheme) ||
      a.locator.body.localeCompare(b.locator.body),
  );

  // Non-file schemes get one group each, holding their resources flat.
  const otherSchemes = new Map<string, TreeNode>();

  for (const resource of sorted) {
    const { scheme, body } = resource.locator;
    if (scheme !== FILE_SCHEME) {
      let group = otherSchemes.get(scheme);
      if (!group) {
        group = {
          name: scheme,
          body: scheme,
          scheme,
          collection: true,
          protectedPath: false,
          bytes: 0,
          children: [],
        };
        otherSchemes.set(scheme, group);
      }
      group.children.push({
        name: body,
        body,
        scheme,
        collection: false,
        protectedPath: resource.protected,
        bytes: resource.content_size ?? 0,
        children: [],
      });
      continue;
    }

    const segments = body.split("/").filter(Boolean);
    if (segments.length === 0) continue;
    if (resource.form === "collection") {
      const node = ensureCollection(segments.join("/"));
      node.protectedPath = resource.protected;
      continue;
    }
    const parent = ensureCollection(segments.slice(0, -1).join("/"));
    parent.children.push({
      name: segments[segments.length - 1],
      body,
      scheme,
      collection: false,
      protectedPath: resource.protected,
      bytes: resource.content_size ?? 0,
      children: [],
    });
  }

  const sort = (node: TreeNode) => {
    node.children.sort((a, b) => {
      if (a.collection !== b.collection) return a.collection ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    node.children.forEach(sort);
  };
  sort(root);
  return [...root.children, ...otherSchemes.values()];
}

/**
 * Single-letter change markers, matching the legend below the tree.
 *
 * These are the contract's own neutral aspects, not a domain's vocabulary.
 */
export type ChangeAspect =
  | "added"
  | "removed"
  | "content_changed"
  | "metadata_changed"
  | "relocated"
  | "form_changed"
  | "attributes_changed";

const aspectLetters: Record<ChangeAspect, string> = {
  added: "A",
  removed: "R",
  content_changed: "C",
  metadata_changed: "M",
  relocated: "L",
  form_changed: "F",
  attributes_changed: "T",
};

const aspectLabels: Record<ChangeAspect, string> = {
  added: "Added",
  removed: "Removed",
  content_changed: "Content changed",
  metadata_changed: "Metadata changed",
  relocated: "Relocated",
  form_changed: "Form changed",
  attributes_changed: "Attributes changed",
};

export function ResourceTree({
  resources,
  changes,
  selected,
  filter,
  onSelect,
}: {
  resources: Resource[];
  changes: Map<string, ChangeAspect>;
  selected: string | null;
  filter: string;
  onSelect: (body: string) => void;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const tree = useMemo(() => buildTree(resources), [resources]);
  const needle = filter.trim().toLowerCase();

  // A search collapses the hierarchy to matching resources so results stay
  // visible. Matching is a plain substring test on the body, which is the only
  // operation valid on a locator the Console does not own.
  if (needle) {
    const matches = resources.filter(
      (resource) =>
        resource.form !== "collection" &&
        resource.locator.body.toLowerCase().includes(needle),
    );
    return (
      <div className="file-tree">
        {matches.length === 0 ? (
          <p className="muted" style={{ padding: "var(--space-3)" }}>
            No resource matches this search.
          </p>
        ) : (
          matches.map((resource) => (
            <button
              key={resource.resource_id}
              className={selected === resource.locator.body ? "tree-node selected" : "tree-node"}
              disabled={resource.protected}
              title={resource.protected ? "Protected control path" : resource.locator.body}
              onClick={() => onSelect(resource.locator.body)}
            >
              <Icon name="file" size={14} />
              <span className="name">{resource.locator.body}</span>
              <ChangeMarker aspect={changes.get(resource.locator.body)} />
            </button>
          ))
        )}
      </div>
    );
  }

  const toggle = (body: string) =>
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(body)) next.delete(body);
      else next.add(body);
      return next;
    });

  const render = (nodes: TreeNode[], depth: number): React.ReactNode =>
    nodes.map((node) => {
      const open = expanded.has(node.body);
      return (
        <div key={`${node.scheme}:${node.body}`}>
          <button
            className={selected === node.body ? "tree-node selected" : "tree-node"}
            style={{ paddingLeft: `calc(var(--space-2) + ${depth * 14}px)` }}
            disabled={!node.collection && node.protectedPath}
            title={node.protectedPath ? "Protected control path" : node.body}
            aria-expanded={node.collection ? open : undefined}
            onClick={() => (node.collection ? toggle(node.body) : onSelect(node.body))}
          >
            {node.collection ? (
              <Icon name="chevron-right" size={12} className={open ? "chevron open" : "chevron"} />
            ) : (
              <Icon name="chevron-right" size={12} className="chevron leaf-space" />
            )}
            <Icon
              name={node.collection ? (open ? "folder-open" : "folder") : "file"}
              size={14}
            />
            <span className="name">{node.name}</span>
            <ChangeMarker aspect={changes.get(node.body)} />
          </button>
          {node.collection && open && render(node.children, depth + 1)}
        </div>
      );
    });

  return <div className="file-tree">{render(tree, 0)}</div>;
}

function ChangeMarker({ aspect }: { aspect: ChangeAspect | undefined }) {
  if (!aspect) return null;
  return (
    <span className={`file-status ${aspect}`} title={aspectLabels[aspect]}>
      {aspectLetters[aspect]}
    </span>
  );
}

export function TreeLegend() {
  return (
    <div className="tree-legend">
      {(Object.keys(aspectLetters) as ChangeAspect[]).map((aspect) => (
        <span key={aspect}>
          <span className={`file-status ${aspect}`}>{aspectLetters[aspect]}</span>
          {aspectLabels[aspect]}
        </span>
      ))}
    </div>
  );
}

/**
 * Maps the authoritative status scan onto the tree's markers.
 *
 * The aspects arrive already named by the contract, so this only picks the most
 * significant one per resource to show in a single-letter slot. Nothing is
 * inferred from a string here.
 */
export function changeMap(status: unknown): Map<string, ChangeAspect> {
  const changes = (status as any)?.changes;
  const map = new Map<string, ChangeAspect>();
  if (!Array.isArray(changes)) return map;

  // Existence first, then shape, then content: the marker shows the aspect a
  // reader most needs to see when only one fits.
  const precedence: ChangeAspect[] = [
    "added",
    "removed",
    "relocated",
    "form_changed",
    "content_changed",
    "metadata_changed",
    "attributes_changed",
  ];

  for (const change of changes) {
    const body = change?.locator?.body;
    if (typeof body !== "string") continue;
    const aspects: string[] = Array.isArray(change.aspects) ? change.aspects : [];
    const chosen = precedence.find((aspect) => aspects.includes(aspect));
    if (chosen) map.set(body, chosen);
  }
  return map;
}
