import { Link } from 'react-router-dom';
import { GLOBAL_SHORTCUTS, docUrl, type PageHelp as PageHelpContent } from '../help/pages';
import type { Keybinding } from '../help/shortcuts';
import CodeText from './CodeText';
import HelpDialog from './HelpDialog';

/** The help popup of a page: what it is for, its keyboard shortcuts, and where the manual says
    more. `bindings`, a page's own shortcuts, come first, with the ones available right now
    highlighted. */
export default function PageHelp({ help, bindings = [], onClose }: {
  help: PageHelpContent;
  bindings?: Keybinding[];
  onClose: () => void;
}) {
  const shortcuts = [
    ...bindings,
    ...[...help.shortcuts, ...GLOBAL_SHORTCUTS].map(s => ({ ...s, available: true })),
  ];
  const highlighted = bindings.length > 0;
  return (
    <HelpDialog title={help.title} wide onClose={onClose}>
      {help.about.map(paragraph => (
        <p key={paragraph} style={{ fontSize: 14, margin: '8px 0' }}><CodeText text={paragraph} /></p>
      ))}
      <h4 style={{ margin: '16px 0 4px' }}>Keyboard shortcuts</h4>
      {highlighted && (
        <div className="help-dialog-intro" style={{ margin: '0 0 6px' }}>Highlighted shortcuts work right now; dimmed ones don't apply here.</div>
      )}
      <table>
        <tbody>
          {shortcuts.map(s => (
            <tr key={`${s.keys.join(' ')} ${s.description}`} className={!highlighted ? undefined : s.available ? 'available' : 'unavailable'}>
              <td>{s.keys.map(k => <kbd key={k}>{k}</kbd>)}</td>
              <td>{s.description}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {help.docs.length > 0 && (
        <>
          <h4 style={{ margin: '16px 0 4px' }}>In the manual</h4>
          <ul style={{ margin: 0, paddingLeft: 18, fontSize: 14 }}>
            {help.docs.map(link => (
              <li key={link.label}><Link to={docUrl(link)}>{link.label}</Link></li>
            ))}
          </ul>
        </>
      )}
    </HelpDialog>
  );
}
