import { Link } from "react-router-dom";
import type { Resource, ResourceContent, TaskDefinition } from "../../../contracts";
import type { PresentationBinding } from "./presentation";
import { Icon } from "../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState } from "../../../components/states";
import { NONE } from "../../../lib/format";
import { formatBytes, resourceClassLabels, resourceLabel, shortDigest } from "../../../lib/format";
import type { TreeOperation } from "./TreeOperationModal";

/**
 * Resource metadata, task links, and the real resource actions.
 *
 * Classification is shown as the **list** it is: a resource is legitimately a
 * text document *and* a language source at once, and showing one would discard
 * a correct assignment. Actions that need a staged session or an attribution
 * are disabled with the reason rather than hidden, so the panel shape does not
 * shift under the user.
 */
export function ResourceInspector({
  resource,
  content,
  presentation,
  dirty,
  sessionId,
  canSave,
  busy,
  tasks,
  onSave,
  onOperation,
}: {
  resource: Resource | null;
  content: ResourceContent | null;
  presentation: PresentationBinding | null;
  dirty: boolean;
  sessionId: string | null;
  canSave: boolean;
  busy: boolean;
  tasks: TaskDefinition[];
  onSave: () => void;
  onOperation: (operation: TreeOperation) => void;
}) {
  if (!resource || !content) {
    return (
      <Panel className="flush">
        <PanelHeader title="Resource" icon="file" />
        <EmptyState
          inline
          icon="file"
          label="No resource selected."
          detail="Open a resource to see its authoritative state."
        />
      </Panel>
    );
  }

  const classes = resourceClassLabels(resource);

  return (
    <>
      <Panel className="padded stack tight">
        <PanelHeader title={resourceLabel(resource.locator)} icon="file-text" plain />
        <p className="muted mono" style={{ fontSize: "var(--font-size-eyebrow)" }}>
          {resource.locator.scheme}:{resource.locator.body}
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
          <dd>{resource.content_size === null ? NONE : formatBytes(resource.content_size)}</dd>
          <dt>Classes</dt>
          <dd>
            {classes.length === 0 ? (
              <span className="muted">Unclassified — no installed extension recognizes this</span>
            ) : (
              classes.map((className) => (
                <span className="chip mono" key={className}>
                  {className}
                </span>
              ))
            )}
          </dd>
          {resource.class_collisions.length > 0 && (
            <>
              <dt>Disputed</dt>
              <dd>
                {resource.class_collisions.map((className) => (
                  <span className="chip mono warning" key={className}>
                    {className}
                  </span>
                ))}
              </dd>
            </>
          )}
          <dt>Presented by</dt>
          <dd>
            {presentation == null || presentation.state === "fallback" ? (
              <span className="muted">
                Neutral fallback — no installed extension binds a presentation to this
              </span>
            ) : presentation.state === "ambiguous" ? (
              <span className="warning">
                {presentation.candidates.length} publishers bind this equally. Draft will not pick
                one; choose a presentation to resolve it.
              </span>
            ) : (
              <span className="chip mono">{presentation.presentation_id}</span>
            )}
          </dd>
          <dt>Revision</dt>
          <dd className="mono">{shortDigest(content.workspace_hash)}</dd>
          <dt>Protected</dt>
          <dd>{resource.protected ? "Yes" : "No"}</dd>
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
        <PanelHeader title="Resource actions" icon="zap" />
        <div className="file-actions">
          <button
            disabled={!canSave || busy}
            title={canSave ? undefined : "Choose an attribution and edit the resource first"}
            onClick={onSave}
          >
            <Icon name="download" size={16} />
            Save
            <kbd>⌘S</kbd>
          </button>
          <button disabled={busy} onClick={() => onOperation("rename")}>
            <Icon name="pencil" size={16} />
            Relocate
            <kbd>F2</kbd>
          </button>
          <button disabled={busy} onClick={() => onOperation("create_file")}>
            <Icon name="file" size={16} />
            New resource
          </button>
          <button
            className="danger"
            disabled={busy || resource.protected}
            title={resource.protected ? "Protected control path" : undefined}
            onClick={() => onOperation("delete")}
          >
            <Icon name="trash" size={16} />
            Delete
          </button>
        </div>
      </Panel>

      <Panel className="padded">
        <p className="muted">
          {sessionId
            ? "A durable session is open. Commit to apply staged work to the canonical context."
            : "Saving stages content below .draft/workspaces/ and never modifies project state directly."}
        </p>
      </Panel>
    </>
  );
}
