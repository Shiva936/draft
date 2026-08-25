import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../api";
import { Icon } from "../../icons";
import { Definitions, Panel, PanelHeader } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { CandidateAvatar } from "../../components/CandidateAvatar";
import { InlineError, QueryState, Skeleton } from "../../components/states";
import { NONE, humanize } from "../../lib/format";
import { useDaemonConnection } from "../../lib/hooks";
import {
  ACCENTS,
  useAccent,
  useRestorePacks,
  useStartupView,
  useTheme,
  type AccentName,
  type StartupView,
  type Theme,
} from "../../lib/preferences";
import type { GlobalSection } from "./Settings";

export function GlobalSettings({ section }: { section: GlobalSection }) {
  const query = useQuery({ queryKey: ["settings"], queryFn: () => api<any>("/api/v1/settings") });
  const daemon = useDaemonConnection();

  if (query.isLoading) return <Skeleton rows={5} />;

  return (
    <div className="settings-columns">
      {(section === "identity" || section === "store") && (
        <QueryState query={query} empty={<Panel className="padded">No profile metadata is configured.</Panel>}>
          {(data: any) => (
            <>
              {section === "identity" && <IdentityPanel data={data} />}
              {section === "store" && <StorePanel data={data} />}
            </>
          )}
        </QueryState>
      )}

      {section === "theme" && <ThemePanel />}
      {section === "console" && <ConsolePanel />}
      {section === "daemon" && (
        <Panel className="padded stack tight">
          <PanelHeader title="Daemon" icon="zap" subtitle="Local draftd connection state." plain />
          <Definitions rows>
            <dt>Connection</dt>
            <dd>
              <StatusBadge value={daemon} plain />
            </dd>
            <dt>Transport</dt>
            <dd>Loopback IPC</dd>
            <dt>Product version</dt>
            <dd>{query.data?.product_version ?? NONE}</dd>
          </Definitions>
          <p className="muted">
            If draftd disconnects, reads report a structured offline state and canonical project state remains on
            disk. Restart with <code>draft service restart</code>.
          </p>
        </Panel>
      )}
    </div>
  );
}

