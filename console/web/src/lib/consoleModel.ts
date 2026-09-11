import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import type { ActionPresentation, ConsoleReadModel } from "../contracts";

/**
 * The authoritative Console read model.
 *
 * State and the actions Draft currently issues arrive together, from one
 * revision, so they cannot disagree. The browser renders them and decides
 * nothing: which actions exist, whether they are available, and what they need
 * are all settled by `draftd`.
 */
export const CONSOLE_MODEL_KEY = ["console-model", "GLOBAL"] as const;

export function useConsoleModel() {
  return useQuery({
    queryKey: CONSOLE_MODEL_KEY,
    queryFn: () => api<ConsoleReadModel>("/api/v1/console/model?scope=GLOBAL"),
    // The model carries short-lived invocation capabilities, so it is never
    // served from cache and never structurally merged: a refetch is a
    // replacement, and an action the new model omits must disappear along with
    // its capability rather than surviving as a stale reference.
    staleTime: 0,
    gcTime: 0,
    structuralSharing: false,
  });
}

/** Discard the model outright, so the next read comes from `draftd`. */
export function useRefreshConsoleModel() {
  const client = useQueryClient();
  return () => {
    client.removeQueries({ queryKey: CONSOLE_MODEL_KEY });
    void client.invalidateQueries({ queryKey: CONSOLE_MODEL_KEY });
  };
}

export function projectModelKey(workspaceId: string) {
  return ["console-model", "PROJECT", workspaceId] as const;
}

/**
 * The authoritative Console model for one project.
 *
 * Same contract as the global one: state and the actions Draft currently
 * issues arrive together from one revision, and the capabilities they carry
 * are short-lived, so nothing here is cached or structurally merged.
 */
export function useProjectConsoleModel(workspaceId: string) {
  return useQuery({
    queryKey: projectModelKey(workspaceId),
    queryFn: () =>
      api<ConsoleReadModel>(
        `/api/v1/console/model?scope=PROJECT&workspace_id=${encodeURIComponent(workspaceId)}`,
      ),
    staleTime: 0,
    gcTime: 0,
    structuralSharing: false,
  });
}

/** Discard one project's model, so the next read comes from `draftd`. */
export function useRefreshProjectConsoleModel(workspaceId: string) {
  const client = useQueryClient();
  return () => {
    client.removeQueries({ queryKey: projectModelKey(workspaceId) });
    void client.invalidateQueries({ queryKey: projectModelKey(workspaceId) });
  };
}

/**
 * View one part of the model as a query, keeping its loading and error states.
 *
 * `QueryState` decides emptiness from the value it is given, and the model is
 * an object that is never "empty" — so a screen listing installed extensions
 * hands it the list while still reporting the model's own loading and failure.
 */
export function projected<T>(
  query: {
    isLoading: boolean;
    error: unknown;
    refetch: () => unknown;
  },
  data: T | undefined,
) {
  return {
    isLoading: query.isLoading,
    error: query.error,
    data,
    refetch: query.refetch,
  };
}

/**
 * Server-issued actions, looked up by stable machine identity.
 *
 * The key is the action id plus its target id — never a label, never a list
 * position. A caller asks for what it wants to render; if `draftd` did not
 * issue it, the answer is `undefined` and nothing is rendered.
 */
export class ActionIndex {
  private readonly byKey: Map<string, ActionPresentation>;

  constructor(actions: ActionPresentation[]) {
    this.byKey = new Map(
      actions.map((action) => [ActionIndex.key(action.action_id, action.target?.id), action]),
    );
  }

  private static key(actionId: string, targetId?: string | null): string {
    return targetId ? `${actionId} ${targetId}` : actionId;
  }

  /** The action `draftd` issued for this id and target, if it issued one. */
  get(actionId: string, targetId?: string | null): ActionPresentation | undefined {
    return this.byKey.get(ActionIndex.key(actionId, targetId));
  }
}
