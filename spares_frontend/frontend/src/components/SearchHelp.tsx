import { useEffect, useRef } from 'react';
import type { SearchField } from '../types/spares';

/** Queries shown by the search help, each with what it finds. */
const EXAMPLES: [string, string][] = [
  ['verb', 'a word starting with "verb", like "verbs"'],
  ['"past tense"', 'the text "past tense" anywhere'],
  ['Paris or Rome', 'either word'],
  ['tag=spanish -tag=spanish:verbs', 'tagged spanish, but not spanish:verbs'],
  ['c.due<=today', 'cards due by the end of today'],
  ['c.state=new created_at>=today-7d', 'new cards of notes created this week'],
  ['c.rated[>=today-7d]=1', 'cards forgotten this week'],
  ['sort_by_asc=c.retrievability limit=20', 'the 20 cards you are least likely to recall'],
  ['linked_to_note=(tag=history)', 'notes linking to a note tagged history'],
  ['(tag=a limit=5) or (tag=b limit=5)', 'up to 5 of each tag'],
];

/** A popup with example queries and the fields, either of which `onPick` puts in the search box. */
export default function SearchHelp({ fields, onPick, onClose }: {
  fields: SearchField[];
  onPick: (query: string) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    // Not closed on unmount, since its close event would fire `onClose` after StrictMode remounts it
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  function pick(query: string) {
    dialogRef.current?.close();
    onPick(query);
  }

  const row = (query: string, about: string) => (
    <tr key={query}>
      <td><button type="button" onClick={() => pick(query)} title="Put this in the search box"><code>{query}</code></button></td>
      <td>{about}</td>
    </tr>
  );

  return (
    <dialog
      ref={dialogRef}
      className="keybindings-help search-help"
      aria-labelledby="search-help-title"
      onClose={onClose}
      // The dialog itself is only hit by a click on the backdrop, as its content fills it
      onClick={e => { if (e.target === e.currentTarget) dialogRef.current?.close(); }}
    >
      <div className="keybindings-help-content">
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 16 }}>
          <h3 id="search-help-title" style={{ margin: 0 }}>Search syntax</h3>
          <button type="button" onClick={() => dialogRef.current?.close()}>Close <span className="key-hint">(Esc)</span></button>
        </div>
        <p style={{ fontSize: 13, color: 'var(--text-secondary)', margin: '8px 0 12px' }}>
          Words are searched for in the note's text. <code>-</code> or <code>not</code> excludes, and Tab completes
          fields and tags. Click a query to put it in the search box.
        </p>
        {/* One table, so the examples and the fields line up */}
        <table>
          <tbody>
            {EXAMPLES.map(([query, about]) => row(query, about))}
            {fields.length > 0 && (
              <tr><th colSpan={2} style={{ textAlign: 'left', padding: '16px 8px 4px' }}>Fields</th></tr>
            )}
            {fields.map(f => row(f.example, f.about))}
          </tbody>
        </table>
      </div>
    </dialog>
  );
}
