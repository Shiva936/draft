import { useState } from "react";
import type { ConsoleReadModel } from "../../contracts";
import { Panel, PanelHeader } from "../../components/layout";
import { DataView } from "../../components/DataView";
import { EmptyState } from "../../components/states";

/**
 * One authoritative scope, rendered as the views its navigation declares.
 *
 * The section list, the view names and the content all come from `draftd`.
 * This picks a view and shows what the model holds under it; it derives
 * nothing. A view the model has no data for says so rather than rendering an
 * empty object, because "nothing was recorded" and "there is nothing to
 * record" are different answers and only one of them is about this Change.
 */
export function ScopeViews({
  model,
  paths,
  title,
  subtitle,
}: {
  model: ConsoleReadModel;
  /** Where each declared view's data lives in the model's content. */
  paths: Record<string, string[]>;
  title: string;
  subtitle: string;
}) {
  const views = model.navigation.flatMap((section) =>
    section.children.length > 0 ? section.children : [section.label],
  );
  const [active, setActive] = useState(views[0] ?? "");
  const selected = views.includes(active) ? active : (views[0] ?? "");
  const path = paths[selected];
  const value = path
    ? path.reduce<unknown>(
        (node, key) =>
          node && typeof node === "object" ? (node as Record<string, unknown>)[key] : undefined,
        model.content,
      )
    : undefined;

  return (
    <>
      <nav className="tabs section-tabs" aria-label={`${title} navigation`}>
        {views.map((view) => (
          <button
            key={view}
            type="button"
            className={view === selected ? "active" : ""}
            onClick={() => setActive(view)}
          >
            {view}
          </button>
        ))}
      </nav>
      <Panel>
        <PanelHeader title={`${title} · ${selected}`} subtitle={subtitle} />
        {value === undefined || value === null ? (
          <EmptyState
            label={`Nothing is recorded for ${selected.toLowerCase()} yet.`}
          />
        ) : (
          <DataView value={value} />
        )}
      </Panel>
    </>
  );
}
