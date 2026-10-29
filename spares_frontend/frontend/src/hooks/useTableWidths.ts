import { setColumnWidth, useColumnWidths } from '../preferences';

/** The widths a table's columns were dragged to, kept in this browser, and the styles keeping its
    headers and cells at them. */
export interface TableWidths {
  /** `style`, kept at the width `column` was dragged to, if it was, cutting off or wrapping what
      doesn't fit. Given to its header and each of its cells. */
  style: (column: string, style: React.CSSProperties) => React.CSSProperties;
  width: (column: string) => number | undefined;
  /** Keeps `column` at `width` pixels, or, if null, as wide as its contents make it. */
  resize: (column: string, width: number | null) => void;
}

/** The widths of the columns of `table`, a name of its own among the app's tables. */
export function useTableWidths(table: string): TableWidths {
  const widths = useColumnWidths(table);
  return {
    style(column, style) {
      const width = widths[column];
      if (width === undefined) return style;
      // Set on both the header and the cells, as a table widens a column to fit any of them
      return { ...style, boxSizing: 'border-box', width, minWidth: width, maxWidth: width, overflow: 'hidden', textOverflow: 'ellipsis', overflowWrap: 'anywhere' };
    },
    width: column => widths[column],
    resize: (column, width) => setColumnWidth(table, column, width),
  };
}
