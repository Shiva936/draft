import React from "react";
import { Icon, type IconName } from "../icons";
import { ConsoleApiError } from "../api";

export function EmptyState({
  label,
  detail,
  icon = "package",
  action,
  inline = false,
}: {
  label: string;
  detail?: string;
  icon?: IconName;
  action?: React.ReactNode;
  inline?: boolean;
}) {
  return (
    <div className={inline ? "empty-state inline" : "empty-state"}>
      <Icon name={icon} size={24} />
      <strong>{label}</strong>
      {detail && <p>{detail}</p>}
      {action}
    </div>
  );
}

export function ErrorState({
  error,
  retry,
  inline = false,
}: {
  error: unknown;
  retry?: () => void;
  inline?: boolean;
}) {
  const apiError = error as ConsoleApiError;
  const offline = apiError?.code === "DAEMON_UNAVAILABLE";
  return (
    <div className={inline ? "error-state inline" : "error-state"} role="alert">
      <Icon name={offline ? "zap" : "alert-triangle"} size={24} />
      <strong>{offline ? "Draft daemon is offline" : "This view could not be loaded"}</strong>
      <p>{apiError?.message ?? String(error)}</p>
      {offline && <p className="muted">Canonical state remains on disk. Restart with `draft service restart`.</p>}
      {retry && (
        <button className="button" onClick={retry}>
          <Icon name="refresh" size={14} />
          Retry
        </button>
      )}
    </div>
  );
}

export function InlineError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <p className="inline-error" role="alert">
      <Icon name="alert-triangle" size={14} />
      <span>{(error as Error).message}</span>
    </p>
  );
}

export function Skeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div className="skeleton" aria-hidden="true">
      {Array.from({ length: rows }, (_, index) => (
        <span key={index} />
      ))}
    </div>
  );
}

export function FullLoading({ label }: { label: string }) {
  return (
    <div className="full-state" role="status">
      <div className="spinner" />
      <p>{label}</p>
    </div>
  );
}

type QueryLike<T> = {
  isLoading: boolean;
  error: unknown;
  data: T | undefined;
  refetch: () => unknown;
};

/**
 * Renders the loading, error, empty and ready states of a query in the one
 * place, so every screen degrades identically.
 */
export function QueryState<T>({
  query,
  empty,
  skeletonRows,
  children,
}: {
  query: QueryLike<T>;
  empty: React.ReactNode;
  skeletonRows?: number;
  children: (value: T) => React.ReactNode;
}) {
  if (query.isLoading) return <Skeleton rows={skeletonRows} />;
  if (query.error) return <ErrorState error={query.error} retry={() => query.refetch()} inline />;
  if (query.data == null || (Array.isArray(query.data) && query.data.length === 0)) return <>{empty}</>;
  return <>{children(query.data)}</>;
}
