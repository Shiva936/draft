import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../api";
import { Icon } from "../../icons";
import {
  Definitions,
  FilterSelect,
  PageHeader,
  Panel,
  PanelHeader,
  SearchField,
  Tabs,
  Toolbar,
} from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { DetailDrawer } from "../../components/DetailDrawer";
import { NextAction } from "../../components/NextAction";
import { EmptyState, InlineError, QueryState } from "../../components/states";
import { NONE, formatDateTime, humanize, relative, statusTone } from "../../lib/format";

/**
 * Unified attention across every registered project. Items come from two real
 * sources: per-project canonical inbox entries and global notification records.
 */
type Item = {
  key: string;
  /** Only notification records carry an id that mutations can address. */
  notificationId: string | null;
  kind: string;
  title: string;
  message: string;
  severity: string | null;
  status: string;
  workspaceId: string | null;
  subjectId: string;
  nextAction: { label: string; kind: string; workspaceId: string | null; subjectId: string | null } | null;
  createdAt: string | null;
  updatedAt: string | null;
  correlationId: string | null;
  read: boolean;
  resolved: boolean;
  occurrences: any[];
};

const SEVERITY_ORDER = ["critical", "high", "medium", "low", "info"];

export function Inbox() {
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: ["inbox"], queryFn: () => api<any>("/api/v1/inbox") });
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState("all");
  const [project, setProject] = useState("all");
  const [severity, setSeverity] = useState("all");
  const [search, setSearch] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const action = useMutation({
    mutationFn: ({ id, verb }: { id: string; verb: string }) =>
      mutate(`/api/v1/inbox/${encodeURIComponent(id)}/${verb}`, {}),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["inbox"] }),
  });

  const items = useMemo(() => normalise(query.data), [query.data]);

  const projects = useMemo(
    () => [...new Set(items.map((item) => item.workspaceId).filter(Boolean))] as string[],
    [items],
  );

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return items.filter((item) => {
      if (tab === "attention" && !(item.severity && ["critical", "high"].includes(item.severity))) return false;
      if (tab === "waiting" && item.read) return false;
      if (tab === "updates" && !item.read) return false;
      if (project !== "all" && item.workspaceId !== project) return false;
      if (severity !== "all" && item.severity !== severity) return false;
      if (needle && !`${item.title} ${item.message} ${item.subjectId}`.toLowerCase().includes(needle)) return false;
      return true;
    });
  }, [items, tab, project, severity, search]);

  const groups = useMemo(() => {
    const map = new Map<string, Item[]>();
    for (const item of filtered) {
      const list = map.get(item.kind) ?? [];
      list.push(item);
      map.set(item.kind, list);
    }
    return [...map.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [filtered]);

  const active = filtered.find((item) => item.key === selected) ?? null;
  const unread = items.filter((item) => !item.read).length;
  const attention = items.filter((item) => item.severity && ["critical", "high"].includes(item.severity)).length;

  return (
    <div className="page">
      <PageHeader
        title="Inbox"
        icon="inbox"
        subtitle="Unified view of all projects and items that need your attention."
        actions={
          <button
            className="button"
            onClick={() => void queryClient.invalidateQueries({ queryKey: ["inbox"] })}
            disabled={query.isFetching}
          >
            <Icon name="refresh" size={16} />
            Refresh
          </button>
        }
      />

      <InlineError error={action.error} />

      <Toolbar>
        <SearchField label="Search inbox" placeholder="Search items…" value={search} onChange={setSearch} />
        <FilterSelect
          label="Project"
          value={project}
          onChange={setProject}
          options={[{ value: "all", label: "All projects" }, ...projects.map((id) => ({ value: id, label: id }))]}
        />
        <FilterSelect
          label="Severity"
          value={severity}
          onChange={setSeverity}
          options={[
            { value: "all", label: "All severity" },
            ...SEVERITY_ORDER.map((level) => ({ value: level, label: humanize(level) })),
          ]}
        />
        <span className="spacer" />
        <span className="result-count">
          {filtered.length} of {items.length}
        </span>
      </Toolbar>

      <div className={active ? "workbench with-detail" : "workbench"}>
        <Panel className="flush">
          <div className="panel-header plain">
            <Tabs
              label="Inbox filters"
              active={tab}
              onSelect={setTab}
              tabs={[
                { id: "all", label: "All items", count: items.length },
                { id: "attention", label: "Needs attention", count: attention },
                { id: "waiting", label: "Waiting on me", count: unread },
                { id: "updates", label: "Updates", count: items.length - unread },
              ]}
            />
          </div>

          <QueryState
            query={query}
            skeletonRows={6}
            empty={<EmptyState icon="check-circle" label="Inbox is clear." detail="No project reported an attention item." />}
          >
            {() =>
              filtered.length === 0 ? (
                <EmptyState
                  inline
                  icon="check-circle"
                  label={items.length === 0 ? "Inbox is clear." : "No items match these filters."}
                />
              ) : (
                <div className="rows">
                  {groups.map(([kind, groupItems]) => (
                    <div key={kind}>
                      <button
                        className="group-header"
                        onClick={() =>
                          setCollapsed((current) => {
                            const next = new Set(current);
                            if (next.has(kind)) next.delete(kind);
                            else next.add(kind);
                            return next;
                          })
                        }
                        aria-expanded={!collapsed.has(kind)}
                      >
                        <Icon name={collapsed.has(kind) ? "chevron-right" : "chevron-down"} size={14} />
                        {humanize(kind)}
                        <span className="nav-count">{groupItems.length}</span>
                      </button>
                      {!collapsed.has(kind) &&
                        groupItems.map((item) => (
                          <button
                            key={item.key}
                            className={item.key === selected ? "row-item selected" : "row-item"}
                            onClick={() => setSelected(item.key)}
                          >
                            <Icon
                              name={statusTone(item.severity ?? item.status) === "danger" ? "x-circle" : "alert-triangle"}
                              size={16}
                              className={statusTone(item.severity ?? item.status)}
                            />
                            <div className="row-main">
                              <strong>{item.title}</strong>
                              <small>
                                {item.workspaceId ?? "System"} · {item.subjectId}
                              </small>
                            </div>
                            {item.severity && <StatusBadge value={item.severity} />}
                            <span className="row-meta">{relative(item.createdAt)}</span>
                            <Icon name="chevron-right" size={16} />
                          </button>
                        ))}
                    </div>
                  ))}
                </div>
              )
            }
          </QueryState>
        </Panel>

        {active && (
          <InboxDetail
            item={active}
            busy={action.isPending}
            onAction={(verb) => active.notificationId && action.mutate({ id: active.notificationId, verb })}
            onClose={() => setSelected(null)}
          />
        )}
      </div>
    </div>
  );
}

function InboxDetail({
  item,
  busy,
  onAction,
  onClose,
}: {
  item: Item;
  busy: boolean;
  onAction: (verb: string) => void;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const target = item.nextAction?.workspaceId ?? item.workspaceId;

  return (
    <DetailDrawer
      title={item.title}
      eyebrow={humanize(item.kind)}
      subtitle={
        <>
          {item.workspaceId ?? "System"} · created {relative(item.createdAt)}
          {item.updatedAt && ` · updated ${relative(item.updatedAt)}`}
        </>
      }
      badges={
        <>
          {item.severity && <StatusBadge value={item.severity} />}
          <StatusBadge value={item.status} />
        </>
      }
      onClose={onClose}
      footer={
        item.notificationId ? (
          <>
            <button className="button" disabled={busy || item.read} onClick={() => onAction("read")}>
              <Icon name="check" size={16} />
              {item.read ? "Read" : "Mark read"}
            </button>
            <button className="button" disabled={busy} onClick={() => onAction("dismiss")}>
              <Icon name="x" size={16} />
              Dismiss
            </button>
          </>
        ) : undefined
      }
    >
      <section className="stack tight">
        <h3>What happened</h3>
        <p className="muted">{item.message || humanize(item.kind)}</p>
      </section>

      {item.nextAction && (
        <NextAction
          label={item.nextAction.label}
          detail="Draft computed this as the next safe action for the item."
          action={
            target ? (
              <button className="button primary" onClick={() => navigate(`/projects/${encodeURIComponent(target)}`)}>
                Open
                <Icon name="chevron-right" size={14} />
              </button>
            ) : undefined
          }
        />
      )}

      <section className="stack tight">
        <h3>Details</h3>
        <Definitions rows>
          <dt>Status</dt>
          <dd>
            <StatusBadge value={item.status} plain />
          </dd>
          <dt>Severity</dt>
          <dd>{item.severity ? <StatusBadge value={item.severity} plain /> : NONE}</dd>
          <dt>Subject</dt>
          <dd className="mono">{item.subjectId || NONE}</dd>
          <dt>Project</dt>
          <dd>{item.workspaceId ?? NONE}</dd>
          <dt>Correlation</dt>
          <dd className="mono">{item.correlationId ?? NONE}</dd>
          <dt>Created</dt>
          <dd>{formatDateTime(item.createdAt)}</dd>
        </Definitions>
      </section>

      {item.occurrences.length > 0 && (
        <section className="stack tight">
          <h3>Occurrences</h3>
          <Panel className="flush">
            <PanelHeader title="Recorded" count={item.occurrences.length} plain />
            <ul className="audit-chain" style={{ padding: "var(--space-4)" }}>
              {item.occurrences.slice(0, 8).map((occurrence, index) => (
                <li key={index}>
                  <Icon name="clock" size={16} />
                  <div>
                    <span>{formatDateTime(occurrence.occurred_at)}</span>
                    {occurrence.correlation_id && <small className="mono">{occurrence.correlation_id}</small>}
                  </div>
                </li>
              ))}
            </ul>
          </Panel>
        </section>
      )}
    </DetailDrawer>
  );
}

/** Flattens the two canonical inbox sources into one comparable shape. */
function normalise(data: any): Item[] {
  if (!data) return [];
  const notifications: Item[] = (data.notifications ?? []).map((record: any) => ({
    key: `notification:${record.id}`,
    notificationId: record.id,
    kind: record.kind,
    title: record.title,
    message: record.message,
    severity: typeof record.severity === "string" ? record.severity.toLowerCase() : null,
    status: record.resolved_at ? "resolved" : record.read_at ? "read" : "unread",
    workspaceId: record.workspace_id ?? null,
    subjectId: record.context?.subject_id ?? record.deduplication_key ?? record.id,
    nextAction: record.next_safe_action
      ? {
          label: record.next_safe_action.label,
          kind: record.next_safe_action.kind,
          workspaceId: record.next_safe_action.workspace_id ?? null,
          subjectId: record.next_safe_action.subject_id ?? null,
        }
      : null,
    createdAt: record.created_at ?? null,
    updatedAt: record.updated_at ?? null,
    correlationId: record.correlation_id ?? null,
    read: Boolean(record.read_at || record.resolved_at),
    resolved: Boolean(record.resolved_at),
    occurrences: record.occurrences ?? [],
  }));

  const projectItems: Item[] = (data.projects ?? []).flatMap((group: any) =>
    (group.items ?? []).map((item: any, index: number) => ({
      key: `project:${group.workspace_id}:${item.id ?? item.subject_id}:${index}`,
      notificationId: null,
      kind: item.kind,
      title: item.summary || humanize(item.kind),
      message: item.summary && item.next_action ? item.next_action : item.next_action || "",
      severity: item.severity ? String(item.severity).toLowerCase() : null,
      status: item.status,
      workspaceId: group.workspace_id,
      subjectId: item.subject_id,
      nextAction: item.next_action
        ? { label: item.next_action, kind: item.kind, workspaceId: group.workspace_id, subjectId: item.subject_id }
        : null,
      createdAt: group.scanned_at ?? null,
      updatedAt: null,
      correlationId: null,
      read: false,
      resolved: false,
      occurrences: [],
    })),
  );

  return [...notifications, ...projectItems].sort(
    (a, b) => SEVERITY_ORDER.indexOf(a.severity ?? "info") - SEVERITY_ORDER.indexOf(b.severity ?? "info"),
  );
}
