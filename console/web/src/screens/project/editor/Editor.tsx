import { useEffect, useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../../api";
import type { EditorFile, EditorFileView, ProjectSummary } from "../../../contracts";
import { Icon } from "../../../icons";
import { Panel, PanelHeader, SearchField } from "../../../components/layout";
import { Menu } from "../../../components/Menu";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState, Skeleton } from "../../../components/states";
import { basename, dirname } from "../../../lib/format";
import { useUnsavedGuard } from "../../../lib/hooks";
import { AttributionField, type AttributionRef } from "./AttributionField";
import { CodePane } from "./CodePane";
import { FileTree, TreeLegend, changeMap } from "./FileTree";
import { FileInspector } from "./FileInspector";
import { TreeOperationModal, type TreeOperation } from "./TreeOperationModal";

/**
 * Draft-native editor.
 *
 * Opening a file is read-only. Saving requires an attribution and persists a
 * durable editor session below excluded `.draft/editor/`; it never modifies
 * source files. Applying staged work to the canonical context is a separate,
 * explicit commit.
 */
export function Editor() {
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

  const filesQuery = useQuery({
    queryKey: ["files", workspaceId],
    queryFn: () => api<EditorFile[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/files`),
  });
  const project = useQuery({
    queryKey: ["project", workspaceId],
    queryFn: () => api<ProjectSummary>(`/api/v1/projects/${encodeURIComponent(workspaceId)}`),
  });
  const fileQuery = useQuery({
    queryKey: ["file", workspaceId, activePath],
    enabled: Boolean(activePath),
    queryFn: () => api<EditorFileView>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/file?path=${encodeURIComponent(activePath!)}`),
  });

  const files = filesQuery.data ?? [];
  const changes = useMemo(() => changeMap(project.data?.status), [project.data]);
  const activeFile = files.find((file) => file.path === activePath) ?? null;
  const dirty = activePath !== null && drafts[activePath] !== undefined && drafts[activePath] !== fileQuery.data?.content;
  const anyDirty = Object.keys(drafts).length > 0;
  useUnsavedGuard(anyDirty);

  const save = useMutation({
    mutationFn: () =>
      mutate<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/editor-save`, {
        session_id: sessionId,
        attribution,
        file_path: activePath,
        content: drafts[activePath!] ?? fileQuery.data?.content ?? "",
      }),
    onSuccess: (session) => {
      setSessionId(session.session_id);
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
      mutate(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/editor-commit`, { session_id: sessionId }),
    onSuccess: () => {
      setSessionId(null);
      setAttribution(null);
      void queryClient.invalidateQueries({ queryKey: ["files", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["file", workspaceId] });
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
    if (activePath && !files.some((file) => file.path === activePath) && files.length > 0) {
      close(activePath);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [files]);

  const canSave = Boolean(activePath) && dirty && Boolean(attribution) && !activeFile?.protected;

  return (
    <>
      <div className="toolbar">
        <AttributionField
          workspaceId={workspaceId}
          value={attribution}
          locked={Boolean(sessionId)}
          onChange={setAttribution}
          label=""
        />
        {sessionId && <StatusBadge value="session open" tone="running" label={`Session ${sessionId}`} />}
        <span className="spacer" />
        <button
          className="button"
          disabled={!canSave || save.isPending}
          title={attribution ? undefined : "Choose a task or pack attribution before saving"}
          onClick={() => save.mutate()}
        >
          <Icon name="download" size={16} />
          {save.isPending ? "Saving…" : "Save session"}
          <kbd>⌘S</kbd>
        </button>
        <button
          className="button primary"
          disabled={!sessionId || anyDirty || commit.isPending}
          title={anyDirty ? "Save the open editor session before committing" : undefined}
          onClick={() => commit.mutate()}
        >
          <Icon name="check" size={16} />
          {commit.isPending ? "Committing…" : "Commit to context"}
        </button>
      </div>

      <InlineError error={save.error} />
      <InlineError error={commit.error} />

      <div className="editor-workbench">
        <Panel className="flush" style={{ display: "flex", flexDirection: "column", minHeight: 0 }}>
          <PanelHeader
            title="Files"
            icon="folder"
            action={
              <Menu
                label="File tree actions"
                items={[
                  { label: "New file", icon: "file", onSelect: () => setOperation({ kind: "create_file" }) },
                  { label: "New folder", icon: "folder", onSelect: () => setOperation({ kind: "create_directory" }) },
                  { kind: "separator" },
                  { label: "Rename or move…", icon: "pencil", onSelect: () => setOperation({ kind: "rename", sourcePath: activePath ?? undefined }) },
                  {
                    label: "Delete tree…",
                    icon: "trash",
                    danger: true,
                    onSelect: () => setOperation({ kind: "delete", sourcePath: activePath ?? undefined }),
                  },
                ]}
                trigger={({ open: menuOpen, toggle }) => (
                  <div className="button-row">
                    <button className="button small" onClick={() => setOperation({ kind: "create_file" })}>
                      <Icon name="plus" size={14} />
                      New file
                    </button>
                    <button
                      className={menuOpen ? "icon-button active" : "icon-button"}
                      onClick={toggle}
                      aria-haspopup="menu"
                      aria-expanded={menuOpen}
                      aria-label="File tree actions"
                    >
                      <Icon name="more-vertical" size={16} />
                    </button>
                  </div>
                )}
              />
            }
          />
          <div className="panel-body" style={{ paddingBottom: 0 }}>
            <SearchField label="Search files" placeholder="Search files…" value={filter} onChange={setFilter} />
          </div>
          <QueryState
            query={filesQuery}
            skeletonRows={6}
            empty={<EmptyState inline icon="file" label="No source files." detail="Draft control paths are excluded from the source view." />}
          >
            {(list: EditorFile[]) => (
              <FileTree files={list} changes={changes} selected={activePath} filter={filter} onSelect={open} />
            )}
          </QueryState>
          <TreeLegend />
        </Panel>

        <Panel className="flush editor-main">
          {openPaths.length === 0 ? (
            <EmptyState
              icon="file-text"
              label="Select a source file to open it."
              detail="Opening a file is read-only until you choose a task or pack attribution."
            />
          ) : (
            <>
              <div className="editor-tabs" role="tablist" aria-label="Open files">
                {openPaths.map((path) => (
                  <div key={path} className={path === activePath ? "editor-tab active" : "editor-tab"}>
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
                <div className="editor-path">
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
                  {activeFile?.protected && <StatusBadge value="protected" tone="warning" />}
                </div>
              )}

              {fileQuery.isLoading ? (
                <Skeleton rows={8} />
              ) : fileQuery.data && activePath ? (
                <CodePane
                  key={activePath}
                  path={activePath}
                  content={fileQuery.data.content}
                  readOnly={Boolean(activeFile?.protected)}
                  onChange={(value) => setDrafts((current) => ({ ...current, [activePath]: value }))}
                  onSave={() => {
                    if (canSave) save.mutate();
                  }}
                />
              ) : (
                <EmptyState inline icon="file" label="File is empty." />
              )}
            </>
          )}
        </Panel>

        <div className="file-inspector stack">
          <FileInspector
            file={activeFile}
            view={fileQuery.data ?? null}
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
