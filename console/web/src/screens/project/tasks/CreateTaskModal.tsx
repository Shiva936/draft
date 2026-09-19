import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate } from "../../../api";
import { Modal } from "../../../components/Modal";
import { InlineError } from "../../../components/states";

/** Creates a canonical task definition with its goal and success criterion. */
export function CreateTaskModal({ workspaceId, onClose }: { workspaceId: string; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [goal, setGoal] = useState("");
  const [success, setSuccess] = useState("");

  const create = useMutation({
    mutationFn: () =>
      mutate(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/task-create`, {
        name,
        goal,
        allowed_zones: [],
        forbidden_zones: [],
        success_criteria: [success],
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["project", workspaceId] });
      onClose();
    },
  });

  const invalid = !name.trim() || !goal.trim() || !success.trim();

  return (
    <Modal
      title="Create task"
      onClose={onClose}
      footer={
        <>
          <button className="button" onClick={onClose}>
            Cancel
          </button>
          <button className="button primary" disabled={invalid || create.isPending} onClick={() => create.mutate()}>
            {create.isPending ? "Creating…" : "Create task"}
          </button>
        </>
      }
    >
      <label className="field">
        <span>Task name</span>
        <input
          className="input"
          aria-label="Task name"
          placeholder="Task name (letters, digits, - or _)"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label className="field">
        <span>Goal</span>
        <input
          className="input"
          aria-label="Task goal"
          placeholder="What this task must achieve"
          value={goal}
          onChange={(event) => setGoal(event.target.value)}
        />
      </label>
      <label className="field">
        <span>Success criterion</span>
        <input
          className="input"
          aria-label="Task success criterion"
          placeholder="How completion is verified"
          value={success}
          onChange={(event) => setSuccess(event.target.value)}
        />
      </label>
      <InlineError error={create.error} />
    </Modal>
  );
}
