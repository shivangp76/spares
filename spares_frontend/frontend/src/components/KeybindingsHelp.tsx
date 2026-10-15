import { useEffect, useRef } from 'react';

export interface Keybinding {
  keys: string[];
  description: string;
  /** Whether pressing it would do something right now. */
  available: boolean;
}

/** A popup listing every keybinding, with the ones available right now highlighted. */
export default function KeybindingsHelp({ bindings, onClose }: { bindings: Keybinding[]; onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    // Not closed on unmount, since its close event would fire `onClose` after StrictMode remounts it
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="keybindings-help"
      aria-labelledby="keybindings-help-title"
      onClose={onClose}
      // The dialog itself is only hit by a click on the backdrop, as its content fills it
      onClick={e => { if (e.target === e.currentTarget) dialogRef.current?.close(); }}
    >
      <div className="keybindings-help-content">
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 16 }}>
          <h3 id="keybindings-help-title" style={{ margin: 0 }}>Keyboard shortcuts</h3>
          <button onClick={() => dialogRef.current?.close()}>Close <span className="key-hint">(Esc)</span></button>
        </div>
        <p style={{ fontSize: 13, color: 'var(--text-secondary)', margin: '8px 0 12px' }}>
          Highlighted shortcuts work right now; dimmed ones don't apply here.
        </p>
        <table>
          <tbody>
            {bindings.map(b => (
              <tr key={b.keys.join(' ')} className={b.available ? 'available' : 'unavailable'}>
                <td>{b.keys.map(k => <kbd key={k}>{k}</kbd>)}</td>
                <td>{b.description}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </dialog>
  );
}
