import { useMemo, useState } from "react";
import type {
  ActionInputField,
  ActionPresentation,
  CanonicalRevisions,
} from "../contracts";
import { Icon, type IconName } from "../icons";
import { Modal } from "./Modal";
import { InlineError } from "./states";

/**
 * Rendering for actions `draftd` issued.
 *
 * These components know nothing about extensions, sources, changes or any other
 * domain. They render the contract the server sent and send back what the user
 * supplied. Every question of substance — whether an action exists, whether it
 * is available, what it needs, and whether a submission is valid — was already
 * answered by `draftd`, and is never re-answered here.
 */

/** Values collected for one action's declared inputs, keyed by stable id. */
export type ActionArguments = Record<string, unknown>;

/**
 * Whether this capability has visibly aged out.
 *
 * Advisory only. The browser's clock is not trusted for authorization —
 * `draftd` decides a capability's real lifetime — so this exists purely to
 * avoid spending an invocation that is obviously stale, and to refresh instead.
 * A clock running behind simply lets the request through, and the server
 * refuses it; a clock running ahead causes one harmless extra refresh. Neither
 * grants, extends or revokes any authority.
 */
function looksExpired(action: ActionPresentation, now: number): boolean {
  return action.expires_at_unix_ms !== null && action.expires_at_unix_ms <= now;
}

/**
 * The initial value of each field, from its declared kind.
 *
 * `prefill` seeds fields whose value a caller already knows — a discovery row
 * knows the candidate it points at. Only declared field ids are honoured, so a
 * prefill can never smuggle in an argument the action did not ask for, and it
 * decides nothing: the server validates the submission either way.
 */
function initialValues(inputs: ActionInputField[], prefill: ActionArguments): ActionArguments {
  const values: ActionArguments = {};
  for (const input of inputs) {
    if (input.id in prefill) {
      values[input.id] = prefill[input.id];
      continue;
    }
    switch (input.kind.type) {
      case "select":
        // Start on the first option the server offered; an empty set leaves
        // nothing to choose and submits nothing.
        values[input.id] = input.kind.options[0]?.value ?? "";
        break;
      case "boolean":
      case "confirmation":
        values[input.id] = false;
        break;
      default:
        values[input.id] = "";
    }
  }
  return values;
}

/** Drop untouched optional text so the server sees only what was supplied. */
function submitted(inputs: ActionInputField[], values: ActionArguments): ActionArguments {
  const out: ActionArguments = {};
  for (const input of inputs) {
    const value = values[input.id];
    if (input.kind.type === "text" || input.kind.type === "select") {
      if (value === "" && !input.required) continue;
    }
    out[input.id] = value;
  }
  return out;
}

/**
 * A control for one server-issued action.
 *
 * An action that was not issued renders nothing at all: absence is the server
 * saying the action does not apply, and inventing a disabled placeholder would
 * be the frontend having an opinion. An action that was issued but disabled
 * shows the server's own reason.
 */
export function ActionButton({
  action,
  revisions,
  onInvoke,
  busy,
  className = "button",
  icon,
  label,
  prefill,
  onExpired,
}: {
  action: ActionPresentation | undefined;
  revisions: CanonicalRevisions;
  onInvoke: (capability: string, args: ActionArguments) => Promise<void> | void;
  busy?: boolean;
  className?: string;
  icon?: IconName;
  /** Overrides only the visible text; never what is invoked. */
  label?: string;
  /** Seed values for declared fields the caller already knows. */
  prefill?: ActionArguments;
  /**
   * Fetch a fresh authoritative model. Called instead of invoking when the
   * capability in hand has visibly expired, so the user acts against actions
   * Draft is currently issuing rather than ones it has retired.
   */
  onExpired?: () => void;
}) {
  const [open, setOpen] = useState(false);
  if (!action) return null;

  const disabled = !action.enabled || Boolean(busy) || !action.invocation_capability;
  const run = (args: ActionArguments) => {
    if (!action.invocation_capability) return;
    if (looksExpired(action, Date.now())) {
      // Do not spend a capability we can already see is stale. Refreshing
      // reissues the action — and if its target, revision or input contract
      // moved in the meantime, the user acts on the new one, not the old.
      onExpired?.();
      return;
    }
    void onInvoke(action.invocation_capability, args);
  };

  return (
    <>
      <button
        className={className}
        disabled={disabled}
        // The server's explanation, shown verbatim rather than reworded.
        title={action.enabled ? undefined : action.disabled_reason ?? undefined}
        onClick={() => {
          if (action.inputs.length > 0 || action.requires_confirmation) {
            setOpen(true);
            return;
          }
          run({});
        }}
      >
        {icon && <Icon name={icon} size={16} />}
        {label ?? action.label}
      </button>
      {open && (
        <ActionForm
          action={action}
          revisions={revisions}
          prefill={prefill}
          busy={Boolean(busy)}
          onCancel={() => setOpen(false)}
          onSubmit={(args) => {
            setOpen(false);
            run(args);
          }}
        />
      )}
    </>
  );
}

