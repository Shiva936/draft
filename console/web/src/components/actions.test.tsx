// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ActionButton } from "./actions";
import { ActionIndex } from "../lib/consoleModel";
import type { ActionPresentation, CanonicalRevisions } from "../contracts";

/**
 * Presentation is never authority.
 *
 * These fixtures are deliberately contradictory: raw extension and source
 * state says one thing while the actions Draft issued say another. The
 * server-issued action must win every time. If the Console had kept a hidden
 * fallback rule — "an enabled extension shows Disable", "an untrusted source
 * cannot refresh" — these tests would catch it, which a source-level grep
 * could not.
 */

declare global {
  // eslint-disable-next-line no-var
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const revisions: CanonicalRevisions = {
  registry: 7,
  workspace: null,
  change_pack: null,
  policy: null,
};

function action(overrides: Partial<ActionPresentation> = {}): ActionPresentation {
  return {
    action_id: "extension.enable",
    label: "Enable extension",
    enabled: true,
    disabled_reason: null,
    invocation_capability: "cap-1",
    requires_confirmation: false,
    expires_at_unix_ms: Number.MAX_SAFE_INTEGER,
    inputs: [],
    input_contract_digest: "digest",
    target: { kind: "extension", id: "draft.language.rust" },
    ...overrides,
  };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function render(node: React.ReactNode) {
  act(() => root.render(node));
}

function buttons(): HTMLButtonElement[] {
  return [...container.querySelectorAll("button")];
}

function click(label: string) {
  const target = buttons().find((button) => button.textContent?.includes(label));
  if (!target) throw new Error(`no control labelled ${label}`);
  act(() => target.click());
}

/** Submit the open form, whose confirm button sits in the dialog footer. */
function submit(label: string) {
  const dialog = container.querySelector(".modal");
  if (!dialog) throw new Error("no form is open");
  const target = [...dialog.querySelectorAll("footer button")].find((button) =>
    button.textContent?.includes(label),
  ) as HTMLButtonElement | undefined;
  if (!target) throw new Error(`no submit labelled ${label}`);
  act(() => target.click());
}

describe("a control exists only because draftd issued it", () => {
  it("renders nothing at all when the action was not issued", () => {
    // The screen asked for an action Draft did not issue. Nothing is rendered
    // — not a disabled placeholder, which would be the Console having an
    // opinion about an action that does not apply.
    render(
      <ActionButton action={undefined} revisions={revisions} onInvoke={() => {}} />,
    );
    expect(buttons()).toHaveLength(0);
  });

  it("renders the issued action even when raw state suggests the opposite", () => {
    // Contradictory on purpose: the extension reads as enabled, and Draft
    // issued Enable anyway. A Console that kept `enabled ? Disable : Enable`
    // would render the wrong control here.
    render(
      <ActionButton
        action={action({ action_id: "extension.enable", label: "Enable extension" })}
        revisions={revisions}
        onInvoke={() => {}}
      />,
    );
    expect(buttons()).toHaveLength(1);
    expect(buttons()[0].textContent).toContain("Enable extension");
    expect(buttons()[0].disabled).toBe(false);
  });

  it("disables with the server's own reason and invokes nothing", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton
        action={action({ enabled: false, disabled_reason: "The extension is already enabled" })}
        revisions={revisions}
        onInvoke={onInvoke}
      />,
    );
    const [button] = buttons();
    expect(button.disabled).toBe(true);
    // Verbatim, not reworded by the Console.
    expect(button.title).toBe("The extension is already enabled");
    act(() => button.click());
    expect(onInvoke).not.toHaveBeenCalled();
  });

  it("invokes with the capability the server issued", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton action={action()} revisions={revisions} onInvoke={onInvoke} />,
    );
    click("Enable extension");
    expect(onInvoke).toHaveBeenCalledWith("cap-1", {});
  });
});

describe("declared inputs are rendered and submitted by stable identity", () => {
  const withInputs = action({
    action_id: "extension.install",
    label: "Install extension",
    target: null,
    inputs: [
      {
        id: "source_id",
        label: "Source",
        kind: {
          type: "select",
          options: [
            { value: "draft-official", label: "Draft Official" },
            { value: "acme", label: "ACME" },
          ],
        },
        required: true,
        help: null,
      },
      {
        id: "version",
        label: "Version",
        kind: { type: "text", max_length: null },
        required: false,
        help: null,
      },
    ],
  });

  it("submits option values, never the labels a person reads", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton action={withInputs} revisions={revisions} onInvoke={onInvoke} />,
    );
    click("Install extension");

    const select = container.querySelector("select")!;
    // The rendered options show labels…
    expect([...select.options].map((option) => option.textContent)).toEqual([
      "Draft Official",
      "ACME",
    ]);
    // …and carry values, which is what is submitted.
    expect(select.value).toBe("draft-official");

    submit("Install extension");
    expect(onInvoke).toHaveBeenCalledWith("cap-1", { source_id: "draft-official" });
  });

  it("omits an untouched optional field rather than sending it blank", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton action={withInputs} revisions={revisions} onInvoke={onInvoke} />,
    );
    click("Install extension");
    submit("Install extension");
    const [, args] = onInvoke.mock.calls[0];
    expect(args).not.toHaveProperty("version");
  });

  it("prefills only fields the action declared", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton
        action={withInputs}
        revisions={revisions}
        onInvoke={onInvoke}
        prefill={{ source_id: "acme", version: "2.0.0", smuggled: "value" }}
      />,
    );
    click("Install extension");
    submit("Install extension");
    // A candidate can seed declared inputs; it cannot introduce an argument
    // the action never asked for.
    expect(onInvoke).toHaveBeenCalledWith("cap-1", {
      source_id: "acme",
      version: "2.0.0",
    });
  });

  it("holds back a required confirmation until it is acknowledged", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton
        action={action({
          action_id: "extension.authorize",
          label: "Authorize capability",
          inputs: [
            {
              id: "acknowledged",
              label: "Grant process.execute to this exact build",
              kind: { type: "confirmation" },
              required: true,
              help: null,
            },
          ],
        })}
        revisions={revisions}
        onInvoke={onInvoke}
      />,
    );
    click("Authorize capability");
    // Submitting unacknowledged does not invoke; the server would refuse it
    // anyway, and this only saves the round trip.
    submit("Authorize capability");
    expect(onInvoke).not.toHaveBeenCalled();

    const checkbox = container.querySelector("input[type=checkbox]") as HTMLInputElement;
    act(() => checkbox.click());
    submit("Authorize capability");
    expect(onInvoke).toHaveBeenCalledWith("cap-1", { acknowledged: true });
  });
});

