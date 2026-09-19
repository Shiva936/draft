import { useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { api, invokeAction, settleJob } from "../../api";
import type {
  CanonicalRevisions,
  DiscoveredExtensionDto,
  ExtensionCatalogSourceStatusDto,
  ServiceJob,
} from "../../contracts";
import { ActionButton, type ActionArguments } from "../../components/actions";
import {
  ActionIndex,
  projected,
  useConsoleModel,
  useRefreshConsoleModel,
} from "../../lib/consoleModel";
import { Icon } from "../../icons";
import { Definitions, PageHeader, Panel, SearchField, Tabs, Toolbar } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { DetailDrawer } from "../../components/DetailDrawer";
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
    permissions?: string[];
    documentation?: string[];
    licenses?: string[];
  };
  content_hash: string;
  enabled: boolean;
  installed_at: string;
  provenance?: any;
  /** Permissions the package asks for. */
  declared_permissions?: string[];
  /** Permissions currently authorized for this exact installed artifact. */
  authorized_permissions?: string[];
  /** Present when the package is installed but some capability is withheld. */
  pending_authorization?: {
    extension_id: string;
    package_version: string;
    missing_permissions: string[];
    superseded_by_update: boolean;
  } | null;
};

/** The wording used wherever a permission is presented for a decision. */
function permissionSummary(permissions: string[]): string {
  return permissions.length === 0 ? "no additional capabilities" : permissions.join(", ");
}

/** What the Global model carries for this screen. */
type ExtensionsContent = {
  extensions?: {
    installed?: InstalledExtension[];
    sources?: ExtensionCatalogSourceStatusDto[];
  };
};

/**
 * Signed catalogs, declarative packages, and canonical enablement state.
 *
 * Configuration never implies trust: sources, trust roots, and usable
 * discovery are presented as three distinct states, exactly as core computes
 * them, and every mutation keeps its contextual confirmation.
 */