/**
 * The inputs an action declared, rendered exactly as described.
 *
 * Required-field checks here are a convenience so a person is not sent to the
 * server to be told something obvious. They are not the decision: `draftd`
 * validates every argument against the contract it issued, and its refusal is
 * what the user is shown.
 */
export function ActionForm({
  action,
  revisions,
  prefill,
  busy,
  onSubmit,
  onCancel,
}: {
  action: ActionPresentation;
  revisions: CanonicalRevisions;
  prefill?: ActionArguments;
  busy: boolean;
  onSubmit: (args: ActionArguments) => void;
  onCancel: () => void;
}) {
  const [values, setValues] = useState<ActionArguments>(() =>
    initialValues(action.inputs, prefill ?? {}),
  );
  const [attempted, setAttempted] = useState(false);
  const missing = useMemo(
    () =>
      action.inputs.filter((input) => {
        if (!input.required) return false;
        const value = values[input.id];
        if (input.kind.type === "confirmation") return value !== true;
        return value === "" || value === undefined;
      }),
    [action.inputs, values],
  );

  const set = (id: string, value: unknown) =>
    setValues((current) => ({ ...current, [id]: value }));

  return (
    <Modal
      title={action.label}
      onClose={onCancel}
      footer={
        <>
          <button className="button" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="button primary"
            disabled={busy}
            onClick={() => {
              setAttempted(true);
              if (missing.length > 0) return;
              onSubmit(submitted(action.inputs, values));
            }}
          >
            {action.label}
          </button>
        </>
      }
    >
      <div className="stack">
        {action.requires_confirmation && action.inputs.length === 0 && (
          <p className="muted">
            Draft revalidates permissions, lifecycle and revisions before this runs.
          </p>
        )}
        {action.inputs.map((input) => (
          <label className="field" key={input.id}>
            <span>
              {input.label}
              {input.required && " *"}
            </span>
            {input.kind.type === "select" ? (
              <select
                className="input"
                value={String(values[input.id] ?? "")}
                onChange={(event) => set(input.id, event.target.value)}
              >
                {input.kind.options.map((option) => (
                  // The value is what is submitted; the label is only read.
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            ) : input.kind.type === "boolean" || input.kind.type === "confirmation" ? (
              <input
                type="checkbox"
                aria-label={input.label}
                checked={values[input.id] === true}
                onChange={(event) => set(input.id, event.target.checked)}
              />
            ) : (
              <input
                className="input"
                aria-label={input.label}
                value={String(values[input.id] ?? "")}
                maxLength={input.kind.max_length ?? undefined}
                onChange={(event) => set(input.id, event.target.value)}
              />
            )}
            {input.help && <small className="muted">{input.help}</small>}
          </label>
        ))}
        {attempted && missing.length > 0 && (
          <InlineError
            error={new Error(`${missing.map((input) => input.label).join(", ")} required`)}
          />
        )}
        <p className="muted" style={{ fontSize: "var(--font-size-eyebrow)" }}>
          Acting against registry revision {revisions.registry}.
        </p>
      </div>
    </Modal>
  );
}
