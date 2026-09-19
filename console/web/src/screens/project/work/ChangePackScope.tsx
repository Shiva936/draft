import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import type { ConsoleReadModel } from "../../../contracts";
import { EmptyState, QueryState } from "../../../components/states";
import { Panel } from "../../../components/layout";
import { ScopeViews } from "../ScopeViews";

/**
 * One ChangePack, in the fourteen views §8.3 gives it.
 *
 * Each view is backed by the application API that owns its question —
 * intent by the canonical definition, scope by the resolution against an exact
 * Baseline, impact by the Stage 11 report, recovery by the promotion restart
 * table. The browser picks a view; it computes none of them.
 */
const CHANGE_PACK_VIEW_PATHS: Record<string, string[]> = {
  Summary: ["summary"],
  Intent: ["intent"],
  Scope: ["scope"],
  Revisions: ["revisions"],
  Impact: ["impact"],
  Representations: ["representations"],
  Evidence: ["authorization", "evidence"],
  Assessments: ["authorization", "assessments"],
  Review: ["authorization", "reviews"],
  Decisions: ["authorization", "decisions"],
  Gates: ["authorization", "gates"],
  Promotion: ["authorization", "promotion"],
  Receipts: ["receipts"],
  Recovery: ["recovery"],
};

export function ChangePackScope() {
  const { workspaceId = "", packId = "" } = useParams();
  const query = useQuery({
    queryKey: ["console-model", "CHANGE_PACK", workspaceId, packId],
    queryFn: () =>
      api<ConsoleReadModel>(
        `/api/v1/console/model?scope=CHANGE_PACK&workspace_id=${encodeURIComponent(workspaceId)}&change_pack_id=${encodeURIComponent(packId)}`,
      ),
    staleTime: 0,
    gcTime: 0,
    structuralSharing: false,
  });

  return (
    <QueryState
      query={query}
      skeletonRows={6}
      empty={
        <Panel>
          <EmptyState label="This project has no such ChangePack." />
        </Panel>
      }
    >
      {(model: ConsoleReadModel) => (
        <ScopeViews
          model={model}
          paths={CHANGE_PACK_VIEW_PATHS}
          title={packId}
          subtitle="Each view is its own act. A Decision authorizes; a Promotion accepts."
        />
      )}
    </QueryState>
  );
}
