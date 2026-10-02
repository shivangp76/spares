import { useState } from 'react';
import { forgetCard, getLeeches, searchCards, unburyCards, updateCards } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import { STATE_LABELS, type CardResponse, type CardsSelector, type SpecialStateUpdate, type UpdateCardsRequest } from '../types/spares';
import { sectionLabel, td, th } from '../utils';
import ActionResult, { type ActionOutcome } from './ActionResult';

type SelectorMode = 'query' | 'ids';
// `''` leaves the special state unchanged, `'None'` clears it
type SpecialStateChoice = '' | 'None' | Exclude<SpecialStateUpdate, 'BuriedUntilLaterToday'>;

const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid #ccc', borderRadius: 4 };
const row: React.CSSProperties = { display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', fontSize: 14 };
const section: React.CSSProperties = { border: '1px solid #ddd', borderRadius: 6, padding: 16, marginBottom: 16 };

/** Parses card ids separated by spaces or commas, like the CLI's `--ids`. */
function parseIds(value: string): number[] {
  const parts = value.split(/[\s,]+/).filter(Boolean);
  const ids = parts.map(Number);
  const invalid = parts.filter((_, i) => !Number.isInteger(ids[i]));
  if (invalid.length) throw new Error(`Invalid card id(s): ${invalid.join(', ')}`);
  if (!ids.length) throw new Error('No card ids given');
  return ids;
}

/** A `--ids` / `--query` target, as the CLI's `CardsSelectorLocal`. */
function SelectorInput({ mode, setMode, value, setValue, onEnter }: {
  mode: SelectorMode;
  setMode: (mode: SelectorMode) => void;
  value: string;
  setValue: (value: string) => void;
  onEnter: () => void;
}) {
  return (
    <>
      <select value={mode} onChange={e => setMode(e.target.value as SelectorMode)} style={input} aria-label="Select cards by">
        <option value="query">Query</option>
        <option value="ids">Card ids</option>
      </select>
      <input
        type="text"
        value={value}
        onChange={e => setValue(e.target.value)}
        onKeyDown={e => { if (e.key === 'Enter') onEnter(); }}
        placeholder={mode === 'query' ? 'e.g. tag=a c.state=2' : 'e.g. 12 34 56'}
        style={{ ...input, flex: 1, minWidth: 200 }}
      />
    </>
  );
}

/** Resolves a selector to card ids, searching when it is a query. */
async function resolveCardIds(mode: SelectorMode, value: string): Promise<number[]> {
  if (mode === 'ids') return parseIds(value);
  if (!value.trim()) throw new Error('No query given');
  return (await searchCards(value.trim())).map(c => c.id);
}

/** Bulk card commands from the CLI: `card unbury`, `card forget`, `card edit` and `card leeches`. */
export default function CardTools({ onOpenCard }: {
  /** Opens a card from the leeches table, along with the table so it can be stepped through. */
  onOpenCard?: (index: number, cards: CardResponse[]) => void;
}) {
  const { credentials } = useAuth();
  // Numbered so each new outcome gets a fresh ActionResult (and Undo button)
  const [outcome, setOutcome] = useState<{ seq: number; outcome: ActionOutcome } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [unburyQuery, setUnburyQuery] = useState('');

  const [forgetMode, setForgetMode] = useState<SelectorMode>('query');
  const [forgetValue, setForgetValue] = useState('');

  const [editMode, setEditMode] = useState<SelectorMode>('query');
  const [editValue, setEditValue] = useState('');
  const [retention, setRetention] = useState('');
  const [specialState, setSpecialState] = useState<SpecialStateChoice>('');
  const [due, setDue] = useState('');

  const [leeches, setLeeches] = useState<CardResponse[] | null>(null);

  async function run(action: () => Promise<ActionOutcome | null>) {
    setBusy(true);
    setError(null);
    try {
      const result = await action();
      if (result) setOutcome(prev => ({ seq: (prev?.seq ?? 0) + 1, outcome: result }));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  function unbury() {
    run(async () => {
      const query = unburyQuery.trim() || null;
      await unburyCards(query);
      // The endpoint doesn't return the event it records, so no Undo is offered
      return { message: query ? `Unburied cards matching \`${query}\`.` : 'Unburied all cards.', eventIds: [] };
    });
  }

  function forget() {
    run(async () => {
      const ids = await resolveCardIds(forgetMode, forgetValue);
      if (!ids.length) return { message: 'No cards matched.', eventIds: [] };
      if (!window.confirm(`Forget ${ids.length} card(s)? Their scheduling is reset; review logs are kept.`)) return null;
      const eventIds: number[] = [];
      for (const id of ids) {
        const res = await forgetCard(id);
        if (res.event_id !== null) eventIds.push(res.event_id);
      }
      return { message: `Forgot ${ids.length} card(s).`, eventIds };
    });
  }

  function edit() {
    run(async () => {
      const req: Omit<UpdateCardsRequest, 'selector'> = {};
      if (retention.trim()) {
        const r = Number(retention);
        if (!(r > 0 && r < 1)) throw new Error(`Desired retention must be between 0 and 1: ${retention}`);
        req.desired_retention = r;
      }
      if (specialState) req.special_state = specialState === 'None' ? null : specialState;
      if (due) req.due = new Date(due).toISOString();
      if (!Object.keys(req).length) throw new Error('Nothing to change');

      let selector: CardsSelector;
      let count: number;
      if (editMode === 'ids') {
        const ids = parseIds(editValue);
        selector = { Ids: ids };
        count = ids.length;
      } else {
        const query = editValue.trim();
        if (!query) throw new Error('No query given');
        selector = { Query: query };
        count = (await searchCards(query)).length;
      }
      if (!count) return { message: 'No cards matched.', eventIds: [] };
      if (!window.confirm(`Edit ${count} card(s)?`)) return null;
      const res = await updateCards({ selector, ...req });
      return { message: `Edited ${res.cards.length} card(s).`, eventIds: res.event_id === null ? [] : [res.event_id] };
    });
  }

  function loadLeeches() {
    if (!credentials) return;
    run(async () => {
      setLeeches(await getLeeches(credentials.schedulerName));
      return null;
    });
  }

  return (
    <div>
      <div style={section}>
        <div style={sectionLabel}>Unbury</div>
        <div style={row}>
          <input
            type="text"
            value={unburyQuery}
            onChange={e => setUnburyQuery(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter') unbury(); }}
            placeholder="Optional query; all buried cards if empty"
            style={{ ...input, flex: 1, minWidth: 200 }}
          />
          <button onClick={unbury} disabled={busy}>Unbury</button>
        </div>
      </div>

      <div style={section}>
        <div style={sectionLabel}>Forget (reset scheduling, keep review logs)</div>
        <div style={row}>
          <SelectorInput mode={forgetMode} setMode={setForgetMode} value={forgetValue} setValue={setForgetValue} onEnter={forget} />
          <button onClick={forget} disabled={busy}>Forget</button>
        </div>
      </div>

      <div style={section}>
        <div style={sectionLabel}>Edit</div>
        <div style={row}>
          <SelectorInput mode={editMode} setMode={setEditMode} value={editValue} setValue={setEditValue} onEnter={edit} />
        </div>
        <div style={{ ...row, marginTop: 8 }}>
          <label>
            Desired retention{' '}
            <input type="number" step="0.01" min="0" max="1" value={retention} onChange={e => setRetention(e.target.value)} placeholder="unchanged" style={{ ...input, width: 100 }} />
          </label>
          <label>
            Special state{' '}
            <select value={specialState} onChange={e => setSpecialState(e.target.value as SpecialStateChoice)} style={input}>
              <option value="">unchanged</option>
              <option value="None">None</option>
              <option value="Suspended">Suspended</option>
              <option value="Buried">Buried</option>
            </select>
          </label>
          <label>
            Due{' '}
            <input type="datetime-local" value={due} onChange={e => setDue(e.target.value)} style={input} />
          </label>
          <button onClick={edit} disabled={busy}>Apply</button>
        </div>
      </div>

      <div style={section}>
        <div style={{ ...row, justifyContent: 'space-between' }}>
          <div style={{ ...sectionLabel, marginBottom: 0 }}>Leeches (frequently forgotten)</div>
          <button onClick={loadLeeches} disabled={busy}>{leeches ? 'Refresh' : 'Load'}</button>
        </div>
        {leeches && (leeches.length === 0 ? (
          <p style={{ color: '#555', fontSize: 14, marginBottom: 0 }}>No leeches.</p>
        ) : (
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14, marginTop: 8 }}>
            <thead>
              <tr>
                <th style={th}>Card</th>
                <th style={th}>Note</th>
                <th style={th}>State</th>
                <th style={th}>Due</th>
                <th style={th}>Special state</th>
              </tr>
            </thead>
            <tbody>
              {leeches.map((c, i) => (
                <tr
                  key={c.id}
                  onClick={onOpenCard ? () => onOpenCard(i, leeches) : undefined}
                  style={onOpenCard ? { cursor: 'pointer' } : undefined}
                >
                  <td style={td}>{c.id}</td>
                  <td style={td}>{c.note_id}</td>
                  <td style={td}>{STATE_LABELS[c.state] ?? c.state}</td>
                  <td style={td}>{new Date(c.due).toLocaleString()}</td>
                  <td style={td}>{c.special_state ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ))}
      </div>

      {error && <div style={{ color: 'red', fontSize: 13 }}>Error: {error}</div>}
      {outcome && <ActionResult key={outcome.seq} outcome={outcome.outcome} />}
    </div>
  );
}
