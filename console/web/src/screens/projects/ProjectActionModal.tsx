import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate } from "../../api";
import type { RegistryProject } from "../../contracts";
import { Modal } from "../../components/Modal";
import { InlineError } from "../../components/states";
import { humanize } from "../../lib/format";

export type ProjectActionMode = "register" | "init" | "relocate" | "adopt-copy";

const copy: Record<ProjectActionMode, { title: string; description: string; pathLabel: string; submit: string }> = {
  register: {
    title: "Register existing project",
    description: "Register a Draft project that already exists at an explicit path on this machine.",
    pathLabel: "Explicit project path",
    submit: "Register",
  },
  init: {
    title: "Initialize new project",
    description: "Create a new Draft project at an explicit path and register its immutable workspace id.",
    pathLabel: "Explicit project path",
    submit: "Initialize",
  },
  relocate: {
    title: "Relocate project",
    description:
      "Use relocate only when the destination already contains the same workspace identity. Filesystem location is mutable metadata; the workspace id is not.",
    pathLabel: "Verified destination path",
    submit: "Relocate",
  },
  "adopt-copy": {
    title: "Adopt copy",
    description:
      "Adopt a copied project under an independent identity. Draft first preserves the copied .draft/ bytes and records source identity, digest, and an adoption receipt.",
    pathLabel: "Copied project path",
    submit: "Adopt copy",
  },
};

/**
 * Runs the registry project actions. The confirmation wording for relocate and
 * adopt-copy is unchanged from the CLI-equivalent flow: both cross an identity
 * boundary and are audited.
 */
export function ProjectActionModal({
  mode,
  projects,
  presetWorkspaceId,
  onClose,
}: {
  mode: ProjectActionMode;
  projects: RegistryProject[];
  presetWorkspaceId?: string;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [path, setPath] = useState("");
  const [target, setTarget] = useState(presetWorkspaceId ?? "");
  const text = copy[mode];

  const action = useMutation({
    mutationFn: () =>
      mutate(
        `/api/v1/project-actions/${mode}`,
        mode === "relocate" ? { workspace_id: target, destination: path } : { path },
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
      void queryClient.invalidateQueries({ queryKey: ["overview"] });
      onClose();
    },
  });

  const run = () => {
    if (
      ["relocate", "adopt-copy"].includes(mode) &&
      !window.confirm(
        `${humanize(mode)} using ${path}?\n\nDraft will verify project identity, acquire a fenced registry lease, and record an audited receipt.`,
      )
    )
      return;
    action.mutate();
  };

  const invalid = !path.trim() || (mode === "relocate" && !target);

  return (
    <Modal
      title={text.title}
      onClose={onClose}
      footer={
        <>
          <button className="button" onClick={onClose}>
            Cancel
          </button>
          <button className="button primary" disabled={invalid || action.isPending} onClick={run}>
            {action.isPending ? "Working…" : text.submit}
          </button>
        </>
      }
    >
      <p className="muted">{text.description}</p>
      {mode === "relocate" && (
        <label className="field">
          <span>Project to relocate</span>
          <select
            className="select"
            aria-label="Project to relocate"
            value={target}
            onChange={(event) => setTarget(event.target.value)}
          >
            <option value="">Choose project…</option>
            {projects.map((project) => (
              <option key={project.workspace_id} value={project.workspace_id}>
                {project.name}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="field">
        <span>{text.pathLabel}</span>
        <input
          className="input"
          aria-label={text.pathLabel}
          placeholder="/absolute/path/to/project"
          value={path}
          onChange={(event) => setPath(event.target.value)}
        />
      </label>
      <InlineError error={action.error} />
    </Modal>
  );
}
