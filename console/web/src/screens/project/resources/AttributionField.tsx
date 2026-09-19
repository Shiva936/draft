import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import type { ProjectSummary } from "../../../contracts";

export type AttributionRef = { kind: string; id: string };

/**
 * Attribution must be chosen before any edit is persisted, and cannot change
 * once a session is open. Both rules are core requirements, not UI polish.
 */
export function AttributionField({
  workspaceId,
  value,
  locked,
  onChange,
  label = "Attribution",
}: {
  workspaceId: string;
  value: AttributionRef | null;
  locked: boolean;
  onChange: (value: AttributionRef | null) => void;
  label?: string;
}) {
  const project = useQuery({
    queryKey: ["project", workspaceId],
    queryFn: () => api<ProjectSummary>(`/api/v1/projects/${encodeURIComponent(workspaceId)}`),
  });

  const serialised = value ? `${value.kind}:${value.id}` : "";

  return (
    <label className="field">
      <span>{label}</span>
      <select
        className="select"
        aria-label="Tree edit attribution"
        value={serialised}
        disabled={locked}
        title={locked ? "Attribution cannot change inside an open edit session" : undefined}
        onChange={(event) => {
          const next = event.target.value;
          const separator = next.indexOf(":");
          onChange(separator === -1 ? null : { kind: next.slice(0, separator), id: next.slice(separator + 1) });
        }}
      >
        <option value="">Choose attribution…</option>
        {(project.data?.tasks ?? []).map((task) => (
          <option key={task.id} value={`task:${task.id}`}>
            Task · {task.name}
          </option>
        ))}
        {(project.data?.change_packs ?? []).map((change) => (
          <option key={change.change_pack_id} value={`change_pack:${change.change_pack_id}`}>
            ChangePack · {change.name}
          </option>
        ))}
      </select>
    </label>
  );
}
