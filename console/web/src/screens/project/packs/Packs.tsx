import { useState } from "react";
import { Navigate, Outlet, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import type { PackSummary } from "../../../contracts";
import { Icon } from "../../../icons";
import { Panel, PanelHeader, SearchField } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, QueryState } from "../../../components/states";
import { packRevisionLabel } from "../../../lib/format";
import { CreatePackModal } from "./CreatePackModal";

/**
 * Pack list and the selected pack's workbench. Lifecycle actions live inside
 * the pack context, never in this list.
 */
export function Packs() {
  const { workspaceId = "", packId } = useParams();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const [search, setSearch] = useState("");
  const [creating, setCreating] = useState(params.get("create") === "1");

  const query = useQuery({
    queryKey: ["packs", workspaceId],
    queryFn: () => api<PackSummary[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/packs`),
  });

  const packs = query.data ?? [];
  const filtered = packs.filter((pack) =>
    `${pack.pack_id} ${pack.name} ${pack.intent}`.toLowerCase().includes(search.trim().toLowerCase()),
  );

  const closeCreate = () => {
    setCreating(false);
    if (params.has("create")) {
      params.delete("create");
      setParams(params, { replace: true });
    }
  };

  // With no pack selected, open the first one so the workbench is never empty.
  if (!packId && packs.length > 0) {
    return <Navigate to={`${encodeURIComponent(packs[0].pack_id)}/summary`} replace />;
  }

  return (
    <>
      <div className={packId ? "pack-workbench" : "pack-workbench no-selection"}>
        <Panel className="flush">
          <PanelHeader
            title="Packs"
            icon="layers"
            count={packs.length}
            action={
              <div className="button-row">
                <button className="button small" onClick={() => setCreating(true)}>
                  <Icon name="plus" size={14} />
                  New pack
                </button>
                <button
                  className="icon-button"
                  onClick={() => void query.refetch()}
                  disabled={query.isFetching}
                  aria-label="Refresh packs"
                >
                  <Icon name="refresh" size={16} />
                </button>
              </div>
            }
          />
          <div className="panel-body" style={{ paddingBottom: 0 }}>
            <SearchField label="Search packs" placeholder="Search packs…" value={search} onChange={setSearch} />
          </div>

          <QueryState
            query={query}
            skeletonRows={5}
            empty={
              <EmptyState
                icon="layers"
                label="No packs exist in this project."
                detail="A pack groups reviewable changes with their evidence and receipts."
                action={
                  <button className="button primary" onClick={() => setCreating(true)}>
                    <Icon name="plus" size={16} />
                    Create pack
                  </button>
                }
              />
            }
          >
            {() =>
              filtered.length === 0 ? (
                <EmptyState inline icon="search" label="No pack matches this search." />
              ) : (
                <div className="rows">
                  {filtered.map((pack) => (
                    <button
                      key={pack.pack_id}
                      className={pack.pack_id === packId ? "pack-row selected" : "pack-row"}
                      onClick={() => navigate(`${encodeURIComponent(pack.pack_id)}/summary`)}
                    >
                      <span className="title-row">
                        <strong>{pack.pack_id}</strong>
                        {packRevisionLabel(pack.revision) && (
                          <span className="chip mono">{packRevisionLabel(pack.revision)}</span>
                        )}
                      </span>
                      <span className="title-row">
                        <small style={{ flex: 1 }}>{pack.name}</small>
                        <StatusBadge value={pack.submit_state} />
                      </span>
                    </button>
                  ))}
                </div>
              )
            }
          </QueryState>
        </Panel>

        {packId ? (
          <Outlet />
        ) : (
          <Panel>
            <EmptyState icon="layers" label="Select a pack." detail="Pack evidence and lifecycle actions open here." />
          </Panel>
        )}
      </div>

      {creating && <CreatePackModal workspaceId={workspaceId} packs={packs} onClose={closeCreate} />}
    </>
  );
}
