import type { TableWidths } from '../hooks/useTableWidths';
import type { SearchSort } from '../types/spares';
import { th } from '../utils';

/** The narrowest a column can be dragged to, in pixels. */
const MIN_COLUMN_WIDTH = 40;
/** How far the arrow keys move a column's edge, in pixels. */
const COLUMN_WIDTH_STEP = 20;

/** The right edge of a header, dragged or moved with the arrow keys to resize its column, and
    double-clicked to fit it to its contents again. */
function ResizeHandle({ label, width, onResize }: { label: string; width: number | undefined; onResize: (width: number | null) => void }) {
  function headerWidth(handle: HTMLElement): number {
    return handle.parentElement!.getBoundingClientRect().width;
  }

  function startDrag(e: React.PointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    // Not selecting the header's text while dragging
    e.preventDefault();
    const handle = e.currentTarget;
    const offset = headerWidth(handle) - e.clientX;
    handle.setPointerCapture(e.pointerId);
    function move(m: PointerEvent) {
      onResize(Math.max(MIN_COLUMN_WIDTH, offset + m.clientX));
    }
    function end() {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', end);
      handle.removeEventListener('pointercancel', end);
    }
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', end);
    handle.addEventListener('pointercancel', end);
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    const step = e.key === 'ArrowLeft' ? -COLUMN_WIDTH_STEP : e.key === 'ArrowRight' ? COLUMN_WIDTH_STEP : null;
    if (step === null) return;
    // Not also turning the table's page
    e.preventDefault();
    e.stopPropagation();
    onResize(Math.max(MIN_COLUMN_WIDTH, headerWidth(e.currentTarget) + step));
  }

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={`Resize the ${label.toLowerCase()} column`}
      aria-valuenow={width}
      tabIndex={0}
      title="Drag to resize the column, double-click to fit it to its contents"
      className="column-resize"
      onPointerDown={startDrag}
      onDoubleClick={() => onResize(null)}
      onKeyDown={handleKeyDown}
    />
  );
}

/** The header of `column` of a table whose column widths are `widths`, which can be resized from
    its right edge. Shows `label` unless given what to show instead. */
export function ResizableHeader({ widths, column, label, style = th, children, ...props }: React.ThHTMLAttributes<HTMLTableCellElement> & {
  widths: TableWidths;
  column: string;
  label: string;
}) {
  return (
    <th {...props} style={{ ...widths.style(column, style), position: 'relative' }}>
      {children ?? label}
      <ResizeHandle label={label} width={widths.width(column)} onResize={width => widths.resize(column, width)} />
    </th>
  );
}

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
export function ColumnHeader<Row>({ column, sort, onSort, widths }: { column: Column<Row>; sort: SearchSort | null; onSort: (field: string) => void; widths: TableWidths }) {
  const className = column.hideNarrow ? 'hide-narrow' : undefined;
  if (!column.sortField) return <ResizableHeader widths={widths} column={column.id} label={column.label} className={className} />;
  const active = sort?.field === column.sortField;
  const ariaSort = active ? (sort.descending ? 'descending' : 'ascending') : 'none';
  const next = !active ? 'ascending' : !sort.descending ? 'descending' : 'the order of the search';
  return (
    <ResizableHeader widths={widths} column={column.id} label={column.label} className={className} aria-sort={ariaSort}>
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
    </ResizableHeader>
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
