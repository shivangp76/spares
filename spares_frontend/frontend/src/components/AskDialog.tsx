import { useEffect, useId, useRef, useState, useSyncExternalStore } from "react";
import { answer, pendingQuestions, subscribe, type Pending } from "../ask";

/** Shows the oldest question asked and not yet answered. */
export function AskDialogHost() {
  const question = useSyncExternalStore(subscribe, pendingQuestions)[0];
  // Keyed, so each question starts with its own text and focus
  return question ? <AskDialog key={question.id} question={question} /> : null;
}

function AskDialog({ question }: { question: Pending }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const messageId = useId();
  const [text, setText] = useState(question.input?.initial ?? "");
  // Set by the buttons, so the dialog's close event knows which answer it is
  const result = useRef<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="help-dialog ask-dialog"
      aria-labelledby={titleId}
      aria-describedby={question.message ? messageId : undefined}
      onClose={() => answer(result.current)}
      onClick={(e) => {
        if (e.target === e.currentTarget) dialogRef.current?.close();
      }}
    >
      <form
        className="help-dialog-content"
        onSubmit={(e) => {
          e.preventDefault();
          result.current = question.input ? text : "";
          dialogRef.current?.close();
        }}
      >
        <h3 id={titleId} style={{ margin: 0 }}>
          {question.title}
        </h3>
        {question.message && (
          <p id={messageId} className="ask-dialog-message">
            {question.message}
          </p>
        )}
        {question.input && (
          <label className="ask-dialog-field">
            {question.input.label}
            <input
              type={question.input.type ?? "text"}
              value={text}
              onChange={(e) => setText(e.target.value)}
              autoFocus
              autoComplete={
                question.input.type === "password" ? "current-password" : "off"
              }
            />
          </label>
        )}
        <div className="ask-dialog-buttons">
          {!question.notice && (
            <button
              type="button"
              onClick={() => dialogRef.current?.close()}
              autoFocus={!question.input && question.danger}
            >
              Cancel
            </button>
          )}
          <button
            type="submit"
            className={question.danger ? "button-danger" : "button-primary"}
            autoFocus={!question.input && !question.danger}
          >
            {question.confirmLabel}
          </button>
        </div>
      </form>
    </dialog>
  );
}
