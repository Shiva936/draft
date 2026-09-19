import React from "react";
import { Icon } from "../icons";

/**
 * Draft's next safe action. Rendered only when core actually recommends one;
 * there is no invented guidance here.
 */
export function NextAction({
  label,
  detail,
  action,
}: {
  label: string;
  detail?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="next-action">
      <Icon name="shield-check" size={20} />
      <div>
        <strong>{label}</strong>
        {detail && <p>{detail}</p>}
      </div>
      {action}
    </div>
  );
}

/** A row in a "recommended actions" list that navigates or runs on click. */
export function SuggestionRow({
  icon,
  title,
  detail,
  onSelect,
  trailing,
}: {
  icon: React.ReactNode;
  title: string;
  detail?: string;
  onSelect?: () => void;
  trailing?: React.ReactNode;
}) {
  return (
    <button className="suggestion-row" type="button" onClick={onSelect} disabled={!onSelect}>
      {icon}
      <div>
        <strong>{title}</strong>
        {detail && <small>{detail}</small>}
      </div>
      {trailing ?? <Icon name="chevron-right" size={16} />}
    </button>
  );
}
