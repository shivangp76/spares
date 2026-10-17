import { useEffect, useId, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { getSearchFields, listTagNames } from '../api/client';
import SearchHelp from './SearchHelp';
import { applyCompletion, searchCompletions, type Completion } from '../searchCompletion';
import type { SearchField } from '../types/spares';

/** The styles that decide where the input's text is drawn, copied onto the underline. */
const TEXT_STYLES = [
  'fontFamily', 'fontSize', 'fontWeight', 'fontStyle', 'letterSpacing', 'lineHeight', 'textIndent',
  'paddingLeft', 'paddingRight', 'paddingTop', 'paddingBottom',
  'borderLeftWidth', 'borderRightWidth', 'borderTopWidth', 'borderBottomWidth',
] as const;

/** A failed search: its query, and where in it the server says the problem is. */
export interface SearchInputError {
  query: string;
  span: [number, number];
}

interface SearchInputProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: (value: string) => void;
  placeholder?: string;
  /** Underlined while the box still holds the query that failed. */
  error?: SearchInputError | null;
  ref?: React.Ref<HTMLInputElement>;
  /** Merged into the wrapper's and the input's styles. */
  style?: React.CSSProperties;
  inputStyle?: React.CSSProperties;
}

/**
 * A search box that completes field names, tags, card states and sort fields, underlines where a
 * failed query went wrong, and has a "Syntax" link in it that opens examples and the fields.
 */