function IdentityPanel({ data }: { data: any }) {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [clearName, setClearName] = useState(false);
  const [clearEmail, setClearEmail] = useState(false);

  const save = useMutation({
    mutationFn: () =>
      mutate("/api/v1/settings/user", {
        ...(clearName ? { name: null } : name.trim() ? { name } : {}),
        ...(clearEmail ? { email: null } : email.trim() ? { email } : {}),
      }),
    onSuccess: () => {
      setName("");
      setEmail("");
      setClearName(false);
      setClearEmail(false);
      setEditing(false);
      void queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });

  const unchanged = !name.trim() && !email.trim() && !clearName && !clearEmail;

  return (
    <>
      <Panel className="padded stack tight">
        <PanelHeader
          title="Identity"
          icon="user"
          subtitle="Name and email used across Draft Console."
          plain
          action={
            <button className="button" onClick={() => setEditing((value) => !value)}>
              <Icon name="pencil" size={16} />
              {editing ? "Cancel" : "Edit"}
            </button>
          }
        />

        <div className="setting-row">
          <div>
            <strong>Full name</strong>
            <p>{data.user?.name_source ? `Source: ${humanize(data.user.name_source)}` : "Non-persisted fallback"}</p>
          </div>
          <span className="avatar-label">
            <CandidateAvatar name={data.user?.name} accent />
            <span>{data.user?.name ?? "unknown"}</span>
          </span>
        </div>

        <div className="setting-row">
          <div>
            <strong>Email address</strong>
            <p>{data.user?.email_source ? `Source: ${humanize(data.user.email_source)}` : "Not set"}</p>
          </div>
          <span>{data.user?.email ?? NONE}</span>
        </div>

        {editing && (
          <div className="stack tight">
            <label className="field">
              <span>New display name</span>
              <input
                className="input"
                aria-label="New display name"
                placeholder="New display name"
                value={name}
                disabled={clearName}
                onChange={(event) => setName(event.target.value)}
              />
            </label>
            <label className="checkbox">
              <input type="checkbox" checked={clearName} onChange={(event) => setClearName(event.target.checked)} />
              Use fallback name
            </label>
            <label className="field">
              <span>New email</span>
              <input
                className="input"
                aria-label="New user email"
                placeholder="New email"
                value={email}
                disabled={clearEmail}
                onChange={(event) => setEmail(event.target.value)}
              />
            </label>
            <label className="checkbox">
              <input type="checkbox" checked={clearEmail} onChange={(event) => setClearEmail(event.target.checked)} />
              Unset email
            </label>
            <button className="button primary" disabled={unchanged || save.isPending} onClick={() => save.mutate()}>
              {save.isPending ? "Saving…" : "Update profile"}
            </button>
            <InlineError error={save.error} />
          </div>
        )}

        <p className="muted">
          Display and contact metadata only. These values never affect signatures, authorization, trust, attribution,
          receipts, event hashes, ownership, or digests.
        </p>
      </Panel>

      <Panel className="padded stack tight">
        <PanelHeader title="Security identity" icon="key" subtitle="Read-only canonical actor state." plain />
        <Definitions rows>
          <dt>Actor ID</dt>
          <dd className="mono">{data.security?.actor?.actor_id ?? NONE}</dd>
          <dt>Public key</dt>
          <dd className="mono">{data.security?.actor?.public_key_id ?? NONE}</dd>
        </Definitions>
      </Panel>
    </>
  );
}

function StorePanel({ data }: { data: any }) {
  return (
    <Panel className="padded stack tight">
      <PanelHeader title="Global store" icon="database" subtitle="Platform-resolved Draft store location." plain />
      <Definitions rows>
        <dt>Location</dt>
        <dd className="mono">{data.global_store ?? NONE}</dd>
        <dt>Product version</dt>
        <dd>{data.product_version ?? NONE}</dd>
      </Definitions>
      <p className="muted">
        The project registry is an atomic registered contract in this store. Filesystem location is mutable metadata;
        each workspace id is immutable.
      </p>
    </Panel>
  );
}

function ThemePanel() {
  const [theme, setTheme] = useTheme();
  const [accent, setAccent] = useAccent();
  const options: { value: Theme; label: string; icon: "sun" | "moon" | "monitor" }[] = [
    { value: "light", label: "Light", icon: "sun" },
    { value: "dark", label: "Dark", icon: "moon" },
    { value: "system", label: "System", icon: "monitor" },
  ];

  return (
    <Panel className="padded stack tight">
      <PanelHeader title="Theme" icon="sun" subtitle="Choose your preferred appearance." plain />
      <div className="theme-options">
        {options.map((option) => (
          <button
            key={option.value}
            className={theme === option.value ? "selected" : ""}
            aria-pressed={theme === option.value}
            onClick={() => setTheme(option.value)}
          >
            <Icon name={option.icon} size={20} />
            {option.label}
            <span className="radio-mark" />
          </button>
        ))}
      </div>

      <div className="stack tight">
        <div>
          <strong>Color accent</strong>
          <p className="muted">Customize the primary accent used across the console.</p>
        </div>
        <div className="accent-options">
          {(Object.keys(ACCENTS) as AccentName[]).map((name) => (
            <button
              key={name}
              className={accent === name ? "selected" : ""}
              style={{ background: ACCENTS[name].surface, color: ACCENTS[name].surface }}
              aria-pressed={accent === name}
              aria-label={ACCENTS[name].label}
              title={ACCENTS[name].label}
              onClick={() => setAccent(name)}
            />
          ))}
        </div>
      </div>

      <p className="muted">Only presentation preferences are stored in the browser.</p>
    </Panel>
  );
}

function ConsolePanel() {
  const [startup, setStartup] = useStartupView();
  const [restore, setRestore] = useRestorePacks();

  return (
    <Panel className="padded stack tight">
      <PanelHeader title="Console startup" icon="monitor" subtitle="What the console opens when you launch it." plain />

      <div className="setting-row">
        <div>
          <strong>On startup</strong>
          <p>A preselected project from `draft console --project` always wins.</p>
        </div>
        <select
          className="select"
          aria-label="Startup view"
          value={startup}
          onChange={(event) => setStartup(event.target.value as StartupView)}
        >
          <option value="last">Open last visited</option>
          <option value="overview">Open overview</option>
          <option value="projects">Open projects</option>
          <option value="inbox">Open inbox</option>
        </select>
      </div>

      <div className="setting-row">
        <div>
          <strong>Restore open packs</strong>
          <p>Reopen the pack that was selected during the last session.</p>
        </div>
        <button
          className="switch"
          role="switch"
          aria-checked={restore}
          aria-label="Restore open packs"
          onClick={() => setRestore(!restore)}
        />
      </div>

      <p className="muted">These preferences live in this browser only and never reach canonical Draft state.</p>
    </Panel>
  );
}
