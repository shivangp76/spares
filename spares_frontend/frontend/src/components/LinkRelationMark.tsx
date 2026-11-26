import type { LinkRelation } from "../types/spares";

/**
 * A muted word after a link that isn't to a prerequisite, the default: `builds on this` for a
 * dependent link, whose note builds on the one with the link, and `related` for a related one.
 */
export default function LinkRelationMark({ relation }: { relation: LinkRelation | undefined }) {
  if (relation !== "dependent" && relation !== "related") return null;
  return (
    <span style={{ color: "var(--text-muted)", marginLeft: 4 }}>
      · {relation === "dependent" ? "builds on this" : "related"}
    </span>
  );
}
