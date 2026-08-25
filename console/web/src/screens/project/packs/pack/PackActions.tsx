import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate, settleJob } from "../../../../api";
import type { ServiceJob } from "../../../../contracts";
import { Icon, type IconName } from "../../../../icons";
import { JobBanner } from "../../../../components/JobBanner";
import { InlineError } from "../../../../components/states";
import { humanize } from "../../../../lib/format";

/** Icons for the canonical action names core reports as valid. */
const actionIcons: Record<string, IconName> = {
  verify: "shield-check",
  risk: "alert-triangle",
  review: "eye",
  approve: "check-circle",
  reject: "x-circle",
  submit: "upload",
  export: "download",
  import: "upload",
  rollback: "rotate-ccw",
  reopen: "refresh",
  delete: "trash",
};

/** Actions requiring an explicit confirmation because they finalize or reverse state. */
const CONFIRMED = ["reject", "submit", "rollback", "reopen", "delete"];

/**
 * Only the actions core computed as valid for the pack's current lifecycle are
 * offered. The UI never derives its own action set.
 */
export function PackActions({
  workspaceId,
  packId,
  actions,
}: {
  workspaceId: string;
  packId: string;
  actions: string[];
}) {
  const queryClient = useQueryClient();
  const [job, setJob] = useState<ServiceJob | null>(null);

  const action = useMutation({
    mutationFn: async (name: string) =>
      settleJob(
        await mutate<any>(
          `/api/v1/projects/${encodeURIComponent(workspaceId)}/packs/${encodeURIComponent(packId)}/actions/${name}`,
          {},
        ),
        setJob,
      ),
    onSettled: () => setJob(null),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["pack", workspaceId, packId] });
      void queryClient.invalidateQueries({ queryKey: ["pack-view", workspaceId, packId] });
      void queryClient.invalidateQueries({ queryKey: ["packs", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["project", workspaceId] });
    },
  });

  const run = (name: string) => {
    if (
      CONFIRMED.some((verb) => name.includes(verb)) &&
      !window.confirm(
        `${humanize(name)} pack ${packId} in project ${workspaceId}?\n\nDraft will revalidate authority, revision, policy, evidence, and operation identity before finalization.`,
      )
    )
      return;
    action.mutate(name);
  };

  if (actions.length === 0) {
    return (
      <div className="panel-body">
        <p className="muted">No mutation is valid in the current lifecycle state.</p>
      </div>
    );
  }

  return (
    <div className="action-rail">
      {actions.map((name) => (
        <button
          key={name}
          className={name.includes("submit") ? "button primary block" : "button block"}
          disabled={action.isPending}
          onClick={() => run(name)}
        >
          <Icon name={actionIcons[Object.keys(actionIcons).find((key) => name.includes(key)) ?? ""] ?? "zap"} size={16} />
          {action.isPending && action.variables === name ? "Working…" : humanize(name)}
        </button>
      ))}
      <JobBanner job={job} />
      <InlineError error={action.error} />
    </div>
  );
}
