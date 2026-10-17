import HelpDialog from './HelpDialog';

export interface Keybinding {
  keys: string[];
  description: string;
  /** Whether pressing it would do something right now. */
  available: boolean;
}

/** A popup listing every keybinding, with the ones available right now highlighted. */
export default function KeybindingsHelp({ bindings, onClose }: { bindings: Keybinding[]; onClose: () => void }) {
  return (
    <HelpDialog
      title="Keyboard shortcuts"
      intro="Highlighted shortcuts work right now; dimmed ones don't apply here."
      onClose={onClose}
    >
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
    </HelpDialog>
  );
}
