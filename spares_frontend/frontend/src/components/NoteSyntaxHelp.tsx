import { useState } from 'react';
import { useParserSyntax } from '../hooks/useParserSyntax';
import CodeText from './CodeText';
import HelpDialog from './HelpDialog';

/** A "Syntax" button opening the parser's examples, each of which `onInsert` puts in the note. */
export default function NoteSyntaxButton({ parserId, onInsert, tips }: {
  parserId: number | null;
  onInsert: (source: string) => void;
  /** About the editor itself, shown under the intro. */
  tips?: string[];
}) {
  const [open, setOpen] = useState(false);
  const { examples, error } = useParserSyntax(open ? parserId : null);
  return (
    <>
      <button
        onClick={() => setOpen(true)}
        disabled={parserId === null}
        className="button-tool"
        title={parserId === null ? 'Select a parser first' : 'How to write clozes and the rest of a note, with the cards each makes'}
        style={{ padding: '2px 8px', fontSize: 12 }}
      >
        Syntax
      </button>
      {open && (
        <HelpDialog
          title="Note syntax"
          wide
          intro={<>
            Each example is followed by the cards it makes, with what each asks for after the arrow.
            Click Insert to put an example at the cursor.
            {tips && tips.length > 0 && <ul style={{ margin: '6px 0 0', paddingLeft: 18 }}>{tips.map(t => <li key={t}>{t}</li>)}</ul>}
          </>}
          onClose={() => setOpen(false)}
        >
          {close => (
            <>
              {error && <div style={{ color: 'var(--error)', fontSize: 13 }}>{error}</div>}
              {!examples && !error && <div style={{ fontSize: 13 }}>Loading…</div>}
              {examples?.map(example => (
                <section key={example.title} className="syntax-example">
                  <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
                    <strong style={{ fontSize: 14 }}>{example.title}</strong>
                    <button type="button" onClick={() => { close(); onInsert(example.source); }} style={{ padding: '2px 8px', fontSize: 12 }}>
                      Insert
                    </button>
                  </div>
                  <div style={{ fontSize: 13, margin: '2px 0 6px' }}><CodeText text={example.about} /></div>
                  <pre>{example.source}</pre>
                  <ol className="syntax-example-cards">
                    {example.cards.map((card, i) => (
                      <li key={i}>
                        <span className="syntax-example-front">{card.front}</span>
                        {card.answers.length > 0 && <span className="syntax-example-answer"> → {card.answers.join(', ')}</span>}
                      </li>
                    ))}
                  </ol>
                </section>
              ))}
            </>
          )}
        </HelpDialog>
      )}
    </>
  );
}
