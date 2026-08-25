import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate, settleJob } from "../../api";
import type { DiscoveredExtensionDto, ExtensionCatalogSourceStatusDto, ServiceJob } from "../../contracts";
import { Icon } from "../../icons";
import { Definitions, PageHeader, Panel, SearchField, Tabs, Toolbar } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { DetailDrawer } from "../../components/DetailDrawer";
import { Menu } from "../../components/Menu";
import { JobBanner } from "../../components/JobBanner";
import { EmptyState, InlineError, QueryState } from "../../components/states";
import { NONE, formatDateTime, humanize, shortDigest } from "../../lib/format";
import { TrustSourcesPanel } from "./TrustSourcesPanel";

type InstalledExtension = {
  manifest: {
    id: string;
    name: string;
    version: string;
    publisher: string;
    draft_api: string;
    contributions?: { id: string; kind: string }[];
    documentation?: string[];
    licenses?: string[];
  };
  content_hash: string;
  enabled: boolean;
  installed_at: string;
  provenance?: any;
};

/**
 * Signed catalogs, declarative packages, and canonical enablement state.
 *
 * Configuration never implies trust: sources, trust roots, and usable
 * discovery are presented as three distinct states, exactly as core computes
 * them, and every mutation keeps its contextual confirmation.
 */
