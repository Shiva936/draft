import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate } from "../../../api";
import type { TaskView } from "../../../contracts";
import { Icon } from "../../../icons";
import { Definitions } from "../../../components/layout";
import { DetailDrawer } from "../../../components/DetailDrawer";
import { RiskBadge, StatusBadge } from "../../../components/StatusBadge";
import { CandidateAvatar } from "../../../components/CandidateAvatar";
import { NextAction } from "../../../components/NextAction";
import { InlineError } from "../../../components/states";
import { NONE, daysUntil, formatDate, formatDateTime, humanize } from "../../../lib/format";

const STATUSES = ["open", "in_progress", "blocked", "completed", "cancelled"];
const PRIORITIES = ["low", "normal", "high", "urgent"];

/** Task detail with real lifecycle mutations and the canonical next-action checklist. */
export function TaskDrawer({
  workspaceId,
  view,
  onClose,
}: {
  workspaceId: string;
  view: TaskView;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState(view.task.status ?? "open");
  const [priority, setPriority] = useState(view.task.priority ?? "normal");
  const [nextAction, setNextAction] = useState("");

  const update = useMutation({
    mutationFn: ({ name, body }: { name: string; body: unknown }) =>
      mutate(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/${name}`, body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["project", workspaceId] });
    },
  });

  const dirty = status !== (view.task.status ?? "open") || priority !== (view.task.priority ?? "normal");
  const due = daysUntil(view.task.due_at);
  const changes = view.produced_changes ?? [];

  return (
    <DetailDrawer
      title={view.task.name}
      eyebrow={<span className="mono">{view.task.id}</span>}
      badges={
        <>
          <StatusBadge value={view.task.status ?? "open"} />
          <StatusBadge value={view.task.priority ?? "normal"} />
        </>
      }
      onClose={onClose}
      footer={
        <button
          className="button primary"
          disabled={!dirty || update.isPending}
          onClick={() => update.mutate({ name: "task-update", body: { task: view.task.id, status, priority } })}
        >
          {update.isPending ? "Updating…" : "Update task"}
        </button>
      }
    >
      {view.task.goal && <p className="muted">{view.task.goal}</p>}

      <section className="stack tight">
        <div className="grid-2">
          <label className="field">
            <span>Status</span>
            <select
              className="select"
              aria-label="Task status"
              value={status}
              onChange={(event) => setStatus(event.target.value)}
            >
              {STATUSES.map((value) => (
                <option key={value} value={value}>
                  {humanize(value)}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Priority</span>
            <select
              className="select"
              aria-label="Task priority"
              value={priority}
              onChange={(event) => setPriority(event.target.value)}
            >
              {PRIORITIES.map((value) => (
                <option key={value} value={value}>
                  {humanize(value)}
                </option>
              ))}
            </select>
          </label>
        </div>
      </section>

      {view.recommended_action && (
        <NextAction label={view.recommended_action} detail="Draft's next safe action for this task." />
      )}

      <section className="stack tight">
        <h3>Details</h3>
        <Definitions rows>
          <dt>Assignee</dt>
          <dd>
            {view.task.assignee_ref ? (
              <span className="avatar-label">
                <CandidateAvatar name={view.task.assignee_ref.id} />
                <span>{view.task.assignee_ref.id}</span>
              </span>
            ) : (
              NONE
            )}
          </dd>
          <dt>Due date</dt>
          <dd>
            {view.task.due_at
              ? `${formatDate(view.task.due_at)}${due === null ? "" : due < 0 ? " (overdue)" : ` (${due}d left)`}`
              : NONE}
          </dd>
          <dt>Risk</dt>
          <dd>
            <RiskBadge value={view.task.risk} />
          </dd>
          <dt>Mode</dt>
          <dd>{humanize(view.task.mode) || NONE}</dd>
          <dt>Health</dt>
          <dd>
            <StatusBadge value={view.health} plain />
          </dd>
          <dt>Review state</dt>
          <dd>
            <StatusBadge value={view.review_status} plain />
          </dd>
          <dt>Executions</dt>
          <dd>{view.execution_count}</dd>
          <dt>Evidence</dt>
          <dd>{view.evidence_count}</dd>
          <dt>Updated</dt>
          <dd>{formatDateTime(view.task.updated_at)}</dd>
        </Definitions>
      </section>

      {changes.length > 0 && (
        <section className="stack tight">
          <h3>Produced changes</h3>
          <div className="rows">
            {changes.map((changeId) => (
              <Link className="row-item" key={changeId} to="../graph">
                <Icon name="layers" size={16} />
                <div className="row-main">
                  <strong className="mono">{changeId}</strong>
                </div>
                <Icon name="external-link" size={14} />
              </Link>
            ))}
          </div>
        </section>
      )}

      <section className="stack tight">
        <h3>Next actions</h3>
        {(view.task.next_actions ?? []).length === 0 ? (
          <p className="muted">No checklist actions recorded.</p>
        ) : (
          (view.task.next_actions ?? []).map((action) => (
            <label className="checkbox" key={action.id}>
              <input
                type="checkbox"
                checked={action.completed}
                disabled={update.isPending}
                onChange={(event) =>
                  update.mutate({
                    name: "task-next-action-set",
                    body: { task: view.task.id, action_id: action.id, completed: event.target.checked },
                  })
                }
              />
              {action.label}
            </label>
          ))
        )}
        <div className="button-row">
          <input
            className="input"
            aria-label="New next action"
            placeholder="Checklist action"
            value={nextAction}
            onChange={(event) => setNextAction(event.target.value)}
            style={{ flex: 1 }}
          />
          <button
            className="button"
            disabled={!nextAction.trim() || update.isPending}
            onClick={() => {
              update.mutate({
                name: "task-next-action-add",
                body: { task: view.task.id, label: nextAction.trim() },
              });
              setNextAction("");
            }}
          >
            <Icon name="plus" size={16} />
            Add
          </button>
        </div>
      </section>

      <InlineError error={update.error} />
    </DetailDrawer>
  );
}
