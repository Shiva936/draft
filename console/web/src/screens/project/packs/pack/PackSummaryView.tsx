import { useMemo } from "react";
import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../../api";
import { Icon } from "../../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../../components/layout";
import { StatusBadge } from "../../../../components/StatusBadge";
import { EvidenceTile } from "../../../../components/evidence";
import { CandidateAvatar } from "../../../../components/CandidateAvatar";
import { EmptyState } from "../../../../components/states";
import { DiffBar } from "../../../../components/charts";
import { NONE, formatDate, humanize, shortDigest } from "../../../../lib/format";
import { parseDiff } from "./diff";

/**
 * The designed pack surface: evidence headline tiles, the per-directory change
 * overview, and the readiness checklist. Every figure comes from the pack
 * inspect report, its readiness report, or the canonical diff.
 */
export function PackSummaryView({ pack }: { pack: any }) {
  const { workspaceId = "", packId = "" } = useParams();
  const manifest = pack.manifest ?? {};

  const readiness = useQuery({
    queryKey: ["pack-view", workspaceId, packId, "verify"],
    queryFn: () => api<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/packs/${encodeURIComponent(packId)}/verify`),
  });
  const diff = useQuery({
    queryKey: ["pack-view", workspaceId, packId, "diff"],
    queryFn: () => api<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/packs/${encodeURIComponent(packId)}/diff`),
  });

  const change = useMemo(() => parseDiff(diff.data), [diff.data]);
  const blockers: string[] = readiness.data?.blockers ?? [];
  const ready = readiness.data?.ok === true;
  const receipts: string[] = pack.receipts ?? [];

  return (
    <>
      <div className="grid-4">
        <EvidenceTile
          label="Changes"
          icon="file-text"
          value={change.files.length || NONE}
          detail={
            change.files.length > 0 ? (
              <>
                <span className="added-count">+{change.added}</span> <span className="removed-count">−{change.removed}</span>
              </>
            ) : (
              "No file changes recorded"
            )
          }
        />
        <EvidenceTile
          label="Verification"
          icon="shield-check"
          value={pack.verified ? "Passed" : "Required"}
          tone={pack.verified ? "success" : "warning"}
          detail={pack.verified ? "Verification receipt recorded" : "Run verify to produce evidence"}
        />
        <EvidenceTile
          label="Review"
          icon="eye"
          value={humanize(pack.lifecycle ?? manifest.submit_state ?? "draft")}
          tone="review"
          detail="Canonical lifecycle state"
        />
        <EvidenceTile
          label="Submission"
          icon="upload"
          value={readiness.isLoading ? "…" : ready ? "Ready" : "Blocked"}
          tone={ready ? "success" : "warning"}
          detail={
            ready
              ? "All readiness checks complete"
              : `${blockers.length} readiness ${blockers.length === 1 ? "blocker" : "blockers"}`
          }
        />
      </div>

      <div className="grid-2">
        <Panel className="flush">
          <PanelHeader
            title="Change overview"
            icon="file-text"
            action={
              change.files.length > 0 ? (
                <span className="result-count">
                  <span className="added-count">+{change.added}</span>{" "}
                  <span className="removed-count">−{change.removed}</span> · {change.files.length} files
                </span>
              ) : undefined
            }
          />
          {change.directories.length === 0 ? (
            <EmptyState
              inline
              icon="file-text"
              label="No change overview."
              detail="The canonical diff for this pack is empty."
            />
          ) : (
            <div>
              {change.directories.map((directory) => (
                <div className="change-row" key={directory.path}>
                  <span className="path">
                    <Icon name="folder" size={14} />
                    {directory.path}
                  </span>
                  <span className="result-count">
                    {directory.files} {directory.files === 1 ? "file" : "files"}
                  </span>
                  <span className="counts">
                    <DiffBar added={directory.added} removed={directory.removed} />
                    <span className="added-count">+{directory.added}</span>
                    <span className="removed-count">−{directory.removed}</span>
                  </span>
                </div>
              ))}
            </div>
          )}
        </Panel>

        <Panel className="flush">
          <PanelHeader
            title="Readiness"
            icon="list-checks"
            action={readiness.data ? <StatusBadge value={ready ? "ready" : "blocked"} /> : undefined}
          />
          {readiness.isLoading ? (
            <EmptyState inline icon="clock" label="Computing readiness…" />
          ) : blockers.length === 0 ? (
            <EmptyState
              inline
              icon="check-circle"
              label={ready ? "All readiness checks passed." : "No blockers recorded."}
            />
          ) : (
            <ul className="checklist">
              {blockers.map((blocker) => (
                <li key={blocker}>
                  <Icon name="alert-triangle" size={16} className="warning" />
                  <span>{humanize(blocker)}</span>
                  <StatusBadge value="blocked" plain />
                </li>
              ))}
            </ul>
          )}
          {readiness.data && (
            <div className="panel-body">
              <Definitions rows>
                <dt>Verification receipt</dt>
                <dd className="mono">{readiness.data.verification_receipt_id ?? NONE}</dd>
                <dt>Review receipt</dt>
                <dd className="mono">{readiness.data.review_receipt_id ?? NONE}</dd>
                <dt>Approval</dt>
                <dd className="mono">{readiness.data.approval_ref ?? NONE}</dd>
              </Definitions>
            </div>
          )}
        </Panel>
      </div>

      <div className="grid-2">
        <Panel className="padded stack tight">
          <PanelHeader title="Pack" icon="layers" plain />
          <Definitions rows>
            <dt>Name</dt>
            <dd>{manifest.name ?? NONE}</dd>
            <dt>Intent</dt>
            <dd>{manifest.intent ? humanize(String(manifest.intent)) : NONE}</dd>
            <dt>Description</dt>
            <dd>{manifest.description || NONE}</dd>
            <dt>Author</dt>
            <dd>
              {manifest.author_id ? (
                <span className="avatar-label">
                  <CandidateAvatar name={manifest.author_id} />
                  <span>{manifest.author_id}</span>
                </span>
              ) : (
                NONE
              )}
            </dd>
            <dt>Created</dt>
            <dd>{formatDate(manifest.created_at)}</dd>
            <dt>Revision</dt>
            <dd className="mono">{pack.revision_id ? shortDigest(pack.revision_id) : NONE}</dd>
            <dt>Manifest digest</dt>
            <dd className="mono">{shortDigest(manifest.manifest_digest)}</dd>
            <dt>Import state</dt>
            <dd>{pack.quarantine ? humanize(pack.quarantine.trust_evaluation) : "Local"}</dd>
          </Definitions>
        </Panel>

        <div className="stack">
          <Panel className="padded stack tight">
            <PanelHeader title="Impact" icon="activity" plain />
            <Definitions rows>
              <dt>Symbols touched</dt>
              <dd>{(pack.symbols_touched ?? []).length || NONE}</dd>
              <dt>Public API changes</dt>
              <dd>{(pack.public_api_changed ?? []).join(", ") || "None recorded"}</dd>
              <dt>Declared dependencies</dt>
              <dd>{(manifest.declared_dependencies ?? []).join(", ") || "None"}</dd>
            </Definitions>
          </Panel>

          <Panel className="flush">
            <PanelHeader title="Receipts" icon="file-text" count={receipts.length} />
            {receipts.length === 0 ? (
              <EmptyState inline icon="file-text" label="No receipts recorded yet." />
            ) : (
              <div className="rows">
                {receipts.map((receipt) => (
                  <div className="row-item" key={receipt}>
                    <Icon name="file-text" size={16} />
                    <div className="row-main">
                      <strong className="mono">{receipt}</strong>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Panel>
        </div>
      </div>
    </>
  );
}
