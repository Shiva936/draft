import { NavLink } from "react-router-dom";
import { CONSOLE_NAVIGATION } from "../../contracts";

/**
 * The views of one §8.3 section.
 *
 * Like the section list itself, the views come from the generated
 * authoritative IA rather than from a list written here. A section with no
 * children renders nothing: it is its own view, and a sub-navigation with one
 * tab in it is noise.
 */
export function SectionNav({
  section,
  routes,
}: {
  section: string;
  routes: Record<string, { to: string; end?: boolean }>;
}) {
  const declared = CONSOLE_NAVIGATION.PROJECT.find((entry) => entry.label === section);
  const children: readonly string[] = declared?.children ?? [];
  if (children.length === 0) return null;
  return (
    <nav className="tabs section-tabs" aria-label={`${section} navigation`}>
      {children.map((view) => {
        const route = routes[view];
        if (!route) throw new Error(`no route for the authoritative view '${section} › ${view}'`);
        return (
          <NavLink
            key={view}
            to={route.to}
            end={route.end}
            className={({ isActive }) => (isActive ? "active" : "")}
          >
            {view}
          </NavLink>
        );
      })}
    </nav>
  );
}
