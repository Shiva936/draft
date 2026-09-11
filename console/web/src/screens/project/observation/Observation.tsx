import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../../api";
import { Icon } from "../../../icons";
import { Definitions, Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, InlineError, QueryState } from "../../../components/states";
import { formatDateTime, humanize, shortDigest } from "../../../lib/format";
import { SectionNav } from "../SectionNav";
import { RESOURCE_ROUTES } from "../resources/routes";

/**
 * A coverage domain, named by the binding that minted it.
 *
 * `local_id` is opaque: it is the adapter's own name for part of its universe,
 * and it is meaningful only inside `adapter_binding_id`. Two adapters may both
 * call a domain "root" and mean unrelated things, so the pair is always shown
 * and never abbreviated to the local half.
 */
export type CoverageDomainRef = { adapter_binding_id: string; local_id: string };

export type ObservationCoverage = {
  domain: CoverageDomainRef;
  status: { status: "complete" } | { status: "incomplete"; gap_ids: string[] };
};

export type ObservationGap = {
  gap_id: string;
  kind: string;
  stable_code: string;
  coverage_domains: CoverageDomainRef[];
  binding_id?: string | null;
  /** Adapter-supplied prose. Rendered verbatim; never parsed as a path. */
  detail: string;
  producer?: string | null;
};

export type CoverageReport = {
  snapshot_id: string;
  snapshot_digest: string;
  observation_context_digest: string;
  domains: ObservationCoverage[];
  resource_membership: { resource_id: string; domain: CoverageDomainRef }[];
  gaps: ObservationGap[];
  status: { status: "complete" } | { status: "incomplete"; gap_ids: string[] };
};

/// A candidate set of semantics, waiting for somebody to decide about it.
export type PendingObservationContext = {
  candidate: { context_digest: string };
  active_context_digest: string;
  reasons: string[];
  detected_at: string;
};

export type ObservationContextPreview = {
  active_context_digest: string;
  candidate_context_digest: string;
  reasons: string[];
  added_bindings: string[];
  removed_bindings: string[];
  changed_bindings: string[];
  would_enter: { scheme: string; body: string }[];
  would_leave: { scheme: string; body: string }[];
  would_supersede: { kind: string; id: string; context_digest: string }[];
};

export type ObservationProvider =
  | { observed_by: "core"; component: string; implementation_revision: number }
  | {
      observed_by: "extension";
      producer: { extension_id: string; extension_version: string; package_digest: string; attestation_digest: string };
      artifact_attestation_digest: string;
    };

export type ObservationRun = {
  run_id: string;
  binding_id: string;
  effective_binding_digest: string;
  observation_provider: ObservationProvider;
  attempted_domains: CoverageDomainRef[];
  committed_domains: CoverageDomainRef[];
  authorization_decisions?: string[];
  environment_metadata_digest?: string | null;
  started_at: string;
  completed_at: string;
  outcome: { outcome: "succeeded" } | { outcome: "partial"; gap_ids: string[] } | { outcome: "failed"; gap_ids: string[] };
};

export type ObservationRunProvenance = {
  snapshot_digest: string;
  snapshot_id?: string | null;
  runs: ObservationRun[];
  view_rule_sources?: { binding_id: string; source: ObservationProvider }[];
  assembled_at: string;
  provenance_digest: string;
};

function domainKey(domain: CoverageDomainRef): string {
  return `${domain.adapter_binding_id}:${domain.local_id}`;
}

/** The scoped pair, always. The local half alone would not identify anything. */
function DomainRef({ domain }: { domain: CoverageDomainRef }) {
  return (
    <span className="chip mono" title="Adapter binding and its own domain name">
      {domain.adapter_binding_id}
      <span className="muted"> · </span>
      {domain.local_id}
    </span>
  );
}

/**
 * What the current observation established, and by which implementation.
 *
 * The two panels answer different questions and are deliberately not merged.
 * Coverage says what Draft could see, and therefore where absence is provable.
 * Provenance says who actually looked — which is history, not state: the same
 * observed state re-observed later gets its own record, and neither replaces
 * the other. Neither panel says anything about whether that state could be
 * restored; that is a separate capability with its own evidence.
 */
