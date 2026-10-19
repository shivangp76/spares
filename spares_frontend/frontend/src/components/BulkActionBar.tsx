import { useEffect, useId, useRef, useState } from 'react';
import { resolveSelection, selectedCount, type IdsOrQuery, type RowSelection } from '../selection';
import { errorMessage } from '../utils';
import ActionResult, { type ActionOutcome } from './ActionResult';

/** What an action asks for before it runs. */
export type BulkInput =
  | { kind: 'tags'; suggestions: () => Promise<string[]> }
  | { kind: 'datetime' }
  | { kind: 'number'; placeholder: string; min: number; max: number; step: number };

export interface BulkAction {
  label: string;
  /** Shown in red, as for deleting. */
  danger?: boolean;
  input?: BulkInput;
  /** The question confirmed before running, given how many rows it acts on and what was asked for. */
  confirm: (count: number, value: string[]) => string;
  /**
   * Runs the action on `selector`, which has `count` rows. `value` holds the tags asked for, or the
   * one value typed.
   */
  run: (selector: IdsOrQuery, value: string[], count: number) => Promise<ActionOutcome>;
}

const input: React.CSSProperties = { padding: '4px 8px', fontSize: 13, border: '1px solid var(--border-strong)', borderRadius: 4 };
const small: React.CSSProperties = { padding: '2px 8px', fontSize: 12 };

/** A checkbox that can show that only some of what it stands for is checked. */
export function Checkbox({ checked, indeterminate = false, onClick, label }: {
  checked: boolean;
  indeterminate?: boolean;
  onClick: (e: React.MouseEvent) => void;
  label: string;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => { if (ref.current) ref.current.indeterminate = indeterminate; }, [indeterminate]);
  return (
    <input
      ref={ref}
      type="checkbox"
      checked={checked}
      // The click is handled rather than the change, for its shift key
      onChange={() => {}}
      onClick={e => { e.stopPropagation(); onClick(e); }}
      aria-label={label}
      style={{ cursor: 'pointer', margin: 0 }}
    />
  );
}

/** Tags typed one at a time, as tag names can have spaces in them. */
function TagsInput({ tags, setTags, suggestions, onSubmit }: {
  tags: string[];
  setTags: (tags: string[]) => void;
  suggestions: string[];
  onSubmit: (tags: string[]) => void;
}) {
  const [text, setText] = useState('');
  const listId = useId();

  /** The tags with what is typed, if anything. */
  function withTyped(): string[] {
    const tag = text.trim();
    return tag && !tags.includes(tag) ? [...tags, tag] : tags;
  }

  return (
    <>
      {tags.map(tag => (
        <span key={tag} style={{ ...small, border: '1px solid var(--border)', borderRadius: 10, display: 'inline-flex', gap: 4, alignItems: 'center' }}>
          {tag}
          <button onClick={() => setTags(tags.filter(t => t !== tag))} aria-label={`Leave out ${tag}`} style={{ border: 'none', background: 'none', padding: 0, cursor: 'pointer', color: 'var(--text-muted)' }}>×</button>
        </span>
      ))}
      <input
        type="text"
        list={listId}
        value={text}
        autoFocus
        onChange={e => setText(e.target.value)}
        onKeyDown={e => {
          if (e.key !== 'Enter') return;
          e.preventDefault();
          // Enter adds what is typed, or with nothing typed, applies
          if (text.trim()) { setTags(withTyped()); setText(''); } else onSubmit(tags);
        }}
        placeholder={tags.length ? 'Another tag, or Enter to apply' : 'Tag, then Enter'}
        style={{ ...input, minWidth: 160 }}
      />
      <datalist id={listId}>
        {suggestions.filter(s => !tags.includes(s)).map(s => <option key={s} value={s} />)}
      </datalist>
      <button onClick={() => onSubmit(withTyped())} style={small}>Apply</button>
    </>
  );
}

/**
 * Runs actions on the rows of a list that are checked, or on all that a search finds. Undoable
 * actions offer Undo once done.
 */
