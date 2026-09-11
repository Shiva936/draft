import { useEffect, useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../../api";
import type { Resource, ResourceContent, ProjectSummary } from "../../../contracts";
import { Icon } from "../../../icons";
import { Panel, PanelHeader, SearchField } from "../../../components/layout";
import { Menu } from "../../../components/Menu";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState, Skeleton } from "../../../components/states";
import { FILE_SCHEME, basename, dirname } from "../../../lib/format";
import { useUnsavedGuard } from "../../../lib/hooks";
import { AttributionField, type AttributionRef } from "./AttributionField";
import { ResourcePane } from "./ResourcePane";
import { ResourceTree, TreeLegend, changeMap } from "./ResourceTree";
import { ResourceInspector } from "./ResourceInspector";
import { grammarOf, presentationFor, type PresentationBinding } from "./presentation";
import { TreeOperationModal, type TreeOperation } from "./TreeOperationModal";
import { SectionNav } from "../SectionNav";
import { RESOURCE_ROUTES } from "./routes";

/**
 * The resource browser.
 *
 * Opening a resource is read-only. Saving requires an attribution and persists
 * a durable Workspace below excluded `.draft/workspaces/`; it never modifies project
 * state directly. Applying staged work to the canonical context is a separate,
 * explicit commit.
 *
 * Resources are addressed by locator throughout. The browser reads a body only
 * to display it, and only splits one for the `file` scheme — where the owning
 * adapter's bodies genuinely are paths.
 */
