/**
 * The Change Graph shapes the browser renders.
 *
 * Types only, plus two presentation helpers. Nothing here decides anything:
 * availability comes from the server, and this file merely reads the answer it
 * sent. A frontend that worked out for itself whether promoting was legal
 * would be a second authority for a question Core already answers.
 */
import type { Tone } from "../../../lib/format";

export type ActionAvailability = { action: string; available: boolean; reason?: string | null };

export type ProviderProvenance = { binding: string; semantic_definition: string };

export type BaselineView = {
  baseline: string;
  project: string;
  manifest: {
    project_state_root: string;
    state_evidence_root: string;
    coverage_evidence_root: string;
    parent_baseline_id?: string | null;
  };
  record: { origin: Record<string, unknown>; actor: string; accepted_at: number };
  lineage: string[];
  composition: Record<string, ProviderProvenance>;
  routable: boolean;
  route_refusal?: string | null;
};

export type OperationState =
  | "pending"
  | "blocked"
  | "running"
  | "recovering"
  | "completed"
  | "failed";

export type PublicationView = {
  publication: string;
  baseline: string;
  promotion: string;
  purpose: string;
  semantics: string;
  state: OperationState;
  outcome?: Record<string, unknown> | null;
  may_retry_automatically: boolean;
  detail: string;
};

export type ChangePackView = {
  change_pack: string;
  lifecycle: string;
  revisions: { id: string; sealed_at: number }[];
};

export type PromotionView = {
  promotion: string;
  state: OperationState;
  baseline?: string | null;
  detail: string;
};

export type GateView = {
  evaluation: { id: string };
  satisfied: boolean;
  unsatisfied: string[];
  waived: string[];
};

export type RepresentationView = {
  revision_pack: string;
  representations: { resource_id: string; strategy_id: string }[];
};

export type AuthorizationView = {
  change_pack: string;
  revision_pack: string;
  evidence: { id: string; outcome: string }[];
  assessments: { id: string; risk: string }[];
  representation?: RepresentationView | null;
  gates: GateView[];
  decisions: { id: string; outcome: Record<string, unknown> }[];
  promotion?: PromotionView | null;
  actions: ActionAvailability[];
};

export type GraphView = {
  project: string;
  baseline?: BaselineView | null;
  change_packs: ChangePackView[];
  publications: PublicationView[];
  actions: ActionAvailability[];
  next_action?: string | null;
};

/** The server's answer for one action, or a refusal when it sent none. */
export function availability(actions: ActionAvailability[], name: string): ActionAvailability {
  return (
    actions.find((action) => action.action === name) ?? {
      action: name,
      available: false,
      reason: "Unknown",
    }
  );
}

export const operationTone: Record<OperationState, Tone> = {
  pending: "neutral",
  blocked: "warning",
  running: "running",
  recovering: "warning",
  completed: "success",
  failed: "danger",
};

export function graphBase(workspaceId: string): string {
  return `/api/v1/projects/${encodeURIComponent(workspaceId)}/graph`;
}
