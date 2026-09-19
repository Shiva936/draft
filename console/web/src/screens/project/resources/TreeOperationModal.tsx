import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate } from "../../../api";
import { Modal } from "../../../components/Modal";
import { InlineError } from "../../../components/states";
import { AttributionField, type AttributionRef } from "./AttributionField";

export type TreeOperation = "create_file" | "create_directory" | "rename" | "delete";

/*
 * The vocabulary matches the rest of the Console, and matches the model. A
 * button labelled "New resource" that opens a dialog titled "New file" is not
 * merely untidy — the two words mean different things here. A resource is
 * whatever its adapter says it is, and only the `file` scheme's happen to be
 * files; "folder" and "tree" are the same mistake one level up.
 */
const copy: Record<TreeOperation, { title: string; submit: string; note: string }> = {
  create_file: {
    title: "New resource",
    submit: "Stage resource",
    note: "Creation is staged in a durable session and applied on commit.",
  },
  create_directory: {
    title: "New collection",
    submit: "Stage collection",
    note: "Creation is staged in a durable session and applied on commit.",
  },
  rename: {
    title: "Relocate",
    submit: "Stage relocation",
    note: "The relocation is staged in a durable session and applied on commit.",
  },
  delete: {
    title: "Remove resources",
    submit: "Stage removal",
    note: "The edit session keeps a recovery anchor and requires an explicit commit.",
  },
};

/**
 * Staged create, relocate, and recursive remove.
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
        operation === "rename"
          ? "resource-relocate"
          : operation === "delete"
            ? "resource-delete"
            : "resource-create";
      // Locators throughout. The modal builds `file`-scheme locators because
      // that is the adapter the tree browses; it never parses a body.
      const locator = (body: string) => ({ scheme: "file", body });
      const body =
        operation === "rename"
          ? {
              change_workspace: sessionId,
              attribution,
              edit_kind: "relocate",
              from: locator(source ?? ""),
              to: locator(destination),
            }
          : operation === "delete"
            ? {
                change_workspace: sessionId,
                attribution,
                edit_kind: "remove",
                resource_locator: locator(source ?? ""),
                recursive: true,
              }
            : {
                change_workspace: sessionId,
                attribution,
                edit_kind:
                  operation === "create_directory" ? "create_collection" : "set_content",
                resource_locator: locator(destination),
              };
      return mutate<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/${action}`, body);
    },
    onSuccess: (session) => {
      onStaged(session.id);
      void queryClient.invalidateQueries({ queryKey: ["resources", workspaceId] });
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
          <span>Source locator</span>
          <input
            className="input"
            aria-label="Source locator"
            placeholder="Source locator"
            value={source}
            onChange={(event) => setSource(event.target.value)}
          />
        </label>
      )}

      {operation !== "delete" && (
        <label className="field">
          <span>{operation === "rename" ? "Destination locator" : "New locator"}</span>
          <input
            className="input"
            aria-label="Destination locator"
            placeholder={operation === "rename" ? "Destination locator" : "New locator"}
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
