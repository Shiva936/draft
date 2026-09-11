import React from "react";
import { Icon } from "../icons";
import { StatusBadge } from "./StatusBadge";
import { humanize, statusTone, type Tone } from "../lib/format";

/** One headline evidence figure — changes, verifications, checks, submissions. */
export function EvidenceTile({
  label,
  icon,
  value,
  tone,
  detail,
  link,
}: {
  label: string;
  icon: Parameters<typeof Icon>[0]["name"];
  value: React.ReactNode;
  tone?: Tone;
  detail?: React.ReactNode;
  link?: React.ReactNode;
}) {
  return (
    <div className="evidence-tile">
      <header>
        <Icon name={icon} size={16} />
        {label}
      </header>
      <strong className={tone}>{value}</strong>
      {detail && <small>{detail}</small>}
      {link}
    </div>
  );
}

export type ChecklistEntry = { id: string; label: string; status: string; detail?: string };

/** Pass/fail evidence list shared by change verification and inbox evidence. */
export function EvidenceChecklist({ entries }: { entries: ChecklistEntry[] }) {
  return (
    <ul className="checklist">
      {entries.map((entry) => (
        <li key={entry.id}>
          <Icon
            name={statusTone(entry.status) === "success" ? "check-circle" : statusTone(entry.status) === "danger" ? "x-circle" : "alert-triangle"}
            size={16}
            className={statusTone(entry.status)}
          />
          <span className="truncate">{entry.label}</span>
          {entry.detail && <span className="muted">{entry.detail}</span>}
          <StatusBadge value={entry.status} plain />
        </li>
      ))}
    </ul>
  );
}

/** Receipt identity and outcome, shown wherever a receipt is referenced. */
export function ReceiptSummary({
  receiptId,
  outcome,
  recordedAt,
}: {
  receiptId: string;
  outcome?: string | null;
  recordedAt?: string | null;
}) {
  return (
    <div className="row-item">
      <Icon name="file-text" size={16} />
      <div className="row-main">
        <strong className="mono">{receiptId}</strong>
        {recordedAt && <small>{recordedAt}</small>}
      </div>
      {outcome && <StatusBadge value={outcome} />}
    </div>
  );
}

/** Blocking-condition notice used by review, submit and editor conflicts. */
export function ConflictNotice({ title, detail }: { title: string; detail?: string }) {
  return (
    <div className="banner warning" role="status">
      <Icon name="alert-triangle" size={18} />
      <div>
        <strong>{humanize(title)}</strong>
        {detail && <p>{detail}</p>}
      </div>
    </div>
  );
}
