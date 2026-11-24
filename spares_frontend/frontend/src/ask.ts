/**
 * The app's own confirm, prompt and alert, in its colours, in place of the browser's
 * `window.confirm`, `window.prompt` and `window.alert`, which look different on each platform and
 * stop the page while open. Each `ask…` resolves once the question is answered; `AskDialogHost`
 * (components/AskDialog.tsx), in `App`, shows them.
 */

export interface Question {
  title: string;
  /** Said below the title. Line breaks are kept. */
  message?: string;
  /** The button that agrees, e.g. "Reschedule". */
  confirmLabel: string;
  /** Destroys something, so the button is red and Cancel has the focus. */
  danger?: boolean;
  /** Only tells something, with an OK button and no Cancel, as `window.alert`. */
  notice?: boolean;
  /** Asks for text, e.g. a name, with this label, and resolves to what was typed. */
  input?: { label: string; initial?: string; type?: "text" | "password" };
}

export type Pending = Question & {
  id: number;
  resolve: (answer: string | null) => void;
};

let pending: Pending[] = [];
let nextId = 0;
const listeners = new Set<() => void>();

export function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function ask(question: Question): Promise<string | null> {
  return new Promise((resolve) => {
    pending = [...pending, { ...question, id: nextId++, resolve }];
    listeners.forEach((l) => l());
  });
}

export function answer(value: string | null) {
  const [first, ...rest] = pending;
  pending = rest;
  listeners.forEach((l) => l());
  first?.resolve(value);
}

/** Whether the user agreed. */
export async function askConfirm(
  question: Omit<Question, "input">,
): Promise<boolean> {
  return (await ask(question)) !== null;
}

/** Tells the user something, resolved once they have read it. */
export async function showNotice(
  notice: Pick<Question, "title" | "message">,
): Promise<void> {
  await ask({ ...notice, confirmLabel: "OK", notice: true });
}

/** What the user typed, trimmed unless a password, or null if cancelled or left empty. */
export async function askText(
  question: Question & { input: NonNullable<Question["input"]> },
): Promise<string | null> {
  const text = await ask(question);
  if (text === null) return null;
  const value = question.input.type === "password" ? text : text.trim();
  return value || null;
}

/** The questions not yet answered, oldest first. */
export function pendingQuestions(): Pending[] {
  return pending;
}
