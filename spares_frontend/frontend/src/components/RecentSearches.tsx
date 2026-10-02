const chipButton: React.CSSProperties = { padding: '6px 10px', fontSize: 13 };

interface RecentSearchesProps<T> {
  recent: T[];
  label?: (item: T) => string;
  onSelect: (item: T) => void;
  onRemove: (item: T) => void;
  selectTitle?: string;
  style?: React.CSSProperties;
}

/** A row of recent searches to run again, each removable. Renders nothing when there are none. */
export default function RecentSearches<T>({
  recent, label = String, onSelect, onRemove, selectTitle = 'Search this again', style,
}: RecentSearchesProps<T>) {
  if (recent.length === 0) return null;
  return (
    <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', alignItems: 'center', marginBottom: 8, fontSize: 13, ...style }}>
      <span style={{ color: 'var(--text-muted)' }}>Recent:</span>
      {recent.map(item => (
        <span key={label(item)} style={{ display: 'inline-flex' }}>
          <button onClick={() => onSelect(item)} title={selectTitle} style={chipButton}>{label(item)}</button>
          <button onClick={() => onRemove(item)} title="Remove from recent" aria-label="Remove from recent" style={{ ...chipButton, color: 'var(--text-faint)' }}>×</button>
        </span>
      ))}
    </div>
  );
}
