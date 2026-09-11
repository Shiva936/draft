import { Link, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import { Icon } from "../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, QueryState } from "../../../components/states";
import { shortDigest } from "../../../lib/format";
import { SectionNav } from "../SectionNav";
import { BASELINE_ROUTES } from "./routes";

/**
 * The Baselines section: what this project accepts, and what it accepted before.
 *
 * Every field is the authority's. The three roots are rendered separately and
 * never collapsed, because they answer three different questions — what
 * material state is accepted, what provenance establishes it, and what
 * justifies absence. A reader shown one number for all three could not tell
 * which of them moved.
 *
 * Publication is a sibling view, not a field of a Baseline. A delivery that
 * failed leaves the accepted Baseline exactly as it is.
 */
type RecoveryTarget = {
  snapshot: string;
  created_at: string;
  status: { recovery: string; missing_resources?: string[]; missing_domains?: unknown[] };
};

type BaselineDetail = {
  baseline: string;
  accepted: boolean;
  manifest: {
    project_state_root: string;
    state_evidence_root: string;
    coverage_evidence_root: string;
    parent_baseline_id?: string | null;
  };
  record: { actor: string; accepted_at: unknown; origin: Record<string, unknown> };
  lineage: string[];
  composition: Record<string, { binding: string; semantic_definition: string }>;
  recoverability: { targets: RecoveryTarget[]; fully_anchored: number; summary: string };
  receipts: unknown[];
  publications: { publication: string }[];
  routable: boolean;
  route_refusal?: string | null;
};

export function Baselines() {
  const { workspaceId = "" } = useParams();
  const query = useQuery({
    queryKey: ["baselines", workspaceId],
    queryFn: () =>
      api<BaselineDetail[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/baselines`),
  });

  return (
    <>
      <SectionNav section="Baselines" routes={BASELINE_ROUTES} />
      <QueryState
        query={query}
        skeletonRows={6}
        empty={
          <Panel>
            <EmptyState label="This project accepts no Baseline yet." />
          </Panel>
        }
      >
        {(baselines: BaselineDetail[]) =>
          baselines.length === 0 ? (
            <Panel>
              <EmptyState label="This project accepts no Baseline yet." />
            </Panel>
          ) : (
            <>
              {baselines.map((baseline) => (
                <BaselinePanel key={baseline.baseline} baseline={baseline} />
              ))}
            </>
          )
        }
      </QueryState>
    </>
  );
}

function BaselinePanel({ baseline }: { baseline: BaselineDetail }) {
  const composition = Object.entries(baseline.composition);
  return (
    <Panel>
      <PanelHeader
        title={baseline.accepted ? "Accepted Baseline" : "Historical Baseline"}
        subtitle={
          baseline.accepted
            ? "This is the project's authoritative state. Only a promotion changes it."
            : "An accepted historical node. Nothing can change what it was composed from."
        }
        action={<StatusBadge value={baseline.routable ? "routable" : "not routable"} />}
      />
      <Definitions rows>
        <dt>Baseline</dt>
        <dd title={baseline.baseline}>
          {/* Into this Baseline's own scope: its three roots, lineage,
              composition, recoverability, receipts and deliveries. */}
          <Link to={encodeURIComponent(baseline.baseline)}>
            {shortDigest(baseline.baseline)}
          </Link>
        </dd>
        <dt>Project state root</dt>
        <dd title={baseline.manifest.project_state_root}>
          {shortDigest(baseline.manifest.project_state_root)}
        </dd>
        <dt>State evidence root</dt>
        <dd title={baseline.manifest.state_evidence_root}>
          {shortDigest(baseline.manifest.state_evidence_root)}
        </dd>
        <dt>Coverage evidence root</dt>
        <dd title={baseline.manifest.coverage_evidence_root}>
          {shortDigest(baseline.manifest.coverage_evidence_root)}
        </dd>
        <dt>Parent</dt>
        <dd>
          {baseline.manifest.parent_baseline_id
            ? shortDigest(baseline.manifest.parent_baseline_id)
            : "none — this is the project's first"}
        </dd>
        <dt>Lineage</dt>
        <dd>
          {baseline.lineage.length} baseline{baseline.lineage.length === 1 ? "" : "s"}
        </dd>
        <dt>Recoverability</dt>
        <dd>{baseline.recoverability.summary}</dd>
        <dt>Receipts</dt>
        <dd>{baseline.receipts.length}</dd>
      </Definitions>
      {baseline.route_refusal ? (
        <p className="muted">
          <Icon name="alert-triangle" size={14} /> {baseline.route_refusal}
        </p>
      ) : null}

      <PanelHeader
        title="Composition"
        subtitle="What observed each Resource's accepted state. Fixed at acceptance; a later unbind does not change it."
      />
      {composition.length > 0 ? (
        <ul className="list">
          {composition.map(([resource, provenance]) => (
            <li key={resource}>
              <code>{resource}</code> — {provenance.binding}
            </li>
          ))}
        </ul>
      ) : (
        <EmptyState label="This Baseline accepts no Resource, so nothing established one." />
      )}

      <PanelHeader
        title="Recovery targets"
        subtitle="What the anchors prove, not what happens to be on disk."
      />
      {baseline.recoverability.targets.length > 0 ? (
        <ul className="list">
          {baseline.recoverability.targets.map((target) => (
            <li key={target.snapshot}>
              <StatusBadge
                value={target.status.recovery}
                tone={target.status.recovery === "fully_anchored" ? "success" : "warning"}
              />{" "}
              <code>{target.snapshot}</code>
            </li>
          ))}
        </ul>
      ) : (
        <EmptyState label="No recovery target has been captured for this project." />
      )}
    </Panel>
  );
}