describe("actions are found by stable machine identity", () => {
  it("keys on action id and target, never on a label", () => {
    const index = new ActionIndex([
      action({ action_id: "extension.enable", label: "Enable extension" }),
      action({
        action_id: "extension.enable",
        label: "Activer l'extension",
        target: { kind: "extension", id: "draft.language.python" },
      }),
      action({ action_id: "extension.update_all", label: "Update all", target: null }),
    ]);

    expect(index.get("extension.enable", "draft.language.rust")?.label).toBe(
      "Enable extension",
    );
    // Relabelled and translated, and still found by the same stable key.
    expect(index.get("extension.enable", "draft.language.python")?.label).toBe(
      "Activer l'extension",
    );
    expect(index.get("extension.update_all")).toBeDefined();
    // Not issued for this target: the answer is nothing, so nothing renders.
    expect(index.get("extension.revoke", "draft.language.rust")).toBeUndefined();
    expect(index.get("extension.enable", "draft.language.go")).toBeUndefined();
  });

  it("does not confuse a targeted action with an untargeted one", () => {
    const index = new ActionIndex([
      action({ action_id: "extension.update", target: { kind: "extension", id: "a" } }),
    ]);
    expect(index.get("extension.update")).toBeUndefined();
    expect(index.get("extension.update", "a")).toBeDefined();
  });
});

describe("expiry is a browser optimization, never an authority", () => {
  const expiring = (expiresAt: number) =>
    action({ expires_at_unix_ms: expiresAt, label: "Enable extension" });

  it("refreshes instead of spending a capability it can see is stale", () => {
    const onInvoke = vi.fn();
    const onExpired = vi.fn();
    vi.spyOn(Date, "now").mockReturnValue(10_000);
    render(
      <ActionButton
        action={expiring(9_000)}
        revisions={revisions}
        onInvoke={onInvoke}
        onExpired={onExpired}
      />,
    );
    click("Enable extension");
    expect(onInvoke).not.toHaveBeenCalled();
    expect(onExpired).toHaveBeenCalledTimes(1);
    vi.restoreAllMocks();
  });

  it("lets a capability it believes valid through, for the server to judge", () => {
    // The clock runs behind, so the browser thinks this is still live. It
    // invokes, and draftd — the only authority on a capability's lifetime —
    // refuses it. Nothing is granted by the browser being wrong.
    const onInvoke = vi.fn();
    const onExpired = vi.fn();
    vi.spyOn(Date, "now").mockReturnValue(1_000);
    render(
      <ActionButton
        action={expiring(5_000)}
        revisions={revisions}
        onInvoke={onInvoke}
        onExpired={onExpired}
      />,
    );
    click("Enable extension");
    expect(onInvoke).toHaveBeenCalledWith("cap-1", {});
    expect(onExpired).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });

  it("a clock running ahead costs one refresh and no authority", () => {
    const onInvoke = vi.fn();
    const onExpired = vi.fn();
    // Far ahead of the server: everything looks expired.
    vi.spyOn(Date, "now").mockReturnValue(Number.MAX_SAFE_INTEGER);
    render(
      <ActionButton
        action={expiring(5_000)}
        revisions={revisions}
        onInvoke={onInvoke}
        onExpired={onExpired}
      />,
    );
    click("Enable extension");
    expect(onExpired).toHaveBeenCalled();
    expect(onInvoke).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });

  it("an action with no declared expiry is simply invoked", () => {
    const onInvoke = vi.fn();
    render(
      <ActionButton
        action={action({ expires_at_unix_ms: null })}
        revisions={revisions}
        onInvoke={onInvoke}
      />,
    );
    click("Enable extension");
    expect(onInvoke).toHaveBeenCalled();
  });

  it("cannot be talked out of the server's own disabled decision", () => {
    // A locally edited expiry cannot make a disabled action invocable: the
    // control is disabled because draftd said so, and expiry is not consulted.
    const onInvoke = vi.fn();
    vi.spyOn(Date, "now").mockReturnValue(0);
    render(
      <ActionButton
        action={action({
          enabled: false,
          disabled_reason: "Nothing is awaiting authorization",
          expires_at_unix_ms: Number.MAX_SAFE_INTEGER,
        })}
        revisions={revisions}
        onInvoke={onInvoke}
      />,
    );
    const [button] = buttons();
    expect(button.disabled).toBe(true);
    act(() => button.click());
    expect(onInvoke).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });
});
