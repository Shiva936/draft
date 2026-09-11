import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import type { SearchResult } from "../contracts";
import { Icon, type IconName } from "../icons";
import { EmptyState, Skeleton } from "../components/states";
import { humanize } from "../lib/format";
import { systemLinks } from "./Shell";

const resultIcons: Record<string, IconName> = {
  project: "package",
  task: "list-checks",
  change: "layers",
  file: "file",
  action: "zap",
  setting: "settings",
  event: "activity",
};

export function CommandPalette({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();
  const results = useQuery({
    queryKey: ["search", query],
    enabled: query.trim().length > 1,
    queryFn: () => api<{ results: SearchResult[] }>(`/api/v1/search?q=${encodeURIComponent(query)}`),
  });

  useEffect(() => input.current?.focus(), []);

  const go = (result: SearchResult) => {
    if (result.kind === "setting") navigate("/settings");
    else if (result.workspace_id) {
      const base = `/projects/${encodeURIComponent(result.workspace_id)}`;
      if (result.kind === "project") navigate(base);
      else if (result.kind === "task") navigate(`${base}/tasks`);
      else if (result.kind === "change" || result.kind === "action")
        navigate(`${base}/graph`);
      else if (result.kind === "file") navigate(`${base}/editor`);
      else navigate(`${base}/events`);
    }
    onClose();
  };

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="command-dialog" role="dialog" aria-modal="true" aria-label="Search and commands">
        <div className="command-input">
          <Icon name="search" size={18} />
          <input
            ref={input}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search canonical Draft state…"
            aria-label="Search canonical Draft state"
          />
          <kbd>Esc</kbd>
        </div>
        <div className="command-results">
          {query.trim().length < 2 ? (
            <>
              <div className="eyebrow">Navigation</div>
              {systemLinks.map((link) => (
                <button
                  key={link.to}
                  onClick={() => {
                    navigate(link.to);
                    onClose();
                  }}
                >
                  <Icon name={link.icon} size={16} />
                  <div>
                    <strong>{link.label}</strong>
                  </div>
                  <Icon name="corner-down-left" size={14} />
                </button>
              ))}
            </>
          ) : results.isLoading ? (
            <Skeleton rows={4} />
          ) : (results.data?.results ?? []).length > 0 ? (
            results.data!.results.map((result, index) => (
              <button key={`${result.kind}-${result.id ?? result.workspace_id}-${index}`} onClick={() => go(result)}>
                <Icon name={resultIcons[result.kind] ?? "dot"} size={16} />
                <div>
                  <strong>{result.title}</strong>
                  <small>
                    {humanize(result.kind)}
                    {result.subtitle ? ` · ${result.subtitle}` : ""}
                  </small>
                </div>
                <Icon name="corner-down-left" size={14} />
              </button>
            ))
          ) : (
            <EmptyState inline icon="search" label="No matching Draft state." />
          )}
        </div>
      </div>
    </div>
  );
}