export function ResourceBrowser() {
  const { workspaceId = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const queryClient = useQueryClient();

  const [openPaths, setOpenPaths] = useState<string[]>([]);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [filter, setFilter] = useState("");
  const [attribution, setAttribution] = useState<AttributionRef | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [operation, setOperation] = useState<{ kind: TreeOperation; sourcePath?: string } | null>(
    params.get("create") === "1" ? { kind: "create_file" } : null,
  );

  const resourcesQuery = useQuery({
    queryKey: ["resources", workspaceId],
    queryFn: () => api<Resource[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/resources`),
  });
  const project = useQuery({
    queryKey: ["project", workspaceId],
    queryFn: () => api<ProjectSummary>(`/api/v1/projects/${encodeURIComponent(workspaceId)}`),
  });
  // Read-only: which presentation applies is Draft's resolution, and an
  // ambiguous one is reported rather than silently settled here.
  const presentationQuery = useQuery({
    queryKey: ["presentation", workspaceId],
    queryFn: () =>
      api<PresentationBinding[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/presentation`),
  });
  const contentQuery = useQuery({
    queryKey: ["resource", workspaceId, activePath],
    enabled: Boolean(activePath),
    queryFn: () =>
      api<ResourceContent>(
        `/api/v1/projects/${encodeURIComponent(workspaceId)}/resource?body=${encodeURIComponent(activePath!)}`,
      ),
  });

  const resources = resourcesQuery.data ?? [];
  const changes = useMemo(() => changeMap(project.data?.status), [project.data]);
  const activeResource = resources.find((resource) => resource.locator.body === activePath) ?? null;
  const activePresentation = presentationFor(presentationQuery.data, activeResource?.resource_id);
  const dirty =
    activePath !== null &&
    drafts[activePath] !== undefined &&
    drafts[activePath] !== contentQuery.data?.content;
  const anyDirty = Object.keys(drafts).length > 0;
  useUnsavedGuard(anyDirty);

  const save = useMutation({
    mutationFn: () =>
      mutate<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/resource-save`, {
        change_workspace: sessionId,
        attribution,
        resource_locator: { scheme: FILE_SCHEME, body: activePath },
        content: drafts[activePath!] ?? contentQuery.data?.content ?? "",
      }),
    onSuccess: (session) => {
      setSessionId(session.id);
      setDrafts((current) => {
        const next = { ...current };
        if (activePath) delete next[activePath];
        return next;
      });
      void queryClient.invalidateQueries({ queryKey: ["project", workspaceId] });
    },
  });

  const commit = useMutation({
    mutationFn: () =>
      mutate(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/resource-commit`, { change_workspace: sessionId }),
    onSuccess: () => {
      setSessionId(null);
      setAttribution(null);
      void queryClient.invalidateQueries({ queryKey: ["resources", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["resource", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["project", workspaceId] });
    },
  });

  const open = (path: string) => {
    setOpenPaths((current) => (current.includes(path) ? current : [...current, path]));
    setActivePath(path);
  };

  const close = (path: string) => {
    setOpenPaths((current) => {
      const next = current.filter((entry) => entry !== path);
      if (activePath === path) setActivePath(next[next.length - 1] ?? null);
      return next;
    });
    setDrafts((current) => {
      const next = { ...current };
      delete next[path];
      return next;
    });
  };

  const closeOperation = () => {
    setOperation(null);
    if (params.has("create")) {
      params.delete("create");
      setParams(params, { replace: true });
    }
  };

  useEffect(() => {
    if (
      activePath &&
      resources.length > 0 &&
      !resources.some((resource) => resource.locator.body === activePath)
    ) {
      close(activePath);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resources]);

  const canSave = Boolean(activePath) && dirty && Boolean(attribution) && !activeResource?.protected;

  return (
    <>
      <SectionNav section="Resources" routes={RESOURCE_ROUTES} />
      <div className="toolbar">
        <AttributionField
          workspaceId={workspaceId}
          value={attribution}
          locked={Boolean(sessionId)}
          onChange={setAttribution}
          label=""
        />
        {sessionId && <StatusBadge value="workspace open" tone="running" label={`Workspace ${sessionId}`} />}
        <span className="spacer" />
        <button
          className="button"
          disabled={!canSave || save.isPending}
          title={attribution ? undefined : "Choose a task or change attribution before saving"}
          onClick={() => save.mutate()}
        >
          <Icon name="download" size={16} />
          {save.isPending ? "Saving…" : "Save session"}
          <kbd>⌘S</kbd>
        </button>
        <button
          className="button primary"
          disabled={!sessionId || anyDirty || commit.isPending}
          title={anyDirty ? "Save the open Workspace before committing" : undefined}
          onClick={() => commit.mutate()}
        >
          <Icon name="check" size={16} />
          {commit.isPending ? "Committing…" : "Commit workspace"}
        </button>
      </div>

      <InlineError error={save.error} />
      <InlineError error={commit.error} />

      <div className="resource-workbench">
        <Panel className="flush" style={{ display: "flex", flexDirection: "column", minHeight: 0 }}>
          <PanelHeader
            title="Resources"
            icon="folder"
            action={
              <Menu
                label="Resource tree actions"
                items={[
                  { label: "New resource", icon: "file", onSelect: () => setOperation({ kind: "create_file" }) },
                  { label: "New collection", icon: "folder", onSelect: () => setOperation({ kind: "create_directory" }) },
                  { kind: "separator" },
                  { label: "Relocate…", icon: "pencil", onSelect: () => setOperation({ kind: "rename", sourcePath: activePath ?? undefined }) },
                  {
                    label: "Remove resources…",
                    icon: "trash",
                    danger: true,
                    onSelect: () => setOperation({ kind: "delete", sourcePath: activePath ?? undefined }),
                  },
                ]}
                trigger={({ open: menuOpen, toggle }) => (
                  <div className="button-row">
                    <button className="button small" onClick={() => setOperation({ kind: "create_file" })}>
                      <Icon name="plus" size={14} />
                      New resource
                    </button>
                    <button
                      className={menuOpen ? "icon-button active" : "icon-button"}
                      onClick={toggle}
                      aria-haspopup="menu"
                      aria-expanded={menuOpen}
                      aria-label="Resource tree actions"
                    >
                      <Icon name="more-vertical" size={16} />
                    </button>
                  </div>
                )}
              />
            }
          />
          <div className="panel-body" style={{ paddingBottom: 0 }}>
            <SearchField label="Search resources" placeholder="Search resources…" value={filter} onChange={setFilter} />
          </div>
          <QueryState
            query={resourcesQuery}
            skeletonRows={6}
            empty={
              <EmptyState
                inline
                icon="file"
                label="No resources."
                detail="Draft's own control plane is never part of project state."
              />
            }
          >
            {(list: Resource[]) => (
              <ResourceTree
                resources={list}
                changes={changes}
                selected={activePath}
                filter={filter}
                onSelect={open}
              />
            )}
          </QueryState>
          <TreeLegend />
        </Panel>

        <Panel className="flush resource-main">
          {openPaths.length === 0 ? (
            <EmptyState
              icon="file-text"
              label="Select a resource to open it."
              detail="Opening a resource is read-only until you choose a task or change attribution."
            />
          ) : (
            <>
              <div className="resource-tabs" role="tablist" aria-label="Open resources">
                {openPaths.map((path) => (
                  <div key={path} className={path === activePath ? "resource-tab active" : "resource-tab"}>
                    <button
                      role="tab"
                      aria-selected={path === activePath}
                      onClick={() => setActivePath(path)}
                      style={{ all: "unset", cursor: "pointer", display: "flex", alignItems: "center", gap: 6 }}
                    >
                      <Icon name="file" size={14} />
                      {basename(path)}
                      {drafts[path] !== undefined && <span className="dirty-dot" />}
                    </button>
                    <button className="close" onClick={() => close(path)} aria-label={`Close ${basename(path)}`}>
                      <Icon name="x" size={12} />
                    </button>
                  </div>
                ))}
              </div>

              {activePath && (
                <div className="resource-path">
                  <Icon name="folder" size={12} />
                  <span>{dirname(activePath) || "."}</span>
                  <Icon name="chevron-right" size={12} />
                  <span className="mono">{basename(activePath)}</span>
                  <span className="spacer" />
                  {attribution && (
                    <span className="chip">
                      <Icon name="list-checks" size={12} />
                      {attribution.kind} · {attribution.id}
                    </span>
                  )}
                  {activeResource?.protected && <StatusBadge value="protected" tone="warning" />}
                </div>
              )}

              {contentQuery.isLoading ? (
                <Skeleton rows={8} />
              ) : contentQuery.data && activePath ? (
                <ResourcePane
                  key={activePath}
                  resourceId={activeResource?.resource_id ?? activePath}
                  resource={activeResource ?? undefined}
                  grammar={grammarOf(activePresentation)}
                  content={contentQuery.data.content}
                  readOnly={Boolean(activeResource?.protected)}
                  onChange={(value) => setDrafts((current) => ({ ...current, [activePath]: value }))}
                  onSave={() => {
                    if (canSave) save.mutate();
                  }}
                />
              ) : (
                <EmptyState inline icon="file" label="This resource is empty." />
              )}
            </>
          )}
        </Panel>

        <div className="file-inspector stack">
          <ResourceInspector
            resource={activeResource}
            content={contentQuery.data ?? null}
            presentation={activePresentation}
            dirty={dirty}
            sessionId={sessionId}
            canSave={canSave}
            busy={save.isPending || commit.isPending}
            tasks={project.data?.tasks ?? []}
            onSave={() => save.mutate()}
            onOperation={(kind) => setOperation({ kind, sourcePath: activePath ?? undefined })}
          />
        </div>
      </div>

      {operation && (
        <TreeOperationModal
          workspaceId={workspaceId}
          operation={operation.kind}
          sourcePath={operation.sourcePath}
          sessionId={sessionId}
          attribution={attribution}
          onAttributionChange={setAttribution}
          onStaged={setSessionId}
          onClose={closeOperation}
        />
      )}
    </>
  );
}
