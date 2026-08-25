import React from "react";
import { Icon } from "../icons";

/**
 * The persistent right-hand detail pane every master/detail screen uses. On
 * narrow viewports the stylesheet drops it below the list rather than hiding it.
 */
export function DetailDrawer({
  title,
  eyebrow,
  subtitle,
  badges,
  onClose,
  headerActions,
  footer,
  children,
}: React.PropsWithChildren<{
  title: React.ReactNode;
  eyebrow?: React.ReactNode;
  subtitle?: React.ReactNode;
  badges?: React.ReactNode;
  onClose: () => void;
  headerActions?: React.ReactNode;
  footer?: React.ReactNode;
}>) {
  return (
    <aside className="detail-panel" aria-label="Details">
      <div className="detail-header">
        <div>
          {eyebrow && <div className="eyebrow">{eyebrow}</div>}
          <div className="detail-title">
            <h2 className="truncate">{title}</h2>
            {badges}
          </div>
          {subtitle && <div className="detail-subtitle">{subtitle}</div>}
        </div>
        {headerActions}
        <button className="icon-button" onClick={onClose} aria-label="Close details">
          <Icon name="x" size={18} />
        </button>
      </div>
      <div className="detail-scroll">{children}</div>
      {footer && <div className="detail-footer">{footer}</div>}
    </aside>
  );
}
