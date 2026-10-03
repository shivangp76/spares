import { useState } from 'react';
import { redoEvent, undoEvent } from '../api/client';
import { RedoIcon, UndoIcon } from './Icons';

export interface ActionOutcome {
  message: string;
  // Events the action created, oldest first. Undo is offered iff non-empty.
  eventIds: number[];
}

/**
 * Shows the outcome of a mutating action, with an Undo button for the events it created. Once
 * undone, the button becomes Redo, and so on.
 */
export default function ActionResult({ outcome, onReverted }: { outcome: ActionOutcome; onReverted?: () => void }) {
  const [undone, setUndone] = useState(false);
  // Events the next Undo or Redo reverses, processed last first. Undo returns its undo events
  // newest action first, so redoing them last first replays the actions in their original order.
  const [eventIds, setEventIds] = useState(outcome.eventIds);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function revert() {
    setBusy(true);
    try {
      const reverted: number[] = [];
      const created: number[] = [];
      // Last first, so each event is reversed on top of the state it produced
      for (const id of [...eventIds].reverse()) {
        if (undone) {
          const res = await redoEvent(id);
          if (res) {
            reverted.push(...res.redone_event_ids);
            created.push(res.redo_event_ids[0]);
          }
        } else {
          const res = await undoEvent(id);
          if (res) {
            reverted.push(...res.undone_event_ids);
            created.push(res.undo_event_ids[0]);
          }
        }
      }
      const verb = undone ? 'Redone' : 'Undone';
      setStatus(reverted.length ? `${verb} event(s): ${reverted.join(', ')}` : `No event to ${undone ? 'redo' : 'undo'}.`);
      setEventIds(created);
      setUndone(!undone);
      onReverted?.();
    } catch (e) {
      setStatus(String(e));
    } finally {
      setBusy(false);
    }
  }

  const label = undone ? (busy ? 'Redoing…' : 'Redo') : (busy ? 'Undoing…' : 'Undo');
  return (
    <div style={{ marginTop: 8, fontSize: 13, color: 'var(--text-secondary)', display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
      <span>{outcome.message}</span>
      {eventIds.length > 0 && (
        <button onClick={revert} disabled={busy} style={{ padding: '2px 8px', fontSize: 12 }}>
          {undone ? <RedoIcon /> : <UndoIcon />}{label}
        </button>
      )}
      {status && <span>{status}</span>}
    </div>
  );
}
