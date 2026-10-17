import HelpDialog from './HelpDialog';
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
  return (
    <HelpDialog
      title="Search syntax"
      wide
      intro={<>
        Words are searched for in the note's text. <code>-</code> or <code>not</code> excludes, and Tab completes
        fields and tags. Click a query to put it in the search box.
      </>}
      onClose={onClose}
    >
      {close => {
        const row = (query: string, about: string) => (
          <tr key={query}>
            <td><button type="button" onClick={() => { close(); onPick(query); }} title="Put this in the search box"><code>{query}</code></button></td>
            <td>{about}</td>
          </tr>
        );
        // One table, so the examples and the fields line up
        return (
          <table>
            <tbody>
              {EXAMPLES.map(([query, about]) => row(query, about))}
              {fields.length > 0 && (
                <tr><th colSpan={2} style={{ textAlign: 'left', padding: '16px 8px 4px' }}>Fields</th></tr>
              )}
              {fields.map(f => row(f.example, f.about))}
            </tbody>
          </table>
        );
      }}
    </HelpDialog>
  );
}
