import { useState } from "react";
import { useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../../api";
import { Icon } from "../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState } from "../../../components/states";
import { humanize, resourceLabel } from "../../../lib/format";
import { SectionNav } from "../SectionNav";
import { EXTENSION_ROUTES } from "../extensions/routes";

export type ToolAction = {
  action_id: string;
  display_name: string;
  description: string | null;
  contributed_by: string;
  /** What the contribution declared it may change. Draft holds it to this. */
  effect: string;
  /** Whether a grant exists for the command this action runs. */
  authorized: boolean;
  /** Locator bodies the action's own selector matches, as the adapter gave them. */
  applies_to: string[];
};

type Locator = { scheme: string; body: string };
/** Tagged by `proposal`; Draft is the one that turns it into an operation. */
type ProposedMutation = { proposal: string; locator?: Locator; from?: Locator; to?: Locator };

export type ToolInvocation = {
  action_id: string;
  summary: string | null;
  proposed_mutations: ProposedMutation[];
  applied: boolean;
  operation_id?: string | null;
};

function mutationLabel(proposal: ProposedMutation): string {
  const kind = humanize(proposal.proposal);
  if (proposal.from && proposal.to) {
    return `${kind} — ${resourceLabel(proposal.from)} → ${resourceLabel(proposal.to)}`;
  }
  return proposal.locator ? `${kind} — ${resourceLabel(proposal.locator)}` : kind;
}

/**
 * The tool actions installed extensions offer, and what running one proposed.
 *
 * A tool never changes the project itself. It returns findings and proposed
 * mutations, and Draft decides — under its own operation id, attribution,
 * protections and lease — whether to author them. So the screen has two
 * distinct buttons: previewing runs the tool and shows what it would ask for,
 * and applying is a separate decision a person makes after reading that.
 */
export function Tools() {
  const { workspaceId = "" } = useParams();
  const client = useQueryClient();
  const [result, setResult] = useState<ToolInvocation | null>(null);

  const query = useQuery({
    queryKey: ["tools", workspaceId],
    queryFn: () => api<ToolAction[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/tools`),
  });

  const invoke = useMutation({
    mutationFn: ({ actionId, apply }: { actionId: string; apply: boolean }) =>
      mutate<ToolInvocation>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/tool-invoke`, {
        action_id: actionId,
        apply,
      }),
    onSuccess: (invocation) => {
      setResult(invocation);
      if (invocation.applied) {
        client.invalidateQueries({ queryKey: ["resources", workspaceId] });
        client.invalidateQueries({ queryKey: ["observation-coverage", workspaceId] });
      }
    },
  });

  return (
    <div className="stack">
      <SectionNav section="Extensions" routes={EXTENSION_ROUTES} />
      <Panel className="flush">
        <PanelHeader
          title="Tools"
          icon="wrench"
          subtitle="Contributed actions. They propose; Draft authors the change."
          count={query.data?.length}
        />
        <QueryState
          query={query}
          empty={
            <EmptyState
              inline
              icon="wrench"
              label="No tool actions are installed."
              detail="Install and authorize an extension that contributes one."
            />
          }
        >
          {(actions) => (
            <div className="rows">
              {actions.map((action) => (
                <div className="row-item" key={action.action_id}>
                  <Icon name="wrench" size={16} />
                  <div className="row-main">
                    <strong>{action.display_name}</strong>
                    <small className="mono">{action.action_id}</small>
                    {action.description && <small>{action.description}</small>}
                    <small className="muted">
                      from {action.contributed_by} · declared effect {action.effect} ·{" "}
                      {action.applies_to.length} matching resources
                    </small>
                  </div>
                  <StatusBadge
                    value={action.authorized ? "authorized" : "not authorized"}
                    tone={action.authorized ? "success" : "warning"}
                    plain
                  />
                  <button
                    disabled={!action.authorized || invoke.isPending}
                    title={action.authorized ? undefined : "Grant this extension permission to run first"}
                    onClick={() => invoke.mutate({ actionId: action.action_id, apply: false })}
                  >
                    <Icon name="play" size={14} />
                    Preview
                  </button>
                </div>
              ))}
            </div>
          )}
        </QueryState>
        <InlineError error={invoke.error} />
      </Panel>

      {result && (
        <Panel className="flush">
          <PanelHeader
            title={result.applied ? "Applied" : "Proposed changes"}
            icon={result.applied ? "check-circle" : "list-checks"}
            count={result.proposed_mutations.length}
            action={
              !result.applied &&
              result.proposed_mutations.length > 0 && (
                <button
                  disabled={invoke.isPending}
                  onClick={() => invoke.mutate({ actionId: result.action_id, apply: true })}
                >
                  <Icon name="check" size={14} />
                  Apply as a Draft operation
                </button>
              )
            }
          />
          <div className="stack tight padded">
            <Definitions rows>
              <dt>Action</dt>
              <dd className="mono">{result.action_id}</dd>
              <dt>Summary</dt>
              <dd>{result.summary ?? "The tool returned no summary."}</dd>
              {result.operation_id && (
                <>
                  <dt>Operation</dt>
                  <dd className="mono">{result.operation_id}</dd>
                </>
              )}
            </Definitions>
            {result.proposed_mutations.length === 0 ? (
              <p className="muted">The tool proposed no changes.</p>
            ) : (
              <div className="rows">
                {result.proposed_mutations.map((proposal, index) => (
                  <div className="row-item" key={index}>
                    <Icon name="pencil" size={16} />
                    <div className="row-main">
                      <strong>{mutationLabel(proposal)}</strong>
                    </div>
                  </div>
                ))}
              </div>
            )}
            {!result.applied && (
              <p className="muted">
                Nothing has changed yet. Applying opens a Draft edit session under Draft's own
                operation id and attribution, where protections and the workspace lease apply exactly
                as they do to a person's edit. A proposal Draft refuses stops the whole operation.
              </p>
            )}
          </div>
        </Panel>
      )}
    </div>
  );
}
