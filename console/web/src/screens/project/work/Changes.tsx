import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../../api";
import { Icon } from "../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState } from "../../../components/states";
import { NONE } from "../../../lib/format";
import { SectionNav } from "../SectionNav";
import { WORK_ROUTES } from "./routes";
import {
  availability,
  graphBase,
  operationTone,
  type AuthorizationView,
  type BaselineView,
  type ChangeView,
  type GraphView,
} from "../graph/model";

/**
 * The Changes view of Work: every Change with a sealed revision, and what has
 * been established about it.
 *
 * The browser renders what `draftd` computed and decides nothing. In
 * particular it never works out whether promotion is legal: the server sends
 * `available` and, when it is not, the reason — so a disabled control always
 * has an explanation somebody wrote rather than one this file invented.
 *
 * The stages are visibly separate because they are separate acts:
 *
 *   Decision authorizes · Promotion accepts · Publication delivers
 */
export function Changes() {
  const { workspaceId = "" } = useParams();
  const client = useQueryClient();
  const base = graphBase(workspaceId);

  const graph = useQuery({ queryKey: ["graph", workspaceId], queryFn: () => api<GraphView>(base) });
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["graph", workspaceId] });
    void client.invalidateQueries({ queryKey: ["graph-authorization", workspaceId] });
  };

  return (
    <>
      <SectionNav section="Work" routes={WORK_ROUTES} />
      <QueryState
        query={graph}
        skeletonRows={6}
        empty={
          <Panel>
            <EmptyState label="This project has no Changes yet." />
          </Panel>
        }
      >
        {(view: GraphView) => {
          const open = view.changes.filter((change) => change.revisions.length > 0);
          if (open.length === 0) {
            return (
              <Panel>
                <PanelHeader
                  title="Changes"
                  subtitle="Evidence, assessment, gate, decision — then promotion."
                />
                <EmptyState label="No Change has a sealed revision to authorize." />
              </Panel>
            );
          }
          return (
            <>
              {open.map((change) => (
                <ChangePanel
                  key={change.change}
                  workspaceId={workspaceId}
                  base={base}
                  change={change}
                  baseline={view.baseline ?? null}
                  onChanged={refresh}
                />
              ))}
            </>
          );
        }}
      </QueryState>
    </>
  );
}

function ChangePanel({
  workspaceId,
  base,
  change,
  baseline,
  onChanged,
}: {
  workspaceId: string;
  base: string;
  change: ChangeView;
  baseline: BaselineView | null;
  onChanged: () => void;
}) {
  const revision = change.revisions[0];
  const query = useQuery({
    queryKey: ["graph-authorization", workspaceId, change.change, revision.id],
    queryFn: () =>
      api<AuthorizationView>(
        `${base}/authorization/${encodeURIComponent(change.change)}/${encodeURIComponent(revision.id)}`,
      ),
  });

  const promote = useMutation({
    mutationFn: (view: AuthorizationView) => {
      const decision = view.decisions.find(
        (candidate) =>
          "approved" in (candidate.outcome ?? {}) ||
          (candidate.outcome as { outcome?: string } | null)?.outcome === "approved",
      );
      const gate = view.gates.find((candidate) => candidate.satisfied);
      return mutate<unknown>(`${base}/promote`, {
        change: view.change,
        revision: view.revision,
        decision: decision?.id ?? "",
        gate: gate?.evaluation.id ?? "",
        // The precondition token. Sent from the model this screen is showing,
        // so acting on a stale view fails rather than promoting onto a parent
        // nobody judged the work against.
        expected_baseline: baseline?.baseline ?? "",
      });
    },
    onSuccess: onChanged,
  });

  return (
    <QueryState
      query={query}
      skeletonRows={4}
      empty={
        <Panel>
          <EmptyState label="Nothing decided yet." />
        </Panel>
      }
    >
      {(view: AuthorizationView) => {
        const promotable = availability(view.actions, "promote");
        const gate = view.gates.find((candidate) => candidate.satisfied) ?? view.gates[0];
        return (
          <Panel>
            <PanelHeader
              title={
                // Into the Change's own scope, where §8.3's fourteen views —
                // intent, scope, impact, recovery and the rest — live.
                <Link to={encodeURIComponent(change.change)}>
                  {change.change} · {view.revision}
                </Link>
              }
              subtitle="A Decision authorizes a promotion. It does not perform one."
              action={<StatusBadge value={change.lifecycle} />}
            />
            <Definitions rows>
              <dt>Evidence</dt>
              <dd>{view.evidence.map((item) => item.outcome).join(", ") || NONE}</dd>
              <dt>Assessments</dt>
              <dd>{view.assessments.map((item) => item.risk).join(", ") || NONE}</dd>
              <dt>Representation</dt>
              <dd>
                {view.representation ? (
                  // What explains the work, and by whose strategy. Named rather
                  // than counted: "explained by the neutral rendering" and
                  // "explained by an installed extension" are different facts,
                  // and only one of them says where inside a Resource the work
                  // landed.
                  <>
                    {view.representation.representations.length} resource
                    {view.representation.representations.length === 1 ? "" : "s"} explained
                    {view.representation.representations.length > 0 ? (
                      <>
                        {" "}
                        ·{" "}
                        {Array.from(
                          new Set(view.representation.representations.map((item) => item.strategy_id)),
                        ).join(", ")}
                      </>
                    ) : null}
                  </>
                ) : (
                  NONE
                )}
              </dd>
              <dt>Gate</dt>
              <dd>
                {gate ? (
                  <>
                    {gate.satisfied ? "satisfied" : `not satisfied — ${gate.unsatisfied.join(", ")}`}
                    {gate.waived.length > 0 ? (
                      // Never folded into "satisfied": somebody allowed this,
                      // which is a different fact from the check passing.
                      <> · waived by a person: {gate.waived.join(", ")}</>
                    ) : null}
                  </>
                ) : (
                  NONE
                )}
              </dd>
              <dt>Decisions</dt>
              <dd>{view.decisions.length > 0 ? view.decisions.map((item) => item.id).join(", ") : NONE}</dd>
              <dt>Promotion</dt>
              <dd>
                {view.promotion ? (
                  <>
                    <StatusBadge
                      value={view.promotion.state}
                      tone={operationTone[view.promotion.state]}
                    />{" "}
                    {view.promotion.detail}
                  </>
                ) : (
                  "not started"
                )}
              </dd>
            </Definitions>

            <div className="panel-actions">
              <button
                className="button primary"
                disabled={!promotable.available || promote.isPending}
                title={promotable.reason ?? undefined}
                onClick={() => promote.mutate(view)}
              >
                <Icon name="check" size={16} />
                Promote — changes the accepted Baseline
              </button>
              {!promotable.available && promotable.reason ? (
                <p className="muted">{promotable.reason}</p>
              ) : null}
            </div>
            {promote.error ? <InlineError error={promote.error} /> : null}
          </Panel>
        );
      }}
    </QueryState>
  );
}
