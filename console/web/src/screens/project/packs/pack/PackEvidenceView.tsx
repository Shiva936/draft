import { useQuery } from "@tanstack/react-query";
import { api } from "../../../../api";
import { Icon } from "../../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../../components/layout";
import { RiskBadge, StatusBadge } from "../../../../components/StatusBadge";
import { EvidenceTile, ReceiptSummary } from "../../../../components/evidence";
import { DataView } from "../../../../components/DataView";
import { DiffBar } from "../../../../components/charts";
import { EmptyState, QueryState } from "../../../../components/states";
import { NONE, formatDateTime, humanize } from "../../../../lib/format";
import { parseDiff } from "./diff";

/**
 * The pack evidence tabs.
 *
 * Diff, risk, readiness (verify / review / approvals / submit) and receipts all
 * have a canonical shape, so each gets a designed surface. Anything else falls
 * back to the readable canonical payload rather than an invented layout.
 */
export function PackEvidenceView({
  workspaceId,
  packId,
  view,
}: {
  workspaceId: string;
  packId: string;
  view: string;
}) {
  const query = useQuery({
    queryKey: ["pack-view", workspaceId, packId, view],
    queryFn: () =>
      api<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/packs/${encodeURIComponent(packId)}/${view}`),
  });

  return (
    <QueryState
      query={query}
      skeletonRows={5}
      empty={<Panel><EmptyState icon="file-text" label={`No ${view} data is available.`} /></Panel>}
    >
      {(value: any) => {
        if (view === "diff") return <DiffView value={value} />;
        if (view === "risk") return <RiskView value={value} />;
        if (["verify", "review", "approvals", "submit"].includes(view)) return <ReadinessView view={view} value={value} />;
        if (["receipts", "rollback"].includes(view)) return <ReceiptsView view={view} value={value} />;
        return (
          <Panel className="flush">
            <PanelHeader title={humanize(view)} icon="file-text" />
            <DataView value={value} />
          </Panel>
        );
      }}
    </QueryState>
  );
}

function DiffView({ value }: { value: unknown }) {
  const summary = parseDiff(value);
  return (
    <>
      {summary.files.length > 0 && (
        <Panel className="flush">
          <PanelHeader
            title="Changed files"
            icon="file-text"
            count={summary.files.length}
            action={
              <span className="result-count">
                <span className="added-count">+{summary.added}</span>{" "}
                <span className="removed-count">−{summary.removed}</span>
              </span>
            }
          />
          <div>
            {summary.files.map((file) => (
              <div className="change-row" key={file.path}>
                <span className="path">
                  <Icon name="file" size={14} />
                  {file.path}
                </span>
                <span className="counts">
                  <DiffBar added={file.added} removed={file.removed} />
                  <span className="added-count">+{file.added}</span>
                  <span className="removed-count">−{file.removed}</span>
                </span>
              </div>
            ))}
          </div>
        </Panel>
      )}
      <Panel className="flush">
        <PanelHeader title="Canonical diff" icon="file-text" />
        {typeof value === "string" && value.trim() === "" ? (
          <EmptyState inline icon="file-text" label="This pack records no diff." />
        ) : (
          <DataView value={value} />
        )}
      </Panel>
    </>
  );
}

function RiskView({ value }: { value: any }) {
  const factors: string[] = value.factors ?? [];
  const gaps: string[] = value.evidence_gaps ?? [];
  const hotspots: string[] = value.hotspots ?? [];

  return (
    <>
      <div className="grid-4">
        <EvidenceTile label="Risk level" icon="alert-triangle" value={<RiskBadge value={value.level} />} />
        <EvidenceTile label="Score" icon="activity" value={value.score ?? NONE} detail="Explainable risk score" />
        <EvidenceTile
          label="Files changed"
          icon="file-text"
          value={value.files_changed ?? NONE}
          detail={hotspots.length > 0 ? `${hotspots.length} hotspots` : "No hotspots"}
        />
        <EvidenceTile
          label="Policy"
          icon="shield"
          value={humanize(value.policy_decision ?? "unknown")}
          tone={String(value.policy_decision).includes("allow") ? "success" : "warning"}
        />
      </div>

      <div className="grid-2">
        <Panel className="flush">
          <PanelHeader title="Risk factors" icon="alert-triangle" count={factors.length} />
          {factors.length === 0 ? (
            <EmptyState inline icon="check-circle" label="No risk factors recorded." />
          ) : (
            <ul className="checklist">
              {factors.map((factor) => (
                <li key={factor}>
                  <Icon name="alert-triangle" size={16} className="warning" />
                  <span>{humanize(factor)}</span>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel className="flush">
          <PanelHeader title="Evidence gaps" icon="shield" count={gaps.length} />
          {gaps.length === 0 ? (
            <EmptyState inline icon="check-circle" label="No evidence gaps." />
          ) : (
            <ul className="checklist">
              {gaps.map((gap) => (
                <li key={gap}>
                  <Icon name="x-circle" size={16} className="danger" />
                  <span>{humanize(gap)}</span>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>

      {hotspots.length > 0 && (
        <Panel className="flush">
          <PanelHeader title="Hotspots" icon="zap" count={hotspots.length} />
          <div>
            {hotspots.map((hotspot) => (
              <div className="change-row" key={String(hotspot)}>
                <span className="path">
                  <Icon name="file" size={14} />
                  {String(hotspot)}
                </span>
              </div>
            ))}
          </div>
        </Panel>
      )}

      <Panel className="padded stack tight">
        <PanelHeader title="Assessment" icon="file-text" plain />
        <Definitions rows>
          <dt>Receipt</dt>
          <dd className="mono">{value.receipt_id ?? NONE}</dd>
          <dt>Reason codes</dt>
          <dd>{(value.reason_codes ?? []).join(", ") || NONE}</dd>
          <dt>Evidence summary</dt>
          <dd>{(value.evidence_summary ?? []).join(", ") || NONE}</dd>
        </Definitions>
      </Panel>
    </>
  );
}

function ReadinessView({ view, value }: { view: string; value: any }) {
  const blockers: string[] = value.blockers ?? [];
  const ready = value.ok === true;

  return (
    <>
      <div className="grid-3">
        <EvidenceTile
          label={humanize(view)}
          icon="shield-check"
          value={ready ? "Ready" : "Blocked"}
          tone={ready ? "success" : "warning"}
          detail={ready ? "All readiness checks satisfied" : `${blockers.length} blocking condition(s)`}
        />
        <EvidenceTile
          label="Verification receipt"
          icon="file-text"
          value={value.verification_receipt_id ? "Recorded" : "Missing"}
          tone={value.verification_receipt_id ? "success" : "warning"}
          detail={value.verification_receipt_id ?? "Run verify to produce one"}
        />
        <EvidenceTile
          label="Approval"
          icon="check-circle"
          value={value.approval_ref ? "Recorded" : "None"}
          tone={value.approval_ref ? "success" : "neutral"}
          detail={value.approval_ref ?? "No approval reference"}
        />
      </div>

      <Panel className="flush">
        <PanelHeader title="Blocking conditions" icon="alert-triangle" count={blockers.length} />
        {blockers.length === 0 ? (
          <EmptyState inline icon="check-circle" label="Nothing is blocking this pack." />
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
      </Panel>

      {(value.ownership || value.reviewability) && (
        <div className="grid-2">
          {value.ownership && (
            <Panel className="flush">
              <PanelHeader title="Ownership" icon="users" />
              <DataView value={value.ownership} />
            </Panel>
          )}
          {value.reviewability && (
            <Panel className="flush">
              <PanelHeader title="Reviewability" icon="eye" />
              <DataView value={value.reviewability} />
            </Panel>
          )}
        </div>
      )}
    </>
  );
}

function ReceiptsView({ view, value }: { view: string; value: any }) {
  const receipts: any[] = Array.isArray(value) ? value : (value?.receipts ?? []);

  if (receipts.length === 0) {
    return (
      <Panel>
        <EmptyState
          icon="file-text"
          label={view === "rollback" ? "No rollback receipts recorded." : "No receipts recorded."}
          detail="Receipts appear here as Draft finalizes verification, review, approval, and submission."
        />
      </Panel>
    );
  }

  return (
    <Panel className="flush">
      <PanelHeader title={humanize(view)} icon="file-text" count={receipts.length} />
      <div className="rows">
        {receipts.map((receipt, index) => {
          if (typeof receipt === "string") {
            return <ReceiptSummary key={receipt} receiptId={receipt} />;
          }
          return (
            <ReceiptSummary
              key={receipt.receipt_id ?? index}
              receiptId={receipt.receipt_id ?? String(index)}
              outcome={receipt.outcome ?? receipt.kind}
              recordedAt={receipt.created_at ? formatDateTime(receipt.created_at) : null}
            />
          );
        })}
      </div>
    </Panel>
  );
}
