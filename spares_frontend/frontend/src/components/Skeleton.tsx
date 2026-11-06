/**
 * Grey shapes standing in for what is loading, so the page doesn't jump when it comes, as a card
 * (`block`) or rows of text (`lines`). Like `loading-delayed`, it appears only after a moment, so
 * a quick load doesn't flash it.
 */
export default function Skeleton({
  lines = 3,
  block,
  style,
}: {
  /** How many lines of text, the last one shorter */
  lines?: number;
  /** The height of a block above the lines, as a card's */
  block?: number;
  style?: React.CSSProperties;
}) {
  return (
    <div
      className="skeleton loading-delayed"
      role="status"
      aria-label="Loading"
      style={style}
    >
      {block !== undefined && (
        <div
          className="skeleton-line skeleton-block"
          style={{ height: block }}
        />
      )}
      {Array.from({ length: lines }, (_, i) => (
        <div
          key={i}
          className="skeleton-line"
          style={{ width: i === lines - 1 && lines > 1 ? "60%" : "100%" }}
        />
      ))}
    </div>
  );
}
