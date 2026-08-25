import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { mutate } from "../../../api";
import type { PackSummary } from "../../../contracts";
import { Modal } from "../../../components/Modal";
import { InlineError } from "../../../components/states";

/** Creates a pack from the working tree, or as the successor of another pack. */
export function CreatePackModal({
  workspaceId,
  packs,
  onClose,
}: {
  workspaceId: string;
  packs: PackSummary[];
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [basePack, setBasePack] = useState("");

  const create = useMutation({
    mutationFn: () =>
      mutate<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/pack-create`, {
        name,
        ...(basePack ? { base_pack: basePack } : {}),
      }),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: ["packs", workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ["project", workspaceId] });
      onClose();
      const created = result?.pack_id ?? result?.manifest?.pack_id;
      if (created) navigate(`${encodeURIComponent(created)}/summary`);
    },
  });

  return (
    <Modal
      title="Create pack"
      onClose={onClose}
      footer={
        <>
          <button className="button" onClick={onClose}>
            Cancel
          </button>
          <button className="button primary" disabled={!name.trim() || create.isPending} onClick={() => create.mutate()}>
            {create.isPending ? "Creating…" : "Create pack"}
          </button>
        </>
      }
    >
      <label className="field">
        <span>Pack name</span>
        <input
          className="input"
          aria-label="New pack name"
          placeholder="Pack name"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label className="field">
        <span>Base</span>
        <select
          className="select"
          aria-label="Optional predecessor pack"
          value={basePack}
          onChange={(event) => setBasePack(event.target.value)}
        >
          <option value="">Working tree base</option>
          {packs.map((pack) => (
            <option key={pack.pack_id} value={pack.pack_id}>
              Successor of {pack.name}
            </option>
          ))}
        </select>
      </label>
      <p className="muted">
        Submitted packs are immutable. To continue from one, create a successor pack from its base instead of reopening
        it.
      </p>
      <InlineError error={create.error} />
    </Modal>
  );
}
