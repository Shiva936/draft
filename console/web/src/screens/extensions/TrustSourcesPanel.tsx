import { useState } from "react";
import type { UseQueryResult } from "@tanstack/react-query";
import type { ExtensionCatalogSourceStatusDto } from "../../contracts";
import { Icon } from "../../icons";
import { Panel, PanelHeader, Toolbar } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { OverflowMenu } from "../../components/Menu";
import { EmptyState, QueryState } from "../../components/states";
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
  busy,
  onAction,
  onConfirm,
}: {
  sources: UseQueryResult<ExtensionCatalogSourceStatusDto[]>;
  busy: boolean;
  onAction: (path: string, body?: unknown) => void;
  onConfirm: (message: string, path: string, body?: unknown) => void;
}) {
  const [sourceId, setSourceId] = useState("");
  const [sourceUrl, setSourceUrl] = useState("");
  const [trustSource, setTrustSource] = useState("");
  const [rootJson, setRootJson] = useState("");
  const [fingerprint, setFingerprint] = useState("");

  return (
    <div className="workbench with-detail">
      <Panel className="flush">
        <PanelHeader
          title="Catalog sources"
          icon="database"
          count={sources.data?.length ?? 0}
          subtitle="Configuration never implies trust. Console accepts HTTPS sources only."
        />
        <div className="panel-body">
          <Toolbar>
            <input
              className="input"
              aria-label="Source ID"
              placeholder="Source ID"
              value={sourceId}
              onChange={(event) => setSourceId(event.target.value)}
              style={{ maxWidth: 200 }}
            />
            <input
              className="input"
              aria-label="HTTPS catalog URL"
              placeholder="https://catalog.example/"
              value={sourceUrl}
              onChange={(event) => setSourceUrl(event.target.value)}
              style={{ flex: 1, minWidth: 220 }}
            />
            <button
              className="button primary"
              disabled={!sourceId || !sourceUrl.startsWith("https://") || busy}
              onClick={() => {
                onAction(`/api/v1/extensions/sources/${encodeURIComponent(sourceId)}/add`, { location: sourceUrl });
                setSourceId("");
                setSourceUrl("");
              }}
            >
              <Icon name="plus" size={16} />
              Configure source
            </button>
          </Toolbar>
        </div>

        <QueryState
          query={sources}
          skeletonRows={3}
          empty={
            <EmptyState
              icon="database"
              label="No extension sources are configured."
              detail="Configure an HTTPS catalog above, then accept its signed root metadata out of band."
            />
          }
        >
          {(items: ExtensionCatalogSourceStatusDto[]) => (
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
                  {items.map((item) => (
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
                      </td>
                      <td className="shrink">
                        <StatusBadge value={item.usability} />
                      </td>
                      <td className="shrink numeric">{item.cached_package_count}</td>
                      <td className="shrink muted">{formatDateTime(item.source.last_refreshed_at) || NONE}</td>
                      <td className="shrink">
                        <OverflowMenu
                          label={`Actions for ${item.source.id}`}
                          items={[
                            {
                              label: "Refresh",
                              icon: "refresh",
                              disabled: !item.trusted || busy,
                              reason: "Accept this source's signed root before refreshing",
                              onSelect: () =>
                                onAction(`/api/v1/extensions/sources/${encodeURIComponent(item.source.id)}/refresh`),
                            },
                            {
                              label: "Remove source",
                              icon: "trash",
                              danger: true,
                              disabled: busy,
                              onSelect: () =>
                                onConfirm(
                                  `Remove source ${item.source.id}? Installed-package provenance and trust history will be retained.`,
                                  `/api/v1/extensions/sources/${encodeURIComponent(item.source.id)}/remove`,
                                ),
                            },
                          ]}
                        />
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
        <label className="field">
          <span>Configured source</span>
          <input
            className="input"
            aria-label="Source to trust"
            placeholder="Configured source ID"
            value={trustSource}
            onChange={(event) => setTrustSource(event.target.value)}
          />
        </label>
        <label className="field">
          <span>Root fingerprint</span>
          <input
            className="input"
            aria-label="Root fingerprint"
            placeholder="sha256:…"
            value={fingerprint}
            onChange={(event) => setFingerprint(event.target.value)}
          />
        </label>
        <label className="field">
          <span>Signed root metadata</span>
          <textarea
            className="textarea"
            aria-label="Signed root metadata"
            placeholder="Signed root metadata JSON"
            value={rootJson}
            onChange={(event) => setRootJson(event.target.value)}
          />
        </label>
        <button
          className="button danger"
          disabled={!trustSource || !fingerprint.startsWith("sha256:") || !rootJson || busy}
          onClick={() =>
            onConfirm(
              `Trust root ${fingerprint} for source ${trustSource}? This changes the canonical extension trust boundary.`,
              `/api/v1/extensions/sources/${encodeURIComponent(trustSource)}/trust`,
              { root_json: rootJson, fingerprint },
            )
          }
        >
          <Icon name="shield-check" size={16} />
          Accept trust root
        </button>
      </Panel>
    </div>
  );
}
