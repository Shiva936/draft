import React, { useState } from "react";
import { Icon, type IconName } from "../icons";
import { useDismiss } from "../lib/hooks";

export type MenuItem =
  | { kind: "separator" }
  | { kind: "title"; label: string }
  | {
      kind?: "item";
      label: string;
      icon?: IconName;
      shortcut?: string;
      danger?: boolean;
      disabled?: boolean;
      /** Shown as the control's title when the item is unavailable. */
      reason?: string;
      onSelect: () => void;
    };

/** A popover menu anchored to a trigger, dismissed on Escape or outside click. */
export function Menu({
  trigger,
  items,
  align = "right",
  label,
}: {
  trigger: (props: { open: boolean; toggle: () => void }) => React.ReactNode;
  items: MenuItem[];
  align?: "left" | "right";
  label: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useDismiss<HTMLDivElement>(open, () => setOpen(false));
  return (
    <div className="menu-anchor" ref={ref}>
      {trigger({ open, toggle: () => setOpen((value) => !value) })}
      {open && (
        <div className={`menu ${align}`} role="menu" aria-label={label}>
          {items.map((item, index) => {
            if (item.kind === "separator") return <div className="menu-separator" key={index} />;
            if (item.kind === "title")
              return (
                <div className="menu-title eyebrow" key={index}>
                  {item.label}
                </div>
              );
            return (
              <button
                key={index}
                role="menuitem"
                type="button"
                className={item.danger ? "danger" : undefined}
                disabled={item.disabled}
                title={item.disabled ? item.reason : undefined}
                onClick={() => {
                  setOpen(false);
                  item.onSelect();
                }}
              >
                {item.icon && <Icon name={item.icon} size={16} />}
                <span>{item.label}</span>
                {item.shortcut && <kbd>{item.shortcut}</kbd>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** The `⋮` overflow trigger used on table rows and card headers. */
export function OverflowMenu({ items, label }: { items: MenuItem[]; label: string }) {
  return (
    <Menu
      label={label}
      items={items}
      trigger={({ open, toggle }) => (
        <button
          className={open ? "icon-button active" : "icon-button"}
          onClick={toggle}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={label}
        >
          <Icon name="more-vertical" size={16} />
        </button>
      )}
    />
  );
}
