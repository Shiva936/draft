import React from "react";
import { Link } from "react-router-dom";
import { Icon, type IconName } from "../icons";

export function Panel({
  children,
  className = "",
  ...rest
}: React.PropsWithChildren<{ className?: string } & React.HTMLAttributes<HTMLElement>>) {
  return (
    <section className={`panel ${className}`.trim()} {...rest}>
      {children}
    </section>
  );
}

export function PanelHeader({
  title,
  icon,
  count,
  action,
  subtitle,
  plain = false,
}: {
  title: string;
  icon?: IconName;
  count?: React.ReactNode;
  action?: React.ReactNode;
  subtitle?: string;
  plain?: boolean;
}) {
  return (
    <header className={plain ? "panel-header plain" : "panel-header"}>
      <div className="title-row">
        {icon && <Icon name={icon} size={18} />}
        <div>
          <h2>{title}</h2>
          {subtitle && <p className="muted">{subtitle}</p>}
        </div>
        {count !== undefined && count !== null && <span className="nav-count">{count}</span>}
      </div>
      {action}
    </header>
  );
}

export function PanelLink({ to, children }: { to: string; children: React.ReactNode }) {
  return (
    <Link className="panel-link" to={to}>
      {children}
      <Icon name="chevron-right" size={14} />
    </Link>
  );
}

export function PageHeader({
  title,
  icon,
  subtitle,
  status,
  breadcrumbs,
  actions,
  star,
}: {
  title: string;
  icon?: IconName;
  subtitle?: string;
  status?: React.ReactNode;
  breadcrumbs?: React.ReactNode;
  actions?: React.ReactNode;
  star?: { on: boolean; toggle: () => void };
}) {
  return (
    <div className="stack tight">
      {breadcrumbs && <nav className="breadcrumbs" aria-label="Breadcrumb">{breadcrumbs}</nav>}
      <div className="page-header">
        <div>
          <div className="title-row">
            {icon && <Icon name={icon} size={22} />}
            <h1>{title}</h1>
            {status}
            {star && (
              <button
                className={star.on ? "icon-button star-button on" : "icon-button star-button"}
                onClick={star.toggle}
                aria-pressed={star.on}
                aria-label={star.on ? `Unstar ${title}` : `Star ${title}`}
              >
                <Icon name="star" size={18} />
              </button>
            )}
          </div>
          {subtitle && <p>{subtitle}</p>}
        </div>
        {actions && <div className="page-actions">{actions}</div>}
      </div>
    </div>
  );
}

export function Toolbar({ children }: React.PropsWithChildren) {
  return <div className="toolbar">{children}</div>;
}

export function SearchField({
  value,
  onChange,
  placeholder,
  label,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label: string;
}) {
  return (
    <div className="search-field">
      <Icon name="search" size={16} />
      <input
        className="input"
        type="search"
        aria-label={label}
        placeholder={placeholder}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

export function FilterSelect({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <select className="select" aria-label={label} value={value} onChange={(event) => onChange(event.target.value)}>
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

export function Tabs({
  tabs,
  active,
  onSelect,
  label,
}: {
  tabs: { id: string; label: string; count?: number }[];
  active: string;
  onSelect: (id: string) => void;
  label: string;
}) {
  return (
    <div className="tabs" role="tablist" aria-label={label}>
      {tabs.map((tab) => (
        <button
          key={tab.id}
          role="tab"
          type="button"
          aria-selected={active === tab.id}
          className={active === tab.id ? "active" : ""}
          onClick={() => onSelect(tab.id)}
        >
          {tab.label}
          {tab.count !== undefined && <span className="tab-count">{tab.count}</span>}
        </button>
      ))}
    </div>
  );
}

export function Segmented({
  options,
  value,
  onChange,
  label,
}: {
  options: { value: string; label: string }[];
  value: string;
  onChange: (value: string) => void;
  label: string;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          className={value === option.value ? "active" : ""}
          aria-pressed={value === option.value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Definitions({ children, rows = false }: React.PropsWithChildren<{ rows?: boolean }>) {
  return <dl className={rows ? "definitions rows" : "definitions"}>{children}</dl>;
}

export function Pagination({
  page,
  pageCount,
  onChange,
}: {
  page: number;
  pageCount: number;
  onChange: (page: number) => void;
}) {
  if (pageCount <= 1) return null;
  const pages = Array.from({ length: pageCount }, (_, index) => index + 1).filter(
    (candidate) => candidate === 1 || candidate === pageCount || Math.abs(candidate - page) <= 1,
  );
  return (
    <nav className="pagination" aria-label="Pagination">
      <button onClick={() => onChange(page - 1)} disabled={page <= 1} aria-label="Previous page">
        <Icon name="chevron-left" size={14} />
      </button>
      {pages.map((candidate, index) => (
        <React.Fragment key={candidate}>
          {index > 0 && candidate - pages[index - 1] > 1 && <span className="muted">…</span>}
          <button
            className={candidate === page ? "active" : ""}
            aria-current={candidate === page ? "page" : undefined}
            onClick={() => onChange(candidate)}
          >
            {candidate}
          </button>
        </React.Fragment>
      ))}
      <button onClick={() => onChange(page + 1)} disabled={page >= pageCount} aria-label="Next page">
        <Icon name="chevron-right" size={14} />
      </button>
    </nav>
  );
}
