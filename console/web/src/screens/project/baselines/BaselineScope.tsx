import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import type { ConsoleReadModel } from "../../../contracts";
import { EmptyState, QueryState } from "../../../components/states";
import { Panel } from "../../../components/layout";
import { ScopeViews } from "../ScopeViews";

/**
 * One accepted Baseline, in the nine views §8.3 gives it.
 *
 * The three roots are three views because they answer three different
 * questions — what material state is accepted, what provenance establishes it,
 * and what justifies absence. Publications is a view of this Baseline and
 * never a rename of it: a delivery has no authority over what is accepted.
 */
const BASELINE_VIEW_PATHS: Record<string, string[]> = {
  Summary: ["baseline", "record"],
  "State root": ["baseline", "manifest", "project_state_root"],
  "Evidence root": ["baseline", "manifest", "state_evidence_root"],
  Coverage: ["baseline", "manifest", "coverage_evidence_root"],
  Lineage: ["baseline", "lineage"],
  Composition: ["baseline", "composition"],
  Recoverability: ["baseline", "recoverability"],
  Receipts: ["baseline", "receipts"],
  Publications: ["baseline", "publications"],
};

export function BaselineScope() {
  const { workspaceId = "", baselineId = "" } = useParams();
  const query = useQuery({
    queryKey: ["console-model", "BASELINE", workspaceId, baselineId],
    queryFn: () =>
      api<ConsoleReadModel>(
        `/api/v1/console/model?scope=BASELINE&workspace_id=${encodeURIComponent(workspaceId)}&baseline_id=${encodeURIComponent(baselineId)}`,
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
          <EmptyState label="This project accepts no such Baseline." />
        </Panel>
      }
    >
      {(model: ConsoleReadModel) => (
        <ScopeViews
          model={model}
          paths={BASELINE_VIEW_PATHS}
          title="Baseline"
          subtitle="An accepted historical node. Nothing here is editable."
        />
      )}
    </QueryState>
  );
}
