import { ApiError } from "./api/client";
import {
  diagnosticCount,
  linter,
  setDiagnostics,
  setDiagnosticsEffect,
} from "@codemirror/lint";
import { EditorState, type EditorView } from "@uiw/react-codemirror";

export const noteDiagnostics = [
  linter(null),
  EditorState.transactionExtender.of((transaction) =>
    transaction.docChanged && diagnosticCount(transaction.startState)
      ? { effects: setDiagnosticsEffect.of([]) }
      : null,
  ),
];

/** Only mark the submitted text when it still matches the editor after the request. */
export function showNoteDiagnostic(
  view: EditorView | undefined,
  submitted: string,
  error: unknown,
) {
  if (
    !view ||
    !(error instanceof ApiError) ||
    !error.source ||
    view.state.doc.toString() !== submitted
  )
    return;
  const offset = submitted.indexOf(error.source);
  if (offset < 0 || submitted.lastIndexOf(error.source) !== offset) return;
  const diagnostics = error.spans
    .filter(
      ([from, to]) => from >= 0 && from <= to && to <= error.source!.length,
    )
    .map(([from, to]) => ({
      from: offset + from,
      to: offset + to,
      message: error.message,
      severity: "error" as const,
    }));
  if (diagnostics.length) {
    view.dispatch(setDiagnostics(view.state, diagnostics));
    view.dispatch({
      selection: { anchor: diagnostics[0].from },
      scrollIntoView: true,
    });
    view.focus();
  }
}
