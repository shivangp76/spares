/**
 * Dictionary lookup on the cards of the `markdown-language-tools` parser, which the Review page
 * loads (with `import()`) only for them. It listens on the cards' documents (`HtmlViewer`'s
 * `onDocument`):
 *
 * - `K` (Vim's keyword lookup) looks up the selection, or the word under the pointer.
 * - On touchscreens, the Look up words toggle makes taps on text look words up. Tapping elsewhere
 *   still shows the answer. It also shows where the card's document can't be listened to (WebKit
 *   runs no listeners in a sandboxed document, even the app's), so a click looks words up there.
 *   Taps go to the app's window instead of the card's iframe while it is on.
 * - The command palette's Look up word… asks for a word, which works on cards shown as PDFs too.
 *
 * The popup lists the entries of the user's dictionaries on the host, with `j`/`k` between them,
 * Enter adding a card for the one picked (quick capture, with this parser, the word and its reading
 * in a cloze, its glosses and the note's language), and Esc closing it.
 */
import { useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { getNote } from '../api/client';
import { usePageCommands } from '../commandPalette';
import { useMediaQuery } from '../hooks/useMediaQuery';
import { captureNote } from '../quickCapture';
import type { SpeechItem } from '../types/spares';
import { errorMessage } from '../utils';
import { lookUp, type DictionaryEntry, type LookupResponse } from './api';
import { highlight, rangesOf, rectOf, textAtPoint, type TextAt } from './caret';
import { vocabularyNote } from './text';

const PARSER = 'markdown-language-tools';

export interface LookupHandle {
  /** Looks up the selection, or the word under the pointer. False if there is neither. */
  lookUp: () => boolean;
}

interface Shown {
  /** What was looked up */
  text: string;
  response: LookupResponse | null;
  error: string | null;
  /** Where the word is in the app's viewport, if on a card */
  rect: DOMRect | null;
}

export default function LookupLayer({
  ref,
  docs,
  noteId,
  tags,
  onOpenNote,
  onSpeak,
}: {
  ref?: React.Ref<LookupHandle>;
  /** The documents of the card's sides that are shown as HTML */
  docs: (Document | null | undefined)[];
  noteId: number;
  /** The card's note's tags, which a card added from the popup gets too */
  tags: string[];
  onOpenNote: (noteId: number) => void;
  onSpeak: (item: SpeechItem) => void;
}) {
  const touch = useMediaQuery('(hover: none)');
  const isNarrow = useMediaQuery('(max-width: 640px)');
  const [tapToLookUp, setTapToLookUp] = useState(false);
  // Whether listeners added to the cards' documents run, which WebKit doesn't let them
  const [listening, setListening] = useState(true);
  const [shown, setShown] = useState<Shown | null>(null);
  const [active, setActive] = useState(0);
  const [asking, setAsking] = useState(false);
  const pointer = useRef<{ doc: Document; x: number; y: number } | null>(null);
  const highlighted = useRef<Document | null>(null);
  const language = useRef<{ noteId: number; language: Promise<string | null> } | null>(null);
  const popupRef = useRef<HTMLDivElement>(null);

  /** The note's `language`, fetched once per note. */
  const noteLanguage = useCallback((): Promise<string | null> => {
    if (language.current?.noteId !== noteId) {
      language.current = {
        noteId,
        language: getNote(noteId).then(
          note => (typeof note.custom_data.language === 'string' ? note.custom_data.language : null),
          () => null,
        ),
      };
    }
    return language.current.language;
  }, [noteId]);

  const clearHighlight = useCallback(() => {
    if (highlighted.current) highlight(highlighted.current, []);
    highlighted.current = null;
  }, []);

  const close = useCallback(() => {
    clearHighlight();
    setShown(null);
    setAsking(false);
  }, [clearHighlight]);

  /** Looks up `text`, from `at` in `doc` if it is on a card, to highlight what matched. */
  const show = useCallback(
    async (text: string, doc?: Document, at?: TextAt, selection?: Range) => {
      clearHighlight();
      // At the word from the start, so a failed lookup shows there too
      const rect = doc && selection ? rectOf(doc, [selection]) : doc && at ? rectOf(doc, rangesOf(doc, at, 1)) : null;
      setShown({ text, response: null, error: null, rect });
      setActive(0);
      try {
        const response = await lookUp(text, await noteLanguage());
        let wordRect = rect;
        if (doc && at && response.matched > 0) {
          const ranges = rangesOf(doc, at, response.matched);
          highlight(doc, ranges);
          highlighted.current = doc;
          wordRect = rectOf(doc, ranges);
        } else if (doc && selection) {
          highlight(doc, [selection]);
          highlighted.current = doc;
        }
        setShown(current => (current?.text === text ? { text, response, error: null, rect: wordRect } : current));
      } catch (e) {
        setShown(current => (current?.text === text ? { ...current, error: errorMessage(e) } : current));
      }
    },
    [clearHighlight, noteLanguage],
  );

  /** Looks up at (x, y) in `doc`. */
  const lookUpAt = useCallback(
    (doc: Document, x: number, y: number): boolean => {
      const at = textAtPoint(doc, x, y);
      if (!at) return false;
      void show(at.text, doc, at);
      return true;
    },
    [show],
  );

  const lookUpNow = useCallback((): boolean => {
    for (const doc of docs) {
      const selection = doc?.getSelection();
      const text = selection?.toString().trim();
      if (doc && selection && text && selection.rangeCount > 0) {
        void show(text, doc, undefined, selection.getRangeAt(0));
        return true;
      }
    }
    const at = pointer.current;
    if (at && docs.includes(at.doc)) return lookUpAt(at.doc, at.x, at.y);
    return false;
  }, [docs, lookUpAt, show]);

  useImperativeHandle(ref, () => ({ lookUp: lookUpNow }), [lookUpNow]);

  usePageCommands([
    {
      id: 'language-tools-look-up',
      label: 'Look up word…',
      hint: 'K',
      run: () => {
        clearHighlight();
        setShown(null);
        setAsking(true);
      },
    },
  ]);

  // The pointer and keys in the cards' documents, which the app's window doesn't get
  useEffect(() => {
    const shown = docs.filter((doc): doc is Document => !!doc);
    const removers = shown.map(doc => {
      const move = (e: MouseEvent) => {
        pointer.current = { doc, x: e.clientX, y: e.clientY };
      };
      const key = (e: KeyboardEvent) => {
        if (e.key === 'K' && !e.ctrlKey && !e.metaKey && !e.altKey && lookUpNow()) e.preventDefault();
      };
      doc.addEventListener('mousemove', move);
      doc.addEventListener('keydown', key);
      return () => {
        doc.removeEventListener('mousemove', move);
        doc.removeEventListener('keydown', key);
      };
    });
    // A listener that doesn't run on an event dispatched here
    const probe = shown[0];
    if (probe) {
      let ran = false;
      const listener = () => {
        ran = true;
      };
      probe.addEventListener('spares-probe', listener);
      probe.dispatchEvent(new Event('spares-probe'));
      probe.removeEventListener('spares-probe', listener);
      queueMicrotask(() => setListening(ran));
    }
    return () => removers.forEach(remove => remove());
  }, [docs, lookUpNow]);

  // With the toggle on, the cards' iframes let taps through to the app's window, which looks up
  // the text under them, or else lets them show the answer as usual
  useEffect(() => {
    if (!tapToLookUp) return;
    const frames = docs
      .filter((doc): doc is Document => !!doc)
      .map(doc => ({ doc, frame: doc.defaultView?.frameElement as HTMLElement | null | undefined }))
      .filter((f): f is { doc: Document; frame: HTMLElement } => !!f.frame);
    const previous = frames.map(({ frame }) => frame.style.pointerEvents);
    frames.forEach(({ frame }) => (frame.style.pointerEvents = 'none'));
    const under = (e: MouseEvent) => {
      for (const { doc, frame } of frames) {
        const rect = frame.getBoundingClientRect();
        if (rect.height > 0 && e.clientX >= rect.left && e.clientX <= rect.right && e.clientY >= rect.top && e.clientY <= rect.bottom)
          return { doc, x: e.clientX - rect.left, y: e.clientY - rect.top };
      }
      return null;
    };
    const click = (e: MouseEvent) => {
      const at = under(e);
      if (at && lookUpAt(at.doc, at.x, at.y)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    const move = (e: MouseEvent) => {
      const at = under(e);
      if (at) pointer.current = at;
    };
    window.addEventListener('click', click, { capture: true });
    window.addEventListener('mousemove', move);
    return () => {
      frames.forEach(({ frame }, i) => (frame.style.pointerEvents = previous[i]));
      window.removeEventListener('click', click, { capture: true });
      window.removeEventListener('mousemove', move);
    };
  }, [docs, tapToLookUp, lookUpAt]);

  // A new card closes the popup
  useEffect(() => close, [noteId, close]);

  const entries = shown?.response?.entries ?? [];
  const picked = entries[Math.min(active, entries.length - 1)];

  const addCard = useCallback(
    async (entry: DictionaryEntry) => {
      const language = (await noteLanguage()) ?? shown?.response?.language ?? null;
      close();
      captureNote({
        parserName: PARSER,
        data: vocabularyNote(entry),
        keywords: [entry.expression],
        tags: ['vocab', ...tags.filter(tag => tag !== 'vocab')],
        customData: language ? { language } : undefined,
      });
    },
    [close, noteLanguage, shown, tags],
  );

  const speak = useCallback(
    (entry: DictionaryEntry) => {
      onSpeak({ text: entry.expression, lang: shown?.response?.language ?? undefined });
    },
    [onSpeak, shown],
  );

  // j/k, Enter and Esc in the popup, before the Review page's keys
  useEffect(() => {
    if (!shown) return;
    function onKey(e: KeyboardEvent) {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target as Element | null;
      if (target?.closest?.('input, textarea, .cm-editor')) return;
      const handled: Record<string, () => void> = {
        j: () => setActive(i => Math.min(i + 1, Math.max(entries.length - 1, 0))),
        k: () => setActive(i => Math.max(i - 1, 0)),
        Escape: close,
        Enter: () => {
          if (picked) void addCard(picked);
        },
      };
      const action = handled[e.key];
      if (!action) return;
      e.preventDefault();
      e.stopPropagation();
      action();
    }
    window.addEventListener('keydown', onKey, { capture: true });
    return () => window.removeEventListener('keydown', onKey, { capture: true });
  }, [shown, entries.length, picked, addCard, close]);

  // The picked entry stays in view
  useEffect(() => {
    popupRef.current?.querySelector(`[data-entry="${active}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [active]);

  return (
    <>
      {(touch || !listening) && (
        <div style={{ marginBottom: 8 }}>
          <button
            onClick={() => setTapToLookUp(on => !on)}
            aria-pressed={tapToLookUp}
            className="touch-target-small"
            title={`${touch ? 'Taps' : 'Clicks'} on the card's text look words up in your dictionaries (K looks up the selection)`}
            style={tapToLookUp ? toggledOn : undefined}
          >
            Look up words
          </button>
        </div>
      )}
      {asking && <AskWord onLookUp={text => void show(text)} onClose={() => setAsking(false)} />}
      {shown && (
        <div
          ref={popupRef}
          role="dialog"
          aria-label={`Dictionary: ${shown.text}`}
          className="language-tools-popup"
          style={popupStyle(shown.rect, isNarrow)}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
            <strong style={{ flex: 1, fontSize: 'var(--font-sm)', color: 'var(--text-secondary)' }}>
              {shown.response && shown.response.matched > 0 ? [...shown.text].slice(0, shown.response.matched).join('') : shown.text}
            </strong>
            <button onClick={close} aria-label="Close" title="Close (Esc)" style={smallButton}>
              ×
            </button>
          </div>
          {shown.error && <div className="error-text">{shown.error}</div>}
          {!shown.error && !shown.response && <div className="secondary-text">Looking up…</div>}
          {shown.response && entries.length === 0 && (
            <div className="secondary-text">
              No entries.{' '}
              {shown.response.language
                ? `Import a dictionary for ${shown.response.language} with spares language-tools dictionary import.`
                : 'Dictionaries are imported with spares language-tools dictionary import.'}
            </div>
          )}
          <ol style={{ listStyle: 'none', margin: 0, padding: 0 }}>
            {entries.map((entry, i) => (
              <li
                key={i}
                data-entry={i}
                onClick={() => setActive(i)}
                style={{
                  padding: '6px 8px',
                  borderRadius: 'var(--radius-sm)',
                  background: i === active ? 'var(--selected)' : undefined,
                }}
              >
                <EntryView
                  entry={entry}
                  onOpenNote={id => {
                    close();
                    onOpenNote(id);
                  }}
                />
                {i === active && (
                  <div style={{ display: 'flex', gap: 6, marginTop: 6, flexWrap: 'wrap' }}>
                    <button onClick={() => void addCard(entry)} style={smallButton}>
                      Add card <span className="key-hint">(Enter)</span>
                    </button>
                    <button onClick={() => speak(entry)} style={smallButton}>
                      Speak
                    </button>
                    <button
                      onClick={() => void navigator.clipboard?.writeText(entry.expression)}
                      style={smallButton}
                    >
                      Copy
                    </button>
                  </div>
                )}
              </li>
            ))}
          </ol>
          {entries.length > 1 && (
            <div className="secondary-text" style={{ fontSize: 'var(--font-xs)', marginTop: 4 }}>
              j/k between entries · Enter adds a card · Esc closes
            </div>
          )}
        </div>
      )}
    </>
  );
}

function EntryView({ entry, onOpenNote }: { entry: DictionaryEntry; onOpenNote: (id: number) => void }) {
  const reading = entry.reading && entry.reading !== entry.expression ? entry.reading : null;
  return (
    <div>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 8, flexWrap: 'wrap' }}>
        <span style={{ fontSize: '1.25em' }}>
          {entry.expression}
        </span>
        {reading && <span style={{ color: 'var(--text-secondary)' }}>{reading}</span>}
        {entry.tags && (
          <span style={{ fontSize: 'var(--font-xs)', color: 'var(--text-muted)' }}>{entry.tags}</span>
        )}
      </div>
      {entry.inflections.length > 0 && (
        <div style={{ fontSize: 'var(--font-xs)', color: 'var(--text-muted)' }}>
          {entry.source} « {entry.inflections.join(' « ')}
        </div>
      )}
      <ol style={{ margin: '4px 0 0', paddingLeft: entry.glosses.length > 1 ? 20 : 0, listStyle: entry.glosses.length > 1 ? 'decimal' : 'none' }}>
        {entry.glosses.map((gloss, i) => (
          <li key={i} style={{ whiteSpace: 'pre-line' }}>
            {gloss}
          </li>
        ))}
      </ol>
      {entry.notes && entry.notes.length > 0 && (
        <button onClick={() => onOpenNote(entry.notes![0])} style={{ ...smallButton, marginTop: 4 }}>
          Already in your notes
        </button>
      )}
      <div style={{ fontSize: 'var(--font-xs)', color: 'var(--text-faint)', marginTop: 2 }}>
        {entry.dictionary}
        {entry.attribution ? ` · ${entry.attribution}` : ''}
      </div>
    </div>
  );
}

/** Asks for a word to look up, for the command palette's Look up word…. */
function AskWord({ onLookUp, onClose }: { onLookUp: (text: string) => void; onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [text, setText] = useState('');
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);
  return (
    <dialog ref={dialogRef} aria-label="Look up word" onClose={onClose} className="audio-recorder">
      <form
        method="dialog"
        onSubmit={() => {
          if (text.trim()) onLookUp(text.trim());
        }}
        style={{ display: 'flex', gap: 8, padding: 12 }}
      >
        <input autoFocus value={text} onChange={e => setText(e.target.value)} placeholder="A word to look up" />
        <button type="submit">Look up</button>
      </form>
    </dialog>
  );
}

const smallButton: React.CSSProperties = { padding: '2px 8px', fontSize: 'var(--font-sm)' };

const toggledOn: React.CSSProperties = {
  background: 'var(--tone-blue-bg)',
  borderColor: 'var(--tone-blue-border)',
  color: 'var(--tone-blue-fg)',
};

/** Below the word, or above it near the bottom of the screen; a sheet at the bottom on phones. */
function popupStyle(rect: DOMRect | null, isNarrow: boolean): React.CSSProperties {
  const base: React.CSSProperties = {
    position: 'fixed',
    zIndex: 50,
    background: 'var(--surface)',
    color: 'var(--text)',
    border: '1px solid var(--border)',
    borderRadius: 'var(--radius-md)',
    boxShadow: 'var(--shadow-popover)',
    padding: 10,
    overflowY: 'auto',
  };
  const width = 380;
  if (isNarrow) return { ...base, left: 8, right: 8, bottom: 8, maxHeight: '45vh' };
  // A word asked for, not on the card
  if (!rect) return { ...base, top: 80, left: `calc(50% - ${width / 2}px)`, width, maxHeight: '60vh' };
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
  const below = rect.bottom + 6;
  const roomBelow = window.innerHeight - below - 8;
  if (roomBelow >= 220) return { ...base, left, top: below, width, maxHeight: roomBelow };
  return { ...base, left, bottom: window.innerHeight - rect.top + 6, width, maxHeight: rect.top - 14 };
}
