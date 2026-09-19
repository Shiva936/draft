import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import { Panel, PanelHeader } from "../../../components/layout";
import { StatusBadge } from "../../../components/StatusBadge";
import { EmptyState, QueryState } from "../../../components/states";
import { SectionNav } from "../SectionNav";
import { EXTENSION_ROUTES } from "./routes";

/**
 * What installed extensions contribute to *this* project.
 *
 * Distinct from the system Extensions screen, which is about the installation:
 * what is installed, authorized and available machine-wide. This view is about
 * the project — which contributions are active here, and what Draft could not
 * interpret because nothing claimed it.
 *
 * Extensions are not providers. An extension contributes capability to Draft;
 * a provider binding attaches this project to a system that observes it. They
 * are separate sections because conflating them would suggest installing
 * something could change what established a Baseline.
 */
type CapabilityGap = { kind: string; reason: string; remediation_action_id?: string | null };

type Classification = {
  capability_gaps?: CapabilityGap[];
  classified?: unknown[];
  [key: string]: unknown;
};

export function ProjectExtensions() {
  const { workspaceId = "" } = useParams();
  const query = useQuery({
    queryKey: ["project-classification", workspaceId],
    queryFn: () =>
      api<Classification>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/classification`),
  });

  return (
    <>
      <SectionNav section="Extensions" routes={EXTENSION_ROUTES} />
      <QueryState
        query={query}
        skeletonRows={4}
        empty={
          <Panel>
            <EmptyState label="Nothing installed describes this project." />
          </Panel>
        }
      >
        {(report: Classification) => {
          const gaps = report.capability_gaps ?? [];
          return (
            <Panel>
              <PanelHeader
                title="Contributions to this project"
                subtitle="What installed extensions describe here. Draft renders this; it does not derive it."
              />
              {gaps.length === 0 ? (
                <EmptyState label="Nothing in this project is waiting on a capability nobody installed." />
              ) : (
                <ul className="list">
                  {gaps.map((gap, index) => (
                    <li key={`${gap.kind}-${index}`}>
                      <StatusBadge value={gap.kind} tone="warning" />{" "}
                      {/* A gap is not a failure. It says installing something
                          would change the answer, which is a different fact
                          from the answer being no. */}
                      {gap.reason}
                    </li>
                  ))}
                </ul>
              )}
            </Panel>
          );
        }}
      </QueryState>
    </>
  );
}
