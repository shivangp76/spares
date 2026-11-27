import {
  formatRetention,
  retentionTitle,
  retentionTone,
} from "../tagRetention";
import type { TagRetention } from "../types/spares";

/** A tag's predicted retention in 7 days as a small bar, subtly coloured by how high it is, with
    today's figure and the card count in its tooltip. */
export default function TagRetentionBar({
  retention,
}: {
  retention: TagRetention | undefined;
}) {
  if (!retention) return null;
  const tone = retentionTone(retention.week);
  return (
    <span className="tag-retention" title={retentionTitle(retention)}>
      <span className="tag-retention-track" aria-hidden>
        <span
          className="tag-retention-fill"
          style={{
            width: `${Math.round(retention.week * 100)}%`,
            background: `var(--tone-${tone}-border)`,
          }}
        />
      </span>
      <span className="tag-retention-value">
        {formatRetention(retention.week)}
      </span>
    </span>
  );
}
