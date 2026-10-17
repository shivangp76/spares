const chipButton: React.CSSProperties = { padding: '6px 10px', fontSize: 13 };

interface RecentSearchesProps<T> {
  recent: T[];
  /** What the row is called, like "Recent". */
  heading?: string;
  label?: (item: T) => string;
  /** The tooltip of an item's button, `selectTitle` if it has none. */
  itemTitle?: (item: T) => string;
  onSelect: (item: T) => void;
  onRemove: (item: T) => void;
  selectTitle?: string;
  style?: React.CSSProperties;
}

/** A row of recent searches to run again, each removable. Renders nothing when there are none. */
export default function RecentSearches<T>({
  recent, heading = 'Recent', label = String, itemTitle, onSelect, onRemove, selectTitle = 'Search this again', style,
}: RecentSearchesProps<T>) {
  if (recent.length === 0) return null;
  return (
    <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', alignItems: 'center', marginBottom: 8, fontSize: 13, ...style }}>
      <span style={{ color: 'var(--text-muted)' }}>{heading}:</span>
      {recent.map(item => (
        <span key={label(item)} style={{ display: 'inline-flex' }}>
          <button onClick={() => onSelect(item)} title={itemTitle?.(item) ?? selectTitle} style={chipButton}>{label(item)}</button>
          <button onClick={() => onRemove(item)} title={`Remove from ${heading.toLowerCase()}`} aria-label={`Remove ${label(item)} from ${heading.toLowerCase()}`} style={{ ...chipButton, color: 'var(--text-faint)' }}>×</button>
        </span>
      ))}
    </div>
  );
}
