import { useState } from 'react';
import { undoEvent } from '../api/client';

export interface ActionOutcome {
  message: string;
  // Events the action created, oldest first. Undo is offered iff non-empty.
  eventIds: number[];
}

/** Shows the outcome of a mutating action, with an Undo button for the events it created. */
export default function ActionResult({ outcome, onUndone }: { outcome: ActionOutcome; onUndone?: () => void }) {
  const [undoStatus, setUndoStatus] = useState<string | null>(null);
  const [undoing, setUndoing] = useState(false);

  async function undo() {
    setUndoing(true);
    try {
      const undone: number[] = [];
      // Newest first, so each event is undone on top of the state it produced
      for (const id of [...outcome.eventIds].reverse()) {
        const res = await undoEvent(id);
        if (res) undone.push(...res.undone_event_ids);
      }
      setUndoStatus(undone.length ? `Undone event(s): ${undone.join(', ')}` : 'No event to undo.');
      onUndone?.();
    } catch (e) {
      setUndoStatus(String(e));
    } finally {
      setUndoing(false);
    }
  }

  return (
    <div style={{ marginTop: 8, fontSize: 13, color: 'var(--text-secondary)', display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
      <span>{outcome.message}</span>
      {outcome.eventIds.length > 0 && undoStatus === null && (
        <button onClick={undo} disabled={undoing} style={{ padding: '2px 8px', fontSize: 12 }}>
          {undoing ? 'Undoing…' : 'Undo'}
        </button>
      )}
      {undoStatus && <span>{undoStatus}</span>}
    </div>
  );
}