export default function BulkActionBar({ noun, selection, query, total, onSelectAll, onSelectNone, search, actions, onDone }: {
  /** What the rows are, as in `note`. */
  noun: string;
  selection: RowSelection;
  /** The search the rows are from, if any. */
  query: string | null;
  /** How many rows the search finds. */
  total: number;
  onSelectAll: () => void;
  onSelectNone: () => void;
  /** The ids of the rows `query` finds, for leaving some out of them. */
  search: (query: string) => Promise<number[]>;
  actions: BulkAction[];
  /** Called once an action has run, or been undone or redone, as it may change any row. */
  onDone: () => void;
}) {
  const count = selectedCount(selection, total);
  const [pending, setPending] = useState<{ action: BulkAction; tags: string[]; text: string; suggestions: string[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Numbered so each new outcome gets a fresh ActionResult (and Undo button)
  const [outcome, setOutcome] = useState<{ seq: number; outcome: ActionOutcome } | null>(null);

  const plural = (n: number) => `${n} ${noun}${n === 1 ? '' : 's'}`;

  function choose(action: BulkAction) {
    setError(null);
    if (!action.input) { void run(action, []); return; }
    setPending({ action, tags: [], text: '', suggestions: [] });
    if (action.input.kind === 'tags') {
      action.input.suggestions().then(
        suggestions => setPending(prev => (prev?.action === action ? { ...prev, suggestions } : prev)),
        (e: unknown) => setError(errorMessage(e)),
      );
    }
  }

  async function run(action: BulkAction, value: string[]) {
    if (action.input && !value.some(v => v.trim())) { setError(`Nothing to ${action.label.toLowerCase()}`); return; }
    if (!window.confirm(action.confirm(count, value))) return;
    setBusy(true);
    setError(null);
    try {
      const selector = await resolveSelection(selection, query, search);
      const result = await action.run(selector, value, count);
      setOutcome(prev => ({ seq: (prev?.seq ?? 0) + 1, outcome: result }));
      setPending(null);
      onDone();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const scope = selection.all
    ? selection.ids.size
      ? `All ${plural(total)} found except ${selection.ids.size}: ${count}`
      : `All ${plural(total)} found`
    : count
      ? `${plural(count)} checked`
      : `Check ${noun}s, or search for them, to act on them at once`;

  return (
    <div style={{ border: '1px solid var(--border)', borderRadius: 6, padding: '8px 12px', marginBottom: 12, fontSize: 13 }}>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
        <span style={{ color: 'var(--text-secondary)' }}>{scope}</span>
        {query !== null && !(selection.all && selection.ids.size === 0) && <button onClick={onSelectAll} style={small}>Select all found</button>}
        {count > 0 && <button onClick={onSelectNone} style={small}>Select none</button>}
        <span style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginLeft: 'auto' }}>
          {actions.map(action => (
            <button
              key={action.label}
              onClick={() => choose(action)}
              disabled={busy || count === 0}
              aria-pressed={pending?.action === action}
              style={{ ...small, ...(action.danger ? { color: 'var(--danger)' } : {}), ...(pending?.action === action ? { background: 'var(--selected)' } : {}) }}
            >
              {action.label}{action.input ? '…' : ''}
            </button>
          ))}
        </span>
      </div>

      {pending?.action.input && (
        <div style={{ display: 'flex', gap: 6, alignItems: 'center', flexWrap: 'wrap', marginTop: 8 }}>
          <span>{pending.action.label}:</span>
          {pending.action.input.kind === 'tags' ? (
            <TagsInput
              tags={pending.tags}
              setTags={tags => setPending({ ...pending, tags })}
              suggestions={pending.suggestions}
              onSubmit={tags => { setPending({ ...pending, tags }); void run(pending.action, tags); }}
            />
          ) : (
            <>
              <input
                type={pending.action.input.kind === 'datetime' ? 'datetime-local' : 'number'}
                value={pending.text}
                autoFocus
                onChange={e => setPending({ ...pending, text: e.target.value })}
                onKeyDown={e => { if (e.key === 'Enter') void run(pending.action, [pending.text]); }}
                {...(pending.action.input.kind === 'number'
                  ? { placeholder: pending.action.input.placeholder, min: pending.action.input.min, max: pending.action.input.max, step: pending.action.input.step }
                  : {})}
                style={input}
              />
              <button onClick={() => void run(pending.action, [pending.text])} disabled={busy} style={small}>Apply</button>
            </>
          )}
          <button onClick={() => setPending(null)} style={small}>Cancel</button>
        </div>
      )}

      {busy && <div style={{ marginTop: 8, color: 'var(--text-secondary)' }}>Working…</div>}
      {error && <pre style={{ color: 'var(--error)', fontSize: 13, margin: '8px 0 0', whiteSpace: 'pre-wrap' }}>{error}</pre>}
      {outcome && <ActionResult key={outcome.seq} outcome={outcome.outcome} onReverted={onDone} />}
    </div>
  );
}