export function Observation() {
  const { workspaceId = "" } = useParams();
  const base = `/api/v1/projects/${encodeURIComponent(workspaceId)}`;

  const coverage = useQuery({
    queryKey: ["observation-coverage", workspaceId],
    queryFn: () => api<CoverageReport>(`${base}/observation-coverage`),
  });
  const provenance = useQuery({
    queryKey: ["observation-provenance", workspaceId],
    queryFn: () => api<ObservationRunProvenance[]>(`${base}/observation-provenance`),
  });
  const pending = useQuery({
    queryKey: ["observation-pending", workspaceId],
    queryFn: () =>
      api<PendingObservationContext | null>(`${base}/observation-pending`),
  });
  const [showPreview, setShowPreview] = useState(false);
  const preview = useQuery({
    queryKey: ["observation-preview", workspaceId],
    enabled: showPreview && Boolean(pending.data),
    queryFn: () => api<ObservationContextPreview>(`${base}/observation-preview`),
  });
  const client = useQueryClient();
  const adopt = useMutation({
    mutationFn: () => mutate(`${base}/actions/observation-adopt`, {}),
    onSuccess: () => {
      setShowPreview(false);
      client.invalidateQueries({ queryKey: ["observation-pending", workspaceId] });
      client.invalidateQueries({ queryKey: ["observation-coverage", workspaceId] });
      client.invalidateQueries({ queryKey: ["observation-provenance", workspaceId] });
      client.invalidateQueries({ queryKey: ["resources", workspaceId] });
    },
  });

  const gapsById = useMemo(() => {
    const map = new Map<string, ObservationGap>();
    for (const gap of coverage.data?.gaps ?? []) map.set(gap.gap_id, gap);
    return map;
  }, [coverage.data]);

  const membershipCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const entry of coverage.data?.resource_membership ?? []) {
      const key = domainKey(entry.domain);
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [coverage.data]);

  return (
    <div className="stack">
      <SectionNav section="Resources" routes={RESOURCE_ROUTES} />
      {pending.data && (
        <Panel className="padded stack tight">
          <PanelHeader
            title="Installed extensions would observe differently"
            icon="alert-triangle"
            subtitle="Nothing has changed yet"
            plain
            action={
              <div className="file-actions">
                <button onClick={() => setShowPreview((open) => !open)}>
                  <Icon name="eye" size={14} />
                  {showPreview ? "Hide preview" : "Preview"}
                </button>
                <button disabled={adopt.isPending} onClick={() => adopt.mutate()}>
                  <Icon name="check" size={14} />
                  Adopt
                </button>
              </div>
            }
          />
          <p className="muted">
            This project is still observed under the semantics it adopted. Adopting the
            candidate establishes a new baseline and supersedes work derived under the
            current one — that work stays readable, but must be re-derived before it can
            change or be submitted.
          </p>
          <Definitions rows>
            <dt>Because</dt>
            <dd>
              {pending.data.reasons.map((reason) => (
                <span className="chip" key={reason}>
                  {humanize(reason)}
                </span>
              ))}
            </dd>
            <dt>In force</dt>
            <dd className="mono">{shortDigest(pending.data.active_context_digest)}</dd>
            <dt>Candidate</dt>
            <dd className="mono">{shortDigest(pending.data.candidate.context_digest)}</dd>
          </Definitions>
          <InlineError error={adopt.error} />

          {showPreview && (
            <QueryState
              query={preview}
              empty={<EmptyState inline icon="eye" label="Nothing to preview." />}
            >
              {(plan) => (
                <div className="stack tight">
                  <Definitions rows>
                    <dt>Would enter</dt>
                    <dd>
                      {plan.would_enter.length === 0 ? (
                        <span className="muted">Nothing</span>
                      ) : (
                        plan.would_enter.map((locator) => (
                          <span className="chip mono" key={locator.body}>
                            {locator.body}
                          </span>
                        ))
                      )}
                    </dd>
                    <dt>Would leave</dt>
                    <dd>
                      {plan.would_leave.length === 0 ? (
                        <span className="muted">Nothing</span>
                      ) : (
                        plan.would_leave.map((locator) => (
                          <span className="chip mono" key={locator.body}>
                            {locator.body}
                          </span>
                        ))
                      )}
                    </dd>
                    <dt>Would supersede</dt>
                    <dd>
                      {plan.would_supersede.length === 0 ? (
                        <span className="muted">No open work</span>
                      ) : (
                        plan.would_supersede.map((work) => (
                          <span className="chip mono" key={work.id}>
                            {humanize(work.kind)} {work.id}
                          </span>
                        ))
                      )}
                    </dd>
                  </Definitions>
                  <p className="muted">
                    Previewing changes nothing: the trial observation behind these figures is
                    discarded.
                  </p>
                </div>
              )}
            </QueryState>
          )}
        </Panel>
      )}

      <Panel className="flush">
        <PanelHeader
          title="Coverage"
          icon="eye"
          subtitle="Where absence is provable, and where it is not"
          count={coverage.data?.domains.length}
          action={
            coverage.data && (
              <StatusBadge
                value={coverage.data.status.status === "complete" ? "complete" : "incomplete"}
                tone={coverage.data.status.status === "complete" ? "success" : "warning"}
              />
            )
          }
        />
        <QueryState
          query={coverage}
          empty={
            <EmptyState
              inline
              icon="eye"
              label="No coverage recorded."
              detail="Observe the project to establish which domains it covers."
            />
          }
        >
          {(report) => (
            <div className="stack tight padded">
              <Definitions rows>
                <dt>Observed state</dt>
                <dd className="mono">{shortDigest(report.snapshot_digest)}</dd>
                <dt>Observation context</dt>
                <dd className="mono">{shortDigest(report.observation_context_digest)}</dd>
              </Definitions>
              <div className="rows">
                {report.domains.map((entry) => {
                  const key = domainKey(entry.domain);
                  const status = entry.status;
                  const incomplete = status.status === "incomplete";
                  return (
                    <div className="row-item" key={key}>
                      <Icon name={incomplete ? "alert-triangle" : "shield-check"} size={16} />
                      <div className="row-main">
                        <DomainRef domain={entry.domain} />
                        <small className="muted">
                          {membershipCounts.get(key) ?? 0} resources ·{" "}
                          {incomplete
                            ? "absence inside this domain proves nothing"
                            : "absence inside this domain is authoritative"}
                        </small>
                        {status.status === "incomplete" &&
                          status.gap_ids.map((gapId: string) => {
                            const gap = gapsById.get(gapId);
                            return (
                              <small className="muted" key={gapId}>
                                {gap ? `${humanize(gap.kind)} — ${gap.detail}` : gapId}
                              </small>
                            );
                          })}
                      </div>
                      <StatusBadge
                        value={incomplete ? "incomplete" : "complete"}
                        tone={incomplete ? "warning" : "success"}
                        plain
                      />
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </QueryState>
      </Panel>

      {(coverage.data?.gaps.length ?? 0) > 0 && (
        <Panel className="flush">
          <PanelHeader
            title="Gaps"
            icon="alert-triangle"
            subtitle="Reported by the adapter, in its own terms"
            count={coverage.data?.gaps.length}
          />
          <div className="rows">
            {coverage.data?.gaps.map((gap) => (
              <div className="row-item" key={gap.gap_id}>
                <Icon name="alert-triangle" size={16} />
                <div className="row-main">
                  <strong>{humanize(gap.kind)}</strong>
                  <small className="mono">{gap.stable_code}</small>
                  <small>{gap.detail}</small>
                  <small className="muted">
                    {gap.coverage_domains.map((domain) => (
                      <DomainRef domain={domain} key={domainKey(domain)} />
                    ))}
                  </small>
                </div>
              </div>
            ))}
          </div>
        </Panel>
      )}

      <Panel className="flush">
        <PanelHeader
          title="Observation history"
          icon="clock"
          subtitle="Which implementation actually observed this state"
          count={provenance.data?.length}
        />
        <QueryState
          query={provenance}
          empty={
            <EmptyState
              inline
              icon="clock"
              label="No observation record for this state."
              detail="A record is written the next time Draft observes the project."
            />
          }
        >
          {(records) => (
            <div className="stack tight padded">
              {records.length > 1 && (
                <p className="muted">
                  The same state was observed more than once. Each record is a separate historical
                  observation; a receipt that relied on one keeps pointing at that one.
                </p>
              )}
              {records.map((record) => (
                <Panel className="padded stack tight" key={record.provenance_digest}>
                  <Definitions rows>
                    <dt>Assembled</dt>
                    <dd>{formatDateTime(record.assembled_at)}</dd>
                    <dt>Record</dt>
                    <dd className="mono">{shortDigest(record.provenance_digest)}</dd>
                  </Definitions>
                  <div className="rows">
                    {record.runs.map((run) => (
                      <div className="row-item" key={run.run_id}>
                        <Icon
                          name={run.outcome.outcome === "succeeded" ? "shield-check" : "alert-triangle"}
                          size={16}
                        />
                        <div className="row-main">
                          <strong>
                            {run.observation_provider.observed_by === "core"
                              ? `Draft — ${run.observation_provider.component} (revision ${run.observation_provider.implementation_revision})`
                              : `${run.observation_provider.producer.extension_id} ${run.observation_provider.producer.extension_version}`}
                          </strong>
                          {run.observation_provider.observed_by === "extension" && (
                            <small className="muted mono">
                              attested {shortDigest(run.observation_provider.artifact_attestation_digest)}
                              {run.authorization_decisions?.length
                                ? ` · authorized ${shortDigest(run.authorization_decisions[0])}`
                                : ""}
                            </small>
                          )}
                          <small className="muted">
                            attempted {run.attempted_domains.length} · committed{" "}
                            {run.committed_domains.length}
                            {run.committed_domains.length === 0 && " — nothing from this attempt was retained"}
                          </small>
                          <small className="muted mono">
                            binding {run.binding_id} · {shortDigest(run.effective_binding_digest)}
                          </small>
                        </div>
                        <StatusBadge
                          value={run.outcome.outcome}
                          tone={run.outcome.outcome === "succeeded" ? "success" : "warning"}
                          plain
                        />
                      </div>
                    ))}
                  </div>
                  {(record.view_rule_sources?.length ?? 0) > 0 && (
                    <Definitions rows>
                      <dt>View rules</dt>
                      <dd>
                        {record.view_rule_sources?.map((source) => (
                          <span className="chip mono" key={source.binding_id}>
                            {source.binding_id}
                          </span>
                        ))}
                      </dd>
                    </Definitions>
                  )}
                </Panel>
              ))}
            </div>
          )}
        </QueryState>
      </Panel>

      <Panel className="padded">
        <p className="muted">
          Coverage describes what could be seen, not what could be put back. Whether a state can be
          recreated is a separate capability with its own retained evidence, reported where recovery
          is reported.
        </p>
      </Panel>
    </div>
  );
}
