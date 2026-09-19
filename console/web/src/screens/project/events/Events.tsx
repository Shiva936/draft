import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import { Icon } from "../../../icons";
import { FilterSelect, Panel, PanelHeader, SearchField, Toolbar } from "../../../components/layout";
import { CandidateAvatar } from "../../../components/CandidateAvatar";
import { EmptyState, QueryState } from "../../../components/states";
import { humanize, relative } from "../../../lib/format";
import { EventDetail } from "./EventDetail";

/** One Activity event, exactly as `core::read_model::activity` projects it. */
export type CanonicalEvent = {
  event_id: string;
  /** The frozen v1 vocabulary name. */
  kind: string;
  subject: string | null;
  actor: string;
  /** Nanoseconds since the epoch. */
  recorded_at: number;
  previous_hash: string;
  record_hash: string;
  metadata: Record<string, unknown>;
};

/** Activity records nanoseconds; the browser reads milliseconds. */
export function eventTime(event: CanonicalEvent): number {
  return Math.floor(event.recorded_at / 1_000_000);
}

const RANGES = [
  { value: "all", label: "All time" },
  { value: "24h", label: "Last 24 hours" },
  { value: "7d", label: "Last 7 days" },
  { value: "30d", label: "Last 30 days" },
];

const RANGE_MS: Record<string, number> = { "24h": 86_400_000, "7d": 604_800_000, "30d": 2_592_000_000 };

/** Append-only canonical project activity, with its hash-linked audit chain. */
export function Events() {
  const { workspaceId = "" } = useParams();
  const [selected, setSelected] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [type, setType] = useState("all");
  const [actor, setActor] = useState("all");
  const [range, setRange] = useState("all");
  const [live, setLive] = useState(true);

  const query = useQuery({
    queryKey: ["events", workspaceId],
    queryFn: () => api<CanonicalEvent[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/events`),
    refetchInterval: live ? 5_000 : false,
  });

  const events = query.data ?? [];
  const types = useMemo(() => [...new Set(events.map(eventType))].sort(), [events]);
  const actors = useMemo(() => [...new Set(events.map((event) => event.actor).filter(Boolean))].sort(), [events]);

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const cutoff = RANGE_MS[range] ? Date.now() - RANGE_MS[range] : null;
    return events.filter((event) => {
      if (type !== "all" && eventType(event) !== type) return false;
      if (actor !== "all" && event.actor !== actor) return false;
      if (cutoff !== null && eventTime(event) < cutoff) return false;
      if (needle && !`${eventType(event)} ${event.subject ?? ""} ${event.actor}`.toLowerCase().includes(needle))
        return false;
      return true;
    });
  }, [events, search, type, actor, range]);

  const active = filtered.find((event) => event.event_id === selected) ?? null;

  return (
    <>
      <Toolbar>
        <SearchField label="Search events" placeholder="Search events…" value={search} onChange={setSearch} />
        <FilterSelect
          label="Event type"
          value={type}
          onChange={setType}
          options={[{ value: "all", label: "All types" }, ...types.map((value) => ({ value, label: humanize(value) }))]}
        />
        <FilterSelect
          label="Actor"
          value={actor}
          onChange={setActor}
          options={[{ value: "all", label: "All actors" }, ...actors.map((value) => ({ value, label: value }))]}
        />
        <FilterSelect label="Time range" value={range} onChange={setRange} options={RANGES} />
        <span className="spacer" />
        <button className="button" onClick={() => setLive((value) => !value)} aria-pressed={!live}>
          <Icon name={live ? "pause" : "play"} size={16} />
          {live ? "Pause live updates" : "Resume live updates"}
        </button>
      </Toolbar>

      <div className={active ? "workbench three-pane with-detail" : "workbench three-pane"}>
        <Panel className="flush">
          <PanelHeader title="Timeline" icon="clock" count={filtered.length} />
          <QueryState
            query={query}
            skeletonRows={6}
            empty={<EmptyState inline icon="activity" label="No events recorded." />}
          >
            {() =>
              filtered.length === 0 ? (
                <EmptyState inline icon="search" label="No events match these filters." />
              ) : (
                <div className="timeline">
                  {filtered.slice(0, 40).map((event) => (
                    <button
                      key={event.event_id}
                      className={event.event_id === selected ? "timeline-item selected" : "timeline-item"}
                      onClick={() => setSelected(event.event_id)}
                    >
                      <Icon name={eventIcon(eventType(event))} size={16} className={eventTone(eventType(event))} />
                      <span className="timeline-body">
                        <time>{relative(eventTime(event))}</time>
                        <strong>{humanize(eventType(event))}</strong>
                      </span>
                    </button>
                  ))}
                </div>
              )
            }
          </QueryState>
        </Panel>

        <Panel className="flush">
          <PanelHeader
            title="Activity"
            icon="activity"
            count={filtered.length}
            action={<span className="result-count">{live ? "Live updates" : "Paused"}</span>}
          />
          {filtered.length === 0 ? (
            <EmptyState inline icon="activity" label="No events to show." />
          ) : (
            <div className="table-wrap">
              <table className="data">
                <thead>
                  <tr>
                    <th>Event</th>
                    <th className="shrink">Actor</th>
                    <th className="shrink">Related</th>
                    <th className="shrink">Time</th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.slice(0, 100).map((event) => (
                    <tr
                      key={event.event_id}
                      tabIndex={0}
                      className={event.event_id === selected ? "selectable selected" : "selectable"}
                      onClick={() => setSelected(event.event_id)}
                      onKeyDown={(keyEvent) => {
                        if (keyEvent.key === "Enter" || keyEvent.key === " ") {
                          keyEvent.preventDefault();
                          setSelected(event.event_id);
                        }
                      }}
                    >
                      <td>
                        <div className="cell-primary">
                          <Icon name={eventIcon(eventType(event))} size={16} className={eventTone(eventType(event))} />
                          <div className="cell-text">
                            <strong>{humanize(eventType(event))}</strong>
                            <small className="mono">{eventType(event)}</small>
                          </div>
                        </div>
                      </td>
                      <td className="shrink">
                        <span className="avatar-label">
                          <CandidateAvatar name={event.actor} />
                          <span className="truncate">{event.actor}</span>
                        </span>
                      </td>
                      <td className="shrink mono">{event.subject ?? <span className="empty-cell">—</span>}</td>
                      <td className="shrink muted">{relative(eventTime(event))}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Panel>

        {active && <EventDetail event={active} events={events} onClose={() => setSelected(null)} />}
      </div>
    </>
  );
}

export function eventType(event: CanonicalEvent): string {
  return event.kind || "event";
}

function eventIcon(type: string) {
  if (/fail|error|reject/.test(type)) return "x-circle" as const;
  if (/warn|risk|drift/.test(type)) return "alert-triangle" as const;
  if (/approve|pass|complete|verified|saved/.test(type)) return "check-circle" as const;
  if (/creat|add/.test(type)) return "plus" as const;
  if (/submit|review/.test(type)) return "upload" as const;
  return "dot" as const;
}

function eventTone(type: string) {
  if (/fail|error|reject/.test(type)) return "danger";
  if (/warn|risk|drift/.test(type)) return "warning";
  if (/approve|pass|complete|verified|saved/.test(type)) return "success";
  return "running";
}
