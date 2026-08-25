import { Link } from "react-router-dom";
import type { EditorFile, EditorFileView, TaskDefinition } from "../../../contracts";
import { Icon } from "../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState } from "../../../components/states";
import { basename, formatBytes, languageOf, shortDigest } from "../../../lib/format";
import type { TreeOperation } from "./TreeOperationModal";

/**
 * File metadata, task links, and the real file actions.
 *
 * Actions that need a staged session or an attribution are disabled with the
 * reason rather than hidden, so the panel shape does not shift under the user.
 */
export function FileInspector({
  file,
  view,
  dirty,
  sessionId,
  canSave,
  busy,
  tasks,
  onSave,
  onOperation,
}: {
  file: EditorFile | null;
  view: EditorFileView | null;
  dirty: boolean;
  sessionId: string | null;
  canSave: boolean;
  busy: boolean;
  tasks: TaskDefinition[];
  onSave: () => void;
  onOperation: (operation: TreeOperation) => void;
}) {
  if (!file || !view) {
    return (
      <Panel className="flush">
        <PanelHeader title="File" icon="file" />
        <EmptyState inline icon="file" label="No file selected." detail="Open a file to see its canonical metadata." />
      </Panel>
    );
  }

  return (
    <>
      <Panel className="padded stack tight">
        <PanelHeader title={basename(file.path)} icon="file-text" plain />
        <p className="muted mono" style={{ fontSize: "var(--font-size-eyebrow)" }}>
          {file.path}
        </p>
        <Definitions rows>
          <dt>Status</dt>
          <dd>
            <StatusBadge
              value={dirty ? "modified" : sessionId ? "session saved" : "unchanged"}
              tone={dirty ? "warning" : sessionId ? "running" : "success"}
              plain
            />
          </dd>
          <dt>Size</dt>
          <dd>{formatBytes(file.bytes)}</dd>
          <dt>Language</dt>
          <dd>{languageOf(file.path)}</dd>
          <dt>Revision</dt>
          <dd className="mono">{shortDigest(view.workspace_hash)}</dd>
          <dt>Protected</dt>
          <dd>{file.protected ? "Yes" : "No"}</dd>
        </Definitions>
      </Panel>

      {tasks.length > 0 && (
        <Panel className="flush">
          <PanelHeader title="Task links" icon="list-checks" count={tasks.length} />
          <div className="rows">
            {tasks.slice(0, 4).map((task) => (
              <Link className="row-item" key={task.id} to="../tasks">
                <Icon name="list-checks" size={16} />
                <div className="row-main">
                  <strong>{task.name}</strong>
                  <small className="mono">{task.id}</small>
                </div>
                <StatusBadge value={task.status ?? "open"} plain />
              </Link>
            ))}
          </div>
        </Panel>
      )}

      <Panel className="flush">
        <PanelHeader title="File actions" icon="zap" />
        <div className="file-actions">
          <button
            disabled={!canSave || busy}
            title={canSave ? undefined : "Choose an attribution and edit the file first"}
            onClick={onSave}
          >
            <Icon name="download" size={16} />
            Save
            <kbd>⌘S</kbd>
          </button>
          <button disabled={busy} onClick={() => onOperation("rename")}>
            <Icon name="pencil" size={16} />
            Rename
            <kbd>F2</kbd>
          </button>
          <button disabled={busy} onClick={() => onOperation("rename")}>
            <Icon name="folder-open" size={16} />
            Move
          </button>
          <button disabled={busy} onClick={() => onOperation("create_file")}>
            <Icon name="file" size={16} />
            New file
          </button>
          <button className="danger" disabled={busy || file.protected} title={file.protected ? "Protected control path" : undefined} onClick={() => onOperation("delete")}>
            <Icon name="trash" size={16} />
            Delete
          </button>
        </div>
      </Panel>

      <Panel className="padded">
        <p className="muted">
          {sessionId
            ? "A durable editor session is open. Commit to apply staged work to canonical context."
            : "Saving stages content below .draft/editor/ and never modifies source files directly."}
        </p>
      </Panel>
    </>
  );
}
