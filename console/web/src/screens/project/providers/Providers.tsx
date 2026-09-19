import { useParams } from "react-router-dom";
import { useMutation } from "@tanstack/react-query";
import { invokeAction } from "../../../api";
import { ActionButton, type ActionArguments } from "../../../components/actions";
import {
  ActionIndex,
  projected,
  useProjectConsoleModel,
  useRefreshProjectConsoleModel,
} from "../../../lib/consoleModel";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState } from "../../../components/states";
import { shortDigest } from "../../../lib/format";

/**
 * Providers: how this project is attached to the systems that observe it.
 *
 * Three kinds of fact, kept apart because their mutability differs:
 *
 *   - a **binding** is mutable — it can be unbound and rebound, and its
 *     generation moves when it does;
 *   - a **semantic definition** and an **operational profile** are immutable,
 *     content-addressed facts a binding points at.
 *
 * That is why an unbind never destroys anything: the definition an accepted
 * Baseline was composed under is still here and still readable afterwards.
 * Both acts run through the audited application operation the CLI calls, so a
 * Console unbind is journalled, audited and appended exactly like a terminal
 * one.
 */
type Binding = {
  binding: {
    id: string;
    generation: number;
    kind: string;
    lifecycle: string;
    current_semantic_definition: string;
    current_operational_profile: string;
  };
  routable: boolean;
  semantic_definition?: { kind: string; locator_state_role: string } | null;
  operational_profile?: {
    merge_capability: string;
    concurrency_policy: string;
    publication_delivery: string;
  } | null;
};

type Catalog = {
  bindings: Binding[];
  definitions: { kind: string; locator_state_role: string }[];
  profiles: { merge_capability: string; concurrency_policy: string }[];
};

export function Providers() {
  const { workspaceId = "" } = useParams();
  const model = useProjectConsoleModel(workspaceId);
  const refresh = useRefreshProjectConsoleModel(workspaceId);
  const catalog = (model.data?.content as { providers?: Catalog } | undefined)?.providers;

  const act = useMutation({
    mutationFn: ({ capability, args }: { capability: string; args: ActionArguments }) =>
      invokeAction<unknown>(capability, model.data!.revisions, args),
    onSuccess: refresh,
  });

  const actions = new ActionIndex(model.data?.actions ?? []);

  return (
    <QueryState
      query={projected(model, catalog)}
      skeletonRows={5}
      empty={
        <Panel>
          <EmptyState label="This project is bound to no provider." />
        </Panel>
      }
    >
      {(view: Catalog) => (
        <>
          <Panel>
            <PanelHeader
              title="Bindings"
              subtitle="Mutable. Unbinding stops new work routing through it and keeps every past fact."
            />
            {view.bindings.length === 0 ? (
              <EmptyState label="This project is bound to no provider." />
            ) : (
              view.bindings.map((entry) => (
                <BindingRow
                  key={entry.binding.id}
                  entry={entry}
                  actions={actions}
                  revisions={model.data!.revisions}
                  busy={act.isPending}
                  onInvoke={(capability, args) => act.mutateAsync({ capability, args }).then(() => {})}
                  onExpired={refresh}
                />
              ))
            )}
            {act.error ? <InlineError error={act.error} /> : null}
          </Panel>

          <Panel>
            <PanelHeader
              title="Semantic definitions"
              subtitle="Immutable and content-addressed. A binding moving off one does not remove it."
            />
            {view.definitions.length === 0 ? (
              <EmptyState label="No semantic definition has been recorded." />
            ) : (
              <ul className="list">
                {view.definitions.map((definition, index) => (
                  <li key={`${definition.kind}-${index}`}>
                    <code>{definition.kind}</code> · {definition.locator_state_role}
                  </li>
                ))}
              </ul>
            )}
          </Panel>

          <Panel>
            <PanelHeader
              title="Operational profiles"
              subtitle="Immutable. What a provider can do, separately from whether it may."
            />
            {view.profiles.length === 0 ? (
              <EmptyState label="No operational profile has been recorded." />
            ) : (
              <ul className="list">
                {view.profiles.map((profile, index) => (
                  <li key={index}>
                    merge {profile.merge_capability} · concurrency {profile.concurrency_policy}
                  </li>
                ))}
              </ul>
            )}
          </Panel>
        </>
      )}
    </QueryState>
  );
}

function BindingRow({
  entry,
  actions,
  revisions,
  busy,
  onInvoke,
  onExpired,
}: {
  entry: Binding;
  actions: ActionIndex;
  revisions: unknown;
  busy: boolean;
  onInvoke: (capability: string, args: ActionArguments) => Promise<void>;
  onExpired: () => void;
}) {
  const unbound = entry.binding.lifecycle === "unbound";
  return (
    <div className="panel-section">
      <PanelHeader
        title={entry.binding.id}
        subtitle={entry.binding.kind}
        action={
          <StatusBadge
            value={entry.binding.lifecycle}
            tone={unbound ? "warning" : entry.routable ? "success" : "neutral"}
          />
        }
      />
      <Definitions rows>
        <dt>Semantic definition</dt>
        <dd title={entry.binding.current_semantic_definition}>
          {shortDigest(entry.binding.current_semantic_definition)}
        </dd>
        <dt>Operational profile</dt>
        <dd title={entry.binding.current_operational_profile}>
          {shortDigest(entry.binding.current_operational_profile)}
        </dd>
        <dt>Generation</dt>
        <dd>{entry.binding.generation}</dd>
        <dt>Routable</dt>
        <dd>
          {entry.routable
            ? "yes — new work may be routed through it"
            : "no — nothing new routes through it, and its history is unaffected"}
        </dd>
      </Definitions>
      <div className="panel-actions">
        {/* Rendered only where the server issued them. An action absent here
            is the authority saying it does not apply. */}
        <ActionButton
          action={actions.get(unbound ? "project.provider.rebind" : "project.provider.unbind")}
          revisions={revisions as never}
          busy={busy}
          icon="plug"
          prefill={{ binding: entry.binding.id }}
          onInvoke={onInvoke}
          onExpired={onExpired}
        />
      </div>
    </div>
  );
}