export function Extensions() {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState("installed");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [job, setJob] = useState<ServiceJob | null>(null);

  const installed = useQuery({ queryKey: ["extensions"], queryFn: () => api<InstalledExtension[]>("/api/v1/extensions") });
  const sources = useQuery({
    queryKey: ["extension-sources"],
    queryFn: () => api<ExtensionCatalogSourceStatusDto[]>("/api/v1/extensions/sources"),
  });
  const discovery = useQuery({
    queryKey: ["extension-discovery", search],
    enabled: tab === "discover",
    queryFn: () => api<DiscoveredExtensionDto[]>(`/api/v1/extensions/discover?q=${encodeURIComponent(search)}`),
  });

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["extensions"] });
    void queryClient.invalidateQueries({ queryKey: ["extension-sources"] });
    void queryClient.invalidateQueries({ queryKey: ["extension-discovery"] });
  };

  const action = useMutation({
    mutationFn: async ({ path, body }: { path: string; body?: unknown }) =>
      settleJob(await mutate<any>(path, body ?? {}), setJob),
    onSuccess: () => {
      setJob(null);
      refresh();
    },
    onError: () => setJob(null),
  });

  const confirm = (message: string, path: string, body: unknown = {}) => {
    if (window.confirm(message)) action.mutate({ path, body });
  };

  const extensions = installed.data ?? [];
  const active = extensions.find((extension) => extension.manifest.id === selected) ?? null;
  const updates = useMemo(
    () =>
      (discovery.data ?? []).filter((item) => {
        const current = extensions.find((extension) => extension.manifest.id === item.target.id);
        return current && current.manifest.version !== item.target.version;
      }),
    [discovery.data, extensions],
  );

  const filteredInstalled = extensions.filter((extension) =>
    `${extension.manifest.id} ${extension.manifest.name} ${extension.manifest.publisher}`
      .toLowerCase()
      .includes(search.trim().toLowerCase()),
  );

  return (
    <div className="page">
      <PageHeader
        title="Extensions"
        icon="puzzle"
        subtitle="Signed catalogs, declarative packages, and canonical enablement state."
        status={<StatusBadge value={extensions.length > 0 ? "healthy" : "neutral"} label={`${extensions.length} installed`} />}
        actions={
          <Menu
            label="Extension actions"
            items={[
              { label: "Refresh state", icon: "refresh", onSelect: refresh },
              {
                label: "Update all",
                icon: "download",
                disabled: extensions.length === 0 || action.isPending,
                reason: "No extensions are installed",
                onSelect: () =>
                  confirm(
                    "Update every installed catalog extension to the newest currently trusted compatible target? Prior versions will be preserved for rollback after failures.",
                    "/api/v1/extensions/actions/update-all",
                  ),
              },
            ]}
            trigger={({ open, toggle }) => (
              <button className="button" onClick={toggle} aria-haspopup="menu" aria-expanded={open}>
                <Icon name="more-vertical" size={16} />
                Actions
              </button>
            )}
          />
        }
      />

      <JobBanner job={job} />
      <InlineError error={action.error} />

      <Tabs
        label="Extension views"
        active={tab}
        onSelect={setTab}
        tabs={[
          { id: "installed", label: "Installed", count: extensions.length },
          { id: "sources", label: "Catalog sources", count: sources.data?.length ?? 0 },
          { id: "discover", label: "Discover" },
          { id: "updates", label: "Updates", count: updates.length },
        ]}
      />

      {tab === "sources" ? (
        <TrustSourcesPanel
          sources={sources}
          busy={action.isPending}
          onAction={(path, body) => action.mutate({ path, body })}
          onConfirm={confirm}
        />
      ) : (
        <>
          <Toolbar>
            <SearchField
              label="Search extensions"
              placeholder={tab === "discover" ? "Search signed catalogs…" : "Search extensions…"}
              value={search}
              onChange={setSearch}
            />
            <span className="spacer" />
            <span className="result-count">
              {tab === "discover"
                ? `${discovery.data?.length ?? 0} catalog packages`
                : tab === "updates"
                  ? `${updates.length} available`
                  : `${filteredInstalled.length} installed`}
            </span>
          </Toolbar>

          <div className={active && tab === "installed" ? "workbench with-detail" : "workbench"}>
            <Panel className="flush">
              {tab === "installed" && (
                <QueryState
                  query={installed}
                  skeletonRows={4}
                  empty={
                    <EmptyState
                      icon="puzzle"
                      label="No extensions are installed."
                      detail="Draft ships no pretrusted remote catalog. Configure a source and accept its signed root before installing."
                      action={
                        <button className="button" onClick={() => setTab("sources")}>
                          Configure catalog sources
                        </button>
                      }
                    />
                  }
                >
                  {() =>
                    filteredInstalled.length === 0 ? (
                      <EmptyState inline icon="search" label="No installed extension matches this search." />
                    ) : (
                      <div className="rows">
                        {filteredInstalled.map((extension) => (
                          <button
                            key={extension.manifest.id}
                            className={
                              extension.manifest.id === selected ? "extension-row selected" : "extension-row"
                            }
                            onClick={() => setSelected(extension.manifest.id)}
                          >
                            <span className="extension-mark">
                              <Icon name="puzzle" size={20} />
                            </span>
                            <span className="extension-body">
                              <span className="title-row">
                                <strong>{extension.manifest.name}</strong>
                                <span className="chip">{extension.manifest.publisher}</span>
                              </span>
                              <span className="extension-tags">
                                {extension.manifest.id} · v{extension.manifest.version}
                              </span>
                              {(extension.manifest.contributions ?? []).length > 0 && (
                                <span className="button-row">
                                  {(extension.manifest.contributions ?? []).slice(0, 3).map((contribution) => (
                                    <span className="chip" key={contribution.id}>
                                      {humanize(contribution.kind)}
                                    </span>
                                  ))}
                                </span>
                              )}
                            </span>
                            <span className="extension-state">
                              <StatusBadge value={extension.enabled ? "enabled" : "disabled"} />
                              <span>v{extension.manifest.version}</span>
                            </span>
                          </button>
                        ))}
                      </div>
                    )
                  }
                </QueryState>
              )}

              {(tab === "discover" || tab === "updates") && (
                <QueryState
                  query={discovery}
                  skeletonRows={4}
                  empty={
                    <EmptyState
                      icon="search"
                      label="No catalog packages are available."
                      detail="Discovery lists packages from trusted, unexpired catalogs only."
                    />
                  }
                >
                  {(items: DiscoveredExtensionDto[]) => {
                    const shown = tab === "updates" ? updates : items;
                    if (shown.length === 0)
                      return (
                        <EmptyState
                          inline
                          icon={tab === "updates" ? "check-circle" : "search"}
                          label={tab === "updates" ? "Every installed extension is up to date." : "No catalog packages match this search."}
                        />
                      );
                    return (
                      <div className="rows">
                        {shown.map((item) => {
                          const current = extensions.find((extension) => extension.manifest.id === item.target.id);
                          const mode = current ? "update" : "install";
                          const usable = item.freshness === "usable";
                          return (
                            <div
                              className="extension-row"
                              key={`${item.source_id}-${item.target.id}-${item.target.version}`}
                            >
                              <span className="extension-mark">
                                <Icon name="package" size={20} />
                              </span>
                              <span className="extension-body">
                                <span className="title-row">
                                  <strong>{item.target.id}</strong>
                                  <span className="chip">{item.target.publisher}</span>
                                  <StatusBadge value={item.freshness} />
                                </span>
                                <span className="extension-tags">
                                  v{item.target.version} · {item.source_id} · {shortDigest(item.target.sha256)}
                                </span>
                              </span>
                              <span className="extension-state">
                                <button
                                  className="button primary"
                                  disabled={!usable || action.isPending || current?.manifest.version === item.target.version}
                                  title={usable ? undefined : "Expired or untrusted metadata cannot authorize install"}
                                  onClick={() =>
                                    confirm(
                                      `${humanize(mode)} ${item.target.id} v${item.target.version} from trusted catalog ${item.catalog_id}? Artifact digest: ${item.target.sha256}`,
                                      `/api/v1/extensions/${encodeURIComponent(item.target.id)}/${mode}`,
                                      { source_id: item.source_id, version: item.target.version },
                                    )
                                  }
                                >
                                  {humanize(mode)}
                                </button>
                                {current && <span>installed v{current.manifest.version}</span>}
                              </span>
                            </div>
                          );
                        })}
                      </div>
                    );
                  }}
                </QueryState>
              )}
            </Panel>

            {active && tab === "installed" && (
              <ExtensionDetail
                extension={active}
                busy={action.isPending}
                onToggle={() =>
                  action.mutate({
                    path: `/api/v1/extensions/${encodeURIComponent(active.manifest.id)}/${active.enabled ? "disable" : "enable"}`,
                  })
                }
                onUninstall={() =>
                  confirm(
                    `Uninstall ${active.manifest.id} v${active.manifest.version}? Package bytes will be preserved in recoverable storage and provenance remains in audit history.`,
                    `/api/v1/extensions/${encodeURIComponent(active.manifest.id)}/uninstall`,
                  )
                }
                onClose={() => setSelected(null)}
              />
            )}
          </div>
        </>
      )}
    </div>
  );
}

