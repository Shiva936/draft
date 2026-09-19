import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../api";
import { Icon } from "../../icons";
import { Panel, PanelHeader } from "../../components/layout";
import { DataView } from "../../components/DataView";
import { EmptyState, InlineError, QueryState } from "../../components/states";
import type { ProjectSection } from "./Settings";

/**
 * Canonical project configuration: resolved config, ignore policy, hooks, and
 * stable candidates. Every mutation goes through the project action contract.
 */
export function ProjectSettingsPanels({
  workspaceId,
  section,
}: {
  workspaceId: string;
  section: ProjectSection;
}) {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: ["project-settings", workspaceId],
    enabled: Boolean(workspaceId),
    queryFn: () => api<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/settings`),
  });

  const action = useMutation({
    mutationFn: ({ name, body }: { name: string; body: unknown }) =>
      mutate(`/api/v1/projects/${encodeURIComponent(workspaceId)}/actions/${name}`, body),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["project-settings", workspaceId] }),
  });

  const busy = action.isPending;

  return (
    <>
      {section === "configuration" && (
        <PairPanel
          title="Configuration"
          icon="settings"
          subtitle="Canonical key/value configuration for this project."
          keyLabel="Key"
          valueLabel="Value"
          busy={busy}
          onPrimary={(key, value) => action.mutate({ name: "config-set", body: { key, value } })}
          primaryLabel="Set"
          onSecondary={(key) => action.mutate({ name: "config-unset", body: { key } })}
          secondaryLabel="Unset"
        />
      )}

      {section === "ignore" && (
        <PairPanel
          title="Ignore policy"
          icon="eye"
          subtitle="Patterns excluded from the canonical source view."
          keyLabel="Pattern"
          busy={busy}
          onPrimary={(pattern) => action.mutate({ name: "ignore-add", body: { pattern } })}
          primaryLabel="Add"
          onSecondary={(pattern) => action.mutate({ name: "ignore-remove", body: { pattern } })}
          secondaryLabel="Remove"
        />
      )}

      {section === "hooks" && (
        <PairPanel
          title="Hooks"
          icon="terminal"
          subtitle="Command templates run at canonical lifecycle points."
          keyLabel="Hook key"
          valueLabel="Command template"
          busy={busy}
          onPrimary={(key, value) => action.mutate({ name: "hook-set", body: { key, value } })}
          primaryLabel="Set"
          onSecondary={(key) => action.mutate({ name: "hook-unset", body: { key } })}
          secondaryLabel="Unset"
          onTertiary={(key) => action.mutate({ name: "hook-run", body: { hook_name: key } })}
          tertiaryLabel="Run"
        />
      )}

      {section === "candidates" && (
        <PairPanel
          title="Candidates"
          icon="users"
          subtitle="Stable named commands that produce candidate executions."
          keyLabel="Candidate name"
          valueLabel="Command and arguments"
          busy={busy}
          onPrimary={(name, command) =>
            action.mutate({
              name: "candidate-add",
              body: { name, kind: "command", command: (command ?? "").trim().split(/\s+/) },
            })
          }
          primaryLabel="Add"
          onSecondary={(name) => {
            if (window.confirm(`Remove candidate ${name}?`)) action.mutate({ name: "candidate-remove", body: { name } });
          }}
          secondaryLabel="Remove"
          destructiveSecondary
        />
      )}

      <InlineError error={action.error} />

      <Panel className="flush">
        <PanelHeader title="Resolved canonical state" icon="file-text" />
        <QueryState
          query={query}
          skeletonRows={3}
          empty={<EmptyState inline icon="file-text" label="No project settings are available." />}
        >
          {(data: unknown) => <DataView value={data} />}
        </QueryState>
      </Panel>
    </>
  );
}

/** One key (and optional value) with up to three canonical operations. */
function PairPanel({
  title,
  icon,
  subtitle,
  keyLabel,
  valueLabel,
  busy,
  onPrimary,
  primaryLabel,
  onSecondary,
  secondaryLabel,
  onTertiary,
  tertiaryLabel,
  destructiveSecondary = false,
}: {
  title: string;
  icon: Parameters<typeof Icon>[0]["name"];
  subtitle: string;
  keyLabel: string;
  valueLabel?: string;
  busy: boolean;
  onPrimary: (key: string, value?: string) => void;
  primaryLabel: string;
  onSecondary: (key: string) => void;
  secondaryLabel: string;
  onTertiary?: (key: string) => void;
  tertiaryLabel?: string;
  destructiveSecondary?: boolean;
}) {
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");

  return (
    <Panel className="padded stack tight">
      <PanelHeader title={title} icon={icon} subtitle={subtitle} plain />
      <label className="field">
        <span>{keyLabel}</span>
        <input
          className="input"
          aria-label={keyLabel}
          placeholder={keyLabel}
          value={key}
          onChange={(event) => setKey(event.target.value)}
        />
      </label>
      {valueLabel && (
        <label className="field">
          <span>{valueLabel}</span>
          <input
            className="input"
            aria-label={valueLabel}
            placeholder={valueLabel}
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
        </label>
      )}
      <div className="button-row">
        <button
          className="button primary"
          disabled={!key.trim() || busy || (Boolean(valueLabel) && primaryLabel === "Add" && !value.trim())}
          onClick={() => onPrimary(key.trim(), value.trim() || undefined)}
        >
          {primaryLabel}
        </button>
        {onTertiary && tertiaryLabel && (
          <button className="button" disabled={!key.trim() || busy} onClick={() => onTertiary(key.trim())}>
            {tertiaryLabel}
          </button>
        )}
        <button
          className={destructiveSecondary ? "button danger" : "button"}
          disabled={!key.trim() || busy}
          onClick={() => onSecondary(key.trim())}
        >
          {secondaryLabel}
        </button>
      </div>
    </Panel>
  );
}
