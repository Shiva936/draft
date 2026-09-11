import { useRef, useState } from "react";
import { useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate, newOperationId } from "../../../api";
import { Icon } from "../../../icons";
import { Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState } from "../../../components/states";
import { shortDigest } from "../../../lib/format";
import { SectionNav } from "../SectionNav";
import { BASELINE_ROUTES } from "./routes";
import { availability, graphBase, operationTone, type GraphView } from "../graph/model";

/**
 * Publication: delivering an accepted Baseline somewhere outside Draft.
 *
 * A sibling of Baselines rather than a field of one, and deliberately so. A
 * delivery is optional, repeatable and never authoritative: it cannot create,
 * change or roll back what the project accepts. Rendering it inside a Baseline
 * would invite reading a failed delivery as a Baseline that failed.
 */
export function Publications() {
  const { workspaceId = "" } = useParams();
  const client = useQueryClient();
  const base = graphBase(workspaceId);
  const query = useQuery({ queryKey: ["graph", workspaceId], queryFn: () => api<GraphView>(base) });
  const refresh = () => void client.invalidateQueries({ queryKey: ["graph", workspaceId] });

  return (
    <>
      <SectionNav section="Baselines" routes={BASELINE_ROUTES} />
      <QueryState
        query={query}
        skeletonRows={5}
        empty={
          <Panel>
            <EmptyState label="Nothing has been published from this project." />
          </Panel>
        }
      >
        {(view: GraphView) => <PublicationPanel base={base} view={view} onChanged={refresh} />}
      </QueryState>
    </>
  );
}

function PublicationPanel({
  base,
  view,
  onChanged,
}: {
  base: string;
  view: GraphView;
  onChanged: () => void;
}) {
  const publishable = availability(view.actions, "publish");
  // One id per attempt, held across retries of that attempt. A re-send under
  // the same id converges on what it concluded; a new send is a new id.
  const attempt = useRef(newOperationId());
  const [attempted, setAttempted] = useState(false);

  const publish = useMutation({
    mutationFn: () => {
      if (attempted) attempt.current = newOperationId();
      setAttempted(true);
      return mutate<unknown>(`${base}/publish`, { purpose: "draft.publish/export" }, attempt.current);
    },
    onSuccess: onChanged,
  });

  const failed = view.publications.filter(
    (item) => item.state === "failed" || item.state === "blocked",
  );

  return (
    <Panel>
      <PanelHeader
        title="Publication"
        subtitle="Delivers an accepted Baseline outside Draft. Optional, repeatable, and never authoritative."
      />
      {failed.length > 0 ? (
        // The exact sentence that keeps a delivery failure from reading as a
        // rollback. The Baselines view still shows the Baseline as accepted,
        // because it is.
        <p className="muted">
          <Icon name="alert-triangle" size={14} /> A delivery did not succeed. The accepted Baseline is
          unchanged — only the delivery failed.
        </p>
      ) : null}
      {view.publications.length === 0 ? (
        <EmptyState label="Nothing has been published from this project." />
      ) : (
        <ul className="list">
          {view.publications.map((item) => (
            <li key={item.publication}>
              <StatusBadge value={item.state} tone={operationTone[item.state]} />{" "}
              <code>{item.publication}</code> · {item.purpose} · baseline {shortDigest(item.baseline)}
              <div className="muted">{item.detail}</div>
              {!item.may_retry_automatically && item.state !== "completed" ? (
                <div className="muted">
                  Another attempt needs somebody to accept that it may duplicate a real-world effect.
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      <div className="panel-actions">
        <button
          className="button"
          disabled={!publishable.available || publish.isPending}
          title={publishable.reason ?? undefined}
          onClick={() => publish.mutate()}
        >
          <Icon name="upload" size={16} />
          Publish the accepted Baseline
        </button>
        {!publishable.available && publishable.reason ? (
          <p className="muted">{publishable.reason}</p>
        ) : null}
      </div>
      {publish.error ? <InlineError error={publish.error} /> : null}
    </Panel>
  );
}
