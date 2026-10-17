import { useEffect, useId, useRef, useState, type ReactNode } from 'react';

/**
 * A modal popup with a title, a Close button and an optional intro, closed by Esc, the button or a
 * click on the backdrop. `children` can be a function, given a `close` that closes the popup (as
 * after a pick).
 */
export default function HelpDialog({ title, intro, wide, onClose, children }: {
  title: string;
  intro?: ReactNode;
  /** For tables of examples, which need more room than a list of keys. */
  wide?: boolean;
  onClose: () => void;
  children: ReactNode | ((close: () => void) => ReactNode);
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    // Not closed on unmount, since its close event would fire `onClose` after StrictMode remounts it
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);
  // Closed from an effect, so `children` can be given `close` while rendering
  const [closeRequested, setCloseRequested] = useState(false);
  useEffect(() => {
    if (closeRequested) dialogRef.current?.close();
  }, [closeRequested]);
  const close = () => setCloseRequested(true);

  return (
    <dialog
      ref={dialogRef}
      className={wide ? 'help-dialog wide' : 'help-dialog'}
      aria-labelledby={titleId}
      onClose={onClose}
      // The dialog itself is only hit by a click on the backdrop, as its content fills it
      onClick={e => { if (e.target === e.currentTarget) close(); }}
    >
      <div className="help-dialog-content">
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 16 }}>
          <h3 id={titleId} style={{ margin: 0 }}>{title}</h3>
          <button type="button" onClick={close}>Close <span className="key-hint">(Esc)</span></button>
        </div>
        {intro && <div className="help-dialog-intro">{intro}</div>}
        {typeof children === 'function' ? children(close) : children}
      </div>
    </dialog>
  );
}