export function Extensions() {
  const [tab, setTab] = useState("installed");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [job, setJob] = useState<ServiceJob | null>(null);

  // State and the actions Draft issues arrive together, from one revision.
  const model = useConsoleModel();
  const refresh = useRefreshConsoleModel();
  const content = (model.data?.content ?? {}) as ExtensionsContent;
  const extensions = content.extensions?.installed ?? [];
  const sourceRecords = content.extensions?.sources ?? [];
  const revisions: CanonicalRevisions =
    model.data?.revisions ?? { registry: 0, workspace: null, change_pack: null, policy: null };
  // Indexed by stable machine identity, never by label or list position.
  const actions = useMemo(() => new ActionIndex(model.data?.actions ?? []), [model.data]);

  // Discovery is a separate, ephemeral paged read. Its rows carry candidate
  // identity only — never eligibility, which Draft re-resolves at invocation.
  const discovery = useQuery({
    queryKey: ["extension-discovery", search],
    enabled: tab === "discover",
    queryFn: () => api<DiscoveredExtensionDto[]>(`/api/v1/extensions/discover?q=${encodeURIComponent(search)}`),
  });

  const action = useMutation({
    mutationFn: async ({
      capability,
      args,
    }: {
      capability: string;
      args: ActionArguments;
    }) => settleJob(await invokeAction<any>(capability, revisions, args), setJob),
    onSuccess: () => {
      setJob(null);
      // Never patch lifecycle locally: the next action set is Draft's to
      // reissue, and the capabilities we held are spent.
      refresh();
      void discovery.refetch();
    },
    onError: () => setJob(null),
  });

  const invoke = (capability: string, args: ActionArguments) =>
    action.mutateAsync({ capability, args }).then(() => undefined);

  const active = extensions.find((extension) => extension.manifest.id === selected) ?? null;
  const updates = useMemo(
    () =>
      (discovery.data ?? []).filter((item) =>
        Boolean(actions.get("extension.update", item.target.id)),
      ),
    [discovery.data, actions],
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
          <span className="button-row">
            <button className="button" onClick={refresh}>
              <Icon name="refresh" size={16} />
              Refresh state
            </button>
            {/* Offered, and enabled, only when Draft's own update plan says so. */}
            <ActionButton
              action={actions.get("extension.update_all")}
              revisions={revisions}
              busy={action.isPending}
              icon="download"
              onInvoke={invoke}
              onExpired={refresh}
            />
          </span>
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
          { id: "sources", label: "Catalog sources", count: sourceRecords.length },
          { id: "discover", label: "Discover" },
          { id: "updates", label: "Updates", count: updates.length },
        ]}
      />

      {tab === "sources" ? (
        <TrustSourcesPanel
          sources={sourceRecords}
          query={projected(model, sourceRecords)}
          actions={actions}
          revisions={revisions}
          busy={action.isPending}
          onInvoke={invoke}
          onExpired={refresh}
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
                  query={projected(model, extensions)}
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
                              {(extension.pending_authorization?.missing_permissions?.length ?? 0) >
                                0 && <StatusBadge value="warning" label="Needs authorization" />}
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
                          // A row already installed uses the targeted update
                          // action Draft issued for it; anything else uses the
                          // one parameterized install action, prefilled from
                          // this candidate. The prefill is input, not
                          // eligibility — Draft re-resolves source, trust,
                          // freshness and compatibility when it runs.
                          const update = actions.get("extension.update", item.target.id);
                          const install = actions.get("extension.install");
                          const candidate: ActionArguments = {
                            extension_id: item.target.id,
                            source_id: item.source_id,
                            version: item.target.version,
                          };
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
                                <ActionButton
                                  action={update ?? install}
                                  revisions={revisions}
                                  busy={action.isPending}
                                  className="button primary"
                                  label={update ? "Update" : "Install"}
                                  prefill={candidate}
                                  onInvoke={invoke}
                                  onExpired={refresh}
                                />
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
                actions={actions}
                revisions={revisions}
                busy={action.isPending}
                onInvoke={invoke}
              onExpired={refresh}
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
  actions,
  revisions,
  busy,
  onInvoke,
  onExpired,
  onClose,
}: {
  extension: InstalledExtension;
  actions: ActionIndex;
  revisions: CanonicalRevisions;
  busy: boolean;
  onInvoke: (capability: string, args: ActionArguments) => Promise<void> | void;
  onExpired: () => void;
  onClose: () => void;
}) {
  const contributions = extension.manifest.contributions ?? [];
  const declared = extension.declared_permissions ?? extension.manifest.permissions ?? [];
  const authorized = extension.authorized_permissions ?? [];
  const pending = extension.pending_authorization ?? null;
  return (
    <DetailDrawer
      title={extension.manifest.name}
      eyebrow={extension.manifest.publisher}
      subtitle={`${extension.manifest.id} · v${extension.manifest.version}`}
      badges={<StatusBadge value={extension.enabled ? "enabled" : "disabled"} />}
      onClose={onClose}
      footer={
        <>
          <ActionButton
            action={actions.get("extension.enable", extension.manifest.id)}
            revisions={revisions}
            busy={busy}
            icon="play"
            onInvoke={onInvoke}
            onExpired={onExpired}
          />
          <ActionButton
            action={actions.get("extension.disable", extension.manifest.id)}
            revisions={revisions}
            busy={busy}
            icon="pause"
            onInvoke={onInvoke}
            onExpired={onExpired}
          />
          <ActionButton
            action={actions.get("extension.authorize", extension.manifest.id)}
            revisions={revisions}
            busy={busy}
            className="button primary"
            icon="shield-check"
            onInvoke={onInvoke}
            onExpired={onExpired}
          />
          <ActionButton
            action={actions.get("extension.revoke", extension.manifest.id)}
            revisions={revisions}
            busy={busy}
            icon="key"
            onInvoke={onInvoke}
            onExpired={onExpired}
          />
          <ActionButton
            action={actions.get("extension.update", extension.manifest.id)}
            revisions={revisions}
            busy={busy}
            icon="download"
            onInvoke={onInvoke}
            onExpired={onExpired}
          />
          <ActionButton
            action={actions.get("extension.uninstall", extension.manifest.id)}
            revisions={revisions}
            busy={busy}
            className="button danger"
            icon="trash"
            onInvoke={onInvoke}
            onExpired={onExpired}
          />
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
        <h3>Capabilities</h3>
        {declared.length === 0 ? (
          <p className="muted">
            This package asks for no capabilities. Its contributions are data only.
          </p>
        ) : (
          <>
            <Definitions rows>
              <dt>Requested</dt>
              <dd className="mono">{declared.join(", ")}</dd>
              <dt>Authorized</dt>
              <dd className="mono">{authorized.length > 0 ? authorized.join(", ") : NONE}</dd>
            </Definitions>
            {pending && pending.missing_permissions.length > 0 && (
              <p className="muted">
                {pending.superseded_by_update
                  ? `This build has not been authorized. A grant for an earlier version does not carry over — ${permissionSummary(pending.missing_permissions)} needs authorizing again.`
                  : `Installed and enabled. Its declared commands stay inert until ${permissionSummary(pending.missing_permissions)} is authorized.`}
              </p>
            )}
          </>
        )}
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