function ExtensionDetail({
  extension,
  busy,
  onToggle,
  onUninstall,
  onClose,
}: {
  extension: InstalledExtension;
  busy: boolean;
  onToggle: () => void;
  onUninstall: () => void;
  onClose: () => void;
}) {
  const contributions = extension.manifest.contributions ?? [];
  return (
    <DetailDrawer
      title={extension.manifest.name}
      eyebrow={extension.manifest.publisher}
      subtitle={`${extension.manifest.id} · v${extension.manifest.version}`}
      badges={<StatusBadge value={extension.enabled ? "enabled" : "disabled"} />}
      onClose={onClose}
      footer={
        <>
          <button className="button" disabled={busy} onClick={onToggle}>
            <Icon name={extension.enabled ? "pause" : "play"} size={16} />
            {extension.enabled ? "Disable" : "Enable"}
          </button>
          <button className="button danger" disabled={busy} onClick={onUninstall}>
            <Icon name="trash" size={16} />
            Uninstall
          </button>
        </>
      }
    >
      <section className="stack tight">
        <h3>Package</h3>
        <Definitions rows>
          <dt>Extension id</dt>
          <dd className="mono">{extension.manifest.id}</dd>
          <dt>Version</dt>
          <dd>{extension.manifest.version}</dd>
          <dt>Draft API</dt>
          <dd>{extension.manifest.draft_api || NONE}</dd>
          <dt>Content hash</dt>
          <dd className="mono">{shortDigest(extension.content_hash)}</dd>
          <dt>Installed</dt>
          <dd>{formatDateTime(extension.installed_at)}</dd>
          <dt>Licenses</dt>
          <dd>{extension.manifest.licenses?.join(", ") || NONE}</dd>
        </Definitions>
      </section>

      <section className="stack tight">
        <h3>Contributions</h3>
        {contributions.length === 0 ? (
          <p className="muted">This package declares no contributions.</p>
        ) : (
          <div className="rows">
            {contributions.map((contribution) => (
              <div className="row-item" key={contribution.id}>
                <Icon name="zap" size={16} />
                <div className="row-main">
                  <strong>{contribution.id}</strong>
                  <small>{humanize(contribution.kind)}</small>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      {extension.provenance && (
        <section className="stack tight">
          <h3>Provenance</h3>
          <Definitions rows>
            <dt>Package</dt>
            <dd className="mono">{extension.provenance.package_id}</dd>
            <dt>Version</dt>
            <dd>{extension.provenance.package_version}</dd>
            <dt>Operation</dt>
            <dd className="mono">{shortDigest(extension.provenance.operation_id)}</dd>
            <dt>Trust source</dt>
            <dd>{humanize(extension.provenance.trust?.source_kind ?? "unknown")}</dd>
          </Definitions>
          <p className="muted">Installed provenance remains available even when a source is removed or expires.</p>
        </section>
      )}
    </DetailDrawer>
  );
}
