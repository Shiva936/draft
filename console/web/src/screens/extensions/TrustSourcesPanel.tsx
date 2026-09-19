import type { CanonicalRevisions, ExtensionCatalogSourceStatusDto } from "../../contracts";
import { Icon } from "../../icons";
import { Panel, PanelHeader, Toolbar } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { EmptyState, QueryState } from "../../components/states";
import { ActionButton, type ActionArguments } from "../../components/actions";
import type { ActionIndex } from "../../lib/consoleModel";
import { NONE, formatDateTime } from "../../lib/format";

/**
 * Catalog source configuration and out-of-band trust bootstrap.
 *
 * Console accepts HTTPS source configuration and pasted signed root metadata
 * only. It never treats a source URL as trust and exposes no filesystem
 * browser; local-directory catalogs remain explicit CLI workflows.
 */
export function TrustSourcesPanel({
  sources,
  query,
  actions,
  revisions,
  busy,
  onInvoke,
  onExpired,
}: {
  sources: ExtensionCatalogSourceStatusDto[];
  /** Loading and error states for the authoritative model behind `sources`. */
  query: {
    isLoading: boolean;
    error: unknown;
    data: ExtensionCatalogSourceStatusDto[] | undefined;
    refetch: () => unknown;
  };
  actions: ActionIndex;
  revisions: CanonicalRevisions;
  busy: boolean;
  onInvoke: (capability: string, args: ActionArguments) => Promise<void> | void;
  onExpired: () => void;
}) {
  return (
    <div className="workbench with-detail">
      <Panel className="flush">
        <PanelHeader
          title="Catalog sources"
          icon="database"
          count={sources.length}
          subtitle="Configuration never implies trust. Console accepts HTTPS sources only."
        />
        <div className="panel-body">
          <Toolbar>
            <ActionButton
              action={actions.get("extension.source.add")}
              revisions={revisions}
              busy={busy}
              className="button primary"
              icon="plus"
              label="Configure source"
              onInvoke={onInvoke}
              onExpired={onExpired}
            />
          </Toolbar>
        </div>

        <QueryState
          query={query}
          skeletonRows={3}
          empty={
            <EmptyState
              icon="database"
              label="No extension sources are configured."
              detail="Configure an HTTPS catalog above, then accept its signed root metadata out of band."
            />
          }
        >
          {() => (
            <div className="table-wrap">
              <table className="data">
                <thead>
                  <tr>
                    <th>Source</th>
                    <th className="shrink">Trust</th>
                    <th className="shrink">Usability</th>
                    <th className="shrink">Packages</th>
                    <th className="shrink">Refreshed</th>
                    <th className="shrink" />
                  </tr>
                </thead>
                <tbody>
                  {sources.map((item) => (
                    <tr key={item.source.id}>
                      <td>
                        <div className="cell-primary">
                          <Icon name="database" size={18} />
                          <div className="cell-text">
                            <strong>{item.source.id}</strong>
                            <small>
                              {item.source.location.kind === "https"
                                ? item.source.location.url
                                : item.source.location.path}
                            </small>
                          </div>
                        </div>
                      </td>
                      <td className="shrink">
                        <StatusBadge value={item.trusted ? "trusted" : "untrusted"} />
                        {item.source.builtin && <StatusBadge value="neutral" label="Built in" />}
                        {item.source.enabled === false && (
                          <StatusBadge value="neutral" label="Disabled" />
                        )}
                      </td>
                      <td className="shrink">
                        <StatusBadge value={item.usability} />
                      </td>
                      <td className="shrink numeric">{item.cached_package_count}</td>
                      <td className="shrink muted">{formatDateTime(item.source.last_refreshed_at) || NONE}</td>
                      <td className="shrink">
                        <span className="button-row">
                          <ActionButton
                            action={actions.get("extension.source.refresh", item.source.id)}
                            revisions={revisions}
                            busy={busy}
                            icon="refresh"
                            onInvoke={onInvoke}
              onExpired={onExpired}
                          />
                          <ActionButton
                            action={actions.get("extension.source.enable", item.source.id)}
                            revisions={revisions}
                            busy={busy}
                            icon="play"
                            onInvoke={onInvoke}
              onExpired={onExpired}
                          />
                          <ActionButton
                            action={actions.get("extension.source.disable", item.source.id)}
                            revisions={revisions}
                            busy={busy}
                            icon="pause"
                            onInvoke={onInvoke}
              onExpired={onExpired}
                          />
                          <ActionButton
                            action={actions.get("extension.source.trust", item.source.id)}
                            revisions={revisions}
                            busy={busy}
                            icon="shield-check"
                            onInvoke={onInvoke}
              onExpired={onExpired}
                          />
                          <ActionButton
                            action={actions.get("extension.source.remove", item.source.id)}
                            revisions={revisions}
                            busy={busy}
                            className="button danger"
                            icon="trash"
                            onInvoke={onInvoke}
              onExpired={onExpired}
                          />
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </QueryState>
      </Panel>

      <Panel className="padded stack">
        <PanelHeader title="Out-of-band trust bootstrap" icon="shield" plain />
        <p className="muted">
          Paste root metadata obtained independently of the catalog and confirm its exact SHA-256 fingerprint. This
          changes the canonical extension trust boundary.
        </p>
        <p className="muted">
          Choose a configured source above and accept its signed root there; Draft states
          exactly what it needs and validates the fingerprint itself.
        </p>
      </Panel>
    </div>
  );
}
