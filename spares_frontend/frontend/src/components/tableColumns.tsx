import type { SearchSort } from '../types/spares';
import { th } from '../utils';

/** A column a table can show, and what it is sorted by when its header is clicked. */
export interface Column<Row, Id extends string = string> {
  id: Id;
  label: string;
  /** The search field that sorts by this column, as `sort_by_asc` takes it. Unsortable without one. */
  sortField?: string;
  /** Hidden on phones, to leave room for the others */
  hideNarrow?: boolean;
  render: (row: Row) => React.ReactNode;
  tdStyle?: React.CSSProperties;
}

/** The header of `column`, which sorts the table by it when clicked, if it can be sorted. */
export function ColumnHeader<Row>({ column, sort, onSort }: { column: Column<Row>; sort: SearchSort | null; onSort: (field: string) => void }) {
  const className = column.hideNarrow ? 'hide-narrow' : undefined;
  if (!column.sortField) return <th style={th} className={className}>{column.label}</th>;
  const active = sort?.field === column.sortField;
  const ariaSort = active ? (sort.descending ? 'descending' : 'ascending') : 'none';
  const next = !active ? 'ascending' : !sort.descending ? 'descending' : 'the order of the search';
  return (
    <th style={th} className={className} aria-sort={ariaSort}>
      <button
        onClick={() => onSort(column.sortField!)}
        title={`Sort by ${column.label.toLowerCase()}, ${next}`}
        style={{ padding: 0, border: 'none', background: 'none', font: 'inherit', fontWeight: 'inherit', color: 'inherit', cursor: 'pointer', whiteSpace: 'nowrap' }}
      >
        {column.label}
        <span aria-hidden style={{ display: 'inline-block', width: 14, marginLeft: 2, fontSize: 10, color: 'var(--text-muted)' }}>
          {active ? (sort.descending ? '▼' : '▲') : ''}
        </span>
      </button>
    </th>
  );
}

/** A menu of checkboxes choosing which of `columns` a table shows. The last one can't be hidden. */
export function ColumnPicker<Id extends string>({ columns, shown, onChange }: {
  columns: readonly { id: Id; label: string }[];
  shown: readonly Id[];
  onChange: (shown: Id[]) => void;
}) {
  function toggle(id: Id) {
    const next = shown.includes(id) ? shown.filter(c => c !== id) : [...shown, id];
    onChange(columns.map(c => c.id).filter(c => next.includes(c)));
  }
  return (
    <details style={{ position: 'relative', fontSize: 13 }}>
      <summary style={{ cursor: 'pointer', listStyle: 'none' }} className="touch-target-small" title="Choose the columns the table shows">
        Columns ▾
      </summary>
      <div style={{
        position: 'absolute', right: 0, zIndex: 10, marginTop: 4, padding: '6px 10px', minWidth: 160,
        background: 'var(--surface)', border: '1px solid var(--border)', borderRadius: 6, boxShadow: '0 2px 8px rgba(0, 0, 0, 0.15)',
      }}>
        {columns.map(column => {
          const checked = shown.includes(column.id);
          return (
            <label key={column.id} style={{ display: 'flex', gap: 6, alignItems: 'center', padding: '3px 0', whiteSpace: 'nowrap' }}>
              <input type="checkbox" checked={checked} disabled={checked && shown.length === 1} onChange={() => toggle(column.id)} />
              {column.label}
            </label>
          );
        })}
      </div>
    </details>
  );
}