export default function SearchInput({ value, onChange, onSubmit, placeholder, error, ref, style, inputStyle }: SearchInputProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  useImperativeHandle(ref, () => inputRef.current!, []);
  const underlineRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [fields, setFields] = useState<SearchField[]>([]);
  const [tags, setTags] = useState<string[]>([]);
  const [caret, setCaret] = useState(0);
  const [focused, setFocused] = useState(false);
  // Hidden by Escape, accepting one or searching, until the query is edited again
  const [dismissed, setDismissed] = useState(false);
  // The highlighted suggestion, or -1 before the arrow keys pick one
  const [active, setActive] = useState(-1);
  const [showHelp, setShowHelp] = useState(false);
  // Where the caret goes once an accepted suggestion is in the box
  const pendingCaret = useRef<number | null>(null);

  useEffect(() => {
    // Suggestions are a convenience, so offline there are just none
    getSearchFields().then(setFields).catch(() => {});
    listTagNames().then(setTags).catch(() => {});
  }, []);

  const completions = useMemo(
    () => (focused && !dismissed ? searchCompletions(value, caret, fields, tags) : null),
    [focused, dismissed, value, caret, fields, tags],
  );

  useLayoutEffect(() => {
    const input = inputRef.current;
    if (input && pendingCaret.current !== null) {
      input.setSelectionRange(pendingCaret.current, pendingCaret.current);
      setCaret(pendingCaret.current);
      pendingCaret.current = null;
    }
  }, [value]);

  // The underline is drawn by a copy of the text over the input, so it needs the input's text
  // styles and scroll position
  const underlined = error && error.query === value ? error.span : null;
  useLayoutEffect(() => {
    const input = inputRef.current;
    const underline = underlineRef.current;
    if (!input || !underline) return;
    const style = getComputedStyle(input);
    for (const property of TEXT_STYLES) underline.style[property] = style[property];
    underline.style.left = `${input.offsetLeft}px`;
    underline.style.top = `${input.offsetTop}px`;
    underline.style.width = `${input.offsetWidth}px`;
    underline.style.height = `${input.offsetHeight}px`;
    underline.scrollLeft = input.scrollLeft;
  });

  function syncCaret() {
    const input = inputRef.current;
    if (input) setCaret(input.selectionStart ?? input.value.length);
    if (input && underlineRef.current) underlineRef.current.scrollLeft = input.scrollLeft;
  }

  function accept(completion: Completion) {
    if (!completions) return;
    const applied = applyCompletion(value, completions, completion);
    pendingCaret.current = applied.caret;
    setDismissed(true);
    setActive(-1);
    onChange(applied.query);
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    const count = completions?.items.length ?? 0;
    if (count > 0 && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault();
      setActive(i => (e.key === 'ArrowDown' ? (i + 1) % count : (i <= 0 ? count - 1 : i - 1)));
    } else if (count > 0 && e.key === 'Tab' && !e.shiftKey) {
      e.preventDefault();
      accept(completions!.items[Math.max(active, 0)]);
    } else if (e.key === 'Enter') {
      if (count > 0 && active >= 0) {
        e.preventDefault();
        accept(completions!.items[active]);
      } else {
        setDismissed(true);
        onSubmit(value);
      }
    } else if (e.key === 'Escape') {
      setDismissed(true);
    }
  }

  function pickExample(example: string) {
    pendingCaret.current = example.length;
    onChange(example);
    setDismissed(true);
    inputRef.current?.focus();
  }

  const open = completions !== null && completions.items.length > 0;
  return (
    <div className="search-input" style={{ position: 'relative', display: 'flex', flex: 1, maxWidth: 400, ...style }}>
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={e => {
          onChange(e.target.value);
          setCaret(e.target.selectionStart ?? e.target.value.length);
          setDismissed(false);
          setActive(-1);
        }}
        onKeyDown={handleKeyDown}
        onSelect={syncCaret}
        onScroll={syncCaret}
        onFocus={() => setFocused(true)}
        onBlur={() => { setFocused(false); setActive(-1); }}
        placeholder={placeholder}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        role="combobox"
        aria-expanded={open}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
        aria-invalid={underlined !== null}
        // The right padding leaves room for the Syntax link
        style={{ padding: '6px 10px', fontSize: 14, flex: 1, minWidth: 0, ...inputStyle, paddingRight: 64 }}
      />
      {underlined && (
        <div ref={underlineRef} aria-hidden="true" style={underlineStyle}>
          {value.slice(0, underlined[0])}
          <span style={markStyle}>{value.slice(underlined[0], Math.max(underlined[1], underlined[0] + 1)) || ' '}</span>
          {value.slice(Math.max(underlined[1], underlined[0] + 1))}
        </div>
      )}
      <button
        type="button"
        className="search-syntax-link"
        onClick={() => { setDismissed(true); setShowHelp(true); }}
        aria-haspopup="dialog"
        title="Example queries and the fields"
      >Syntax</button>

      {open && (
        <ul id={listId} role="listbox" style={popupStyle}>
          {completions.items.map((item, i) => (
            <li
              key={item.label}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              // Keeps the focus in the input
              onMouseDown={e => { e.preventDefault(); accept(item); }}
              onMouseEnter={() => setActive(i)}
              style={{ padding: '6px 10px', cursor: 'pointer', background: i === active ? 'var(--selected)' : undefined }}
            >
              <code>{item.label}</code>
              {item.detail && <span style={{ color: 'var(--text-muted)', marginLeft: 8, fontSize: 12 }}>{item.detail}</span>}
            </li>
          ))}
        </ul>
      )}

      {showHelp && <SearchHelp fields={fields} onPick={pickExample} onClose={() => setShowHelp(false)} />}
    </div>
  );
}

const popupStyle: React.CSSProperties = {
  position: 'absolute',
  top: '100%',
  left: 0,
  right: 0,
  zIndex: 20,
  marginTop: 2,
  padding: 0,
  listStyle: 'none',
  background: 'var(--bg)',
  border: '1px solid var(--border-strong)',
  borderRadius: 4,
  boxShadow: '0 4px 12px rgb(0 0 0 / 0.15)',
  fontSize: 13,
  maxHeight: 280,
  overflowY: 'auto',
};

const underlineStyle: React.CSSProperties = {
  position: 'absolute',
  pointerEvents: 'none',
  boxSizing: 'border-box',
  borderStyle: 'solid',
  borderColor: 'transparent',
  whiteSpace: 'pre',
  overflow: 'hidden',
  color: 'transparent',
  // An input centers its line of text
  display: 'flex',
  alignItems: 'center',
};

const markStyle: React.CSSProperties = {
  textDecorationLine: 'underline',
  textDecorationStyle: 'wavy',
  textDecorationColor: 'var(--error)',
  textDecorationSkipInk: 'none',
  textDecorationThickness: 2,
  textUnderlineOffset: 3,
};
