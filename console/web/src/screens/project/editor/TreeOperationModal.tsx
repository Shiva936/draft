import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate } from "../../../api";
import { Modal } from "../../../components/Modal";
import { InlineError } from "../../../components/states";
import { AttributionField, type AttributionRef } from "./AttributionField";

export type TreeOperation = "create_file" | "create_directory" | "rename" | "delete";

const copy: Record<TreeOperation, { title: string; submit: string; note: string }> = {
  create_file: {
    title: "New file",
    submit: "Stage file",
    note: "Creation is staged in a durable editor session and applied on commit.",
  },
  create_directory: {
    title: "New folder",
    submit: "Stage folder",
    note: "Creation is staged in a durable editor session and applied on commit.",
  },
  rename: {
    title: "Rename or move",
    submit: "Stage move",
    note: "The move is staged in a durable editor session and applied on commit.",
  },
  delete: {
    title: "Delete tree",
    submit: "Stage deletion",
    note: "The edit session keeps a recovery backup and requires an explicit commit.",
  },
};

/**
 * Staged create, move, and recursive delete.
 *
 * Every tree edit inherits one durable attribution, which cannot change once a
 * session is open, and recursive deletion keeps its explicit confirmation.
 */
export function TreeOperationModal({
  workspaceId,
  operation,
  sourcePath,
  sessionId,
  attribution,
  onAttributionChange,
  onStaged,
  onClose,
}: {
  workspaceId: string;
  operation: TreeOperation;
  sourcePath?: string;
  sessionId: string | null;
  attribution: AttributionRef | null;
  onAttributionChange: (value: AttributionRef | null) => void;
  onStaged: (sessionId: string) => void;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [source, setSource] = useState(sourcePath ?? "");
  const [destination, setDestination] = useState("");
  const text = copy[operation];

  const stage = useMutation({
    mutationFn: () => {
      const action =
        operation === "rename" ? "editor-rename" : operation === "delete" ? "editor-delete" : "editor-create";
      const body =
        operation === "rename"
          ? { session_id: sessionId, attribution, edit_kind: "rename", from: source, to: destination }
          : operation === "delete"
            ? { session_id: sessionId, attribution, edit_kind: "delete", file_path: source, recursive: true }
            : { session_id: sessionId, attribution, edit_kind: operation, file_path: destination };
      return mutate<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/${action}`, body);
    },
    onSuccess: (session) => {
      onStaged(session.session_id);
      void queryClient.invalidateQueries({ queryKey: ["files", workspaceId] });
      onClose();
    },
  });

  const run = () => {
    if (
      operation === "delete" &&
      !window.confirm(
        `Stage recursive deletion of ${source}?\n\nThe edit session keeps a recovery backup and requires an explicit commit.`,
      )
    )
      return;
    stage.mutate();
  };

  const invalid =
    !attribution ||
    (operation === "delete" ? !source.trim() : !destination.trim()) ||
    (operation === "rename" && !source.trim());

  return (
    <Modal
      title={text.title}
      onClose={onClose}
      footer={
        <>
          <button className="button" onClick={onClose}>
            Cancel
          </button>
          <button
            className={operation === "delete" ? "button danger" : "button primary"}
            disabled={invalid || stage.isPending}
            onClick={run}
          >
            {stage.isPending ? "Staging…" : text.submit}
          </button>
        </>
      }
    >
      <AttributionField
        workspaceId={workspaceId}
        value={attribution}
        locked={Boolean(sessionId)}
        onChange={onAttributionChange}
      />

      {(operation === "rename" || operation === "delete") && (
        <label className="field">
          <span>Source path</span>
          <input
            className="input"
            aria-label="Source path"
            placeholder="Source path"
            value={source}
            onChange={(event) => setSource(event.target.value)}
          />
        </label>
      )}

      {operation !== "delete" && (
        <label className="field">
          <span>{operation === "rename" ? "Destination path" : "New path"}</span>
          <input
            className="input"
            aria-label="Destination path"
            placeholder={operation === "rename" ? "Destination path" : "New path"}
            value={destination}
            onChange={(event) => setDestination(event.target.value)}
          />
        </label>
      )}

      <p className="muted">{text.note}</p>
      <InlineError error={stage.error} />
    </Modal>
  );
}
