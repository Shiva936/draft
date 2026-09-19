import { Icon } from "../icons";
import { initials } from "../lib/format";

/**
 * Identity mark for an actor. Draft has no profile images, so this is always
 * initials from the real identity, or a neutral icon when none is recorded.
 */
export function CandidateAvatar({
  name,
  accent = false,
  large = false,
}: {
  name: string | null | undefined;
  accent?: boolean;
  large?: boolean;
}) {
  const mark = initials(name);
  const className = `avatar${accent ? " accent" : ""}${large ? " large" : ""}`;
  return (
    <span className={className} title={name ?? undefined} aria-hidden="true">
      {mark ?? <Icon name="user" size={large ? 18 : 14} />}
    </span>
  );
}

/** Avatar plus the identity's display name. */
export function ActorLabel({ name, fallback = "—" }: { name: string | null | undefined; fallback?: string }) {
  return (
    <span className="avatar-label">
      <CandidateAvatar name={name} />
      <span className="truncate">{name || fallback}</span>
    </span>
  );
}
