import { Decoration, type DecorationSet, EditorView, type Extension, type Range, ViewPlugin, type ViewUpdate } from '@uiw/react-codemirror';

/** The syntaxes the note editor highlights, chosen by the note's parser. */
export type NoteSyntax = 'markdown' | 'typst' | 'latex';

export function noteSyntax(parserName: string | null | undefined): NoteSyntax | null {
  const name = parserName?.toLowerCase() ?? '';
  if (name.includes('typst')) return 'typst';
  if (name.includes('latex')) return 'latex';
  if (name.includes('markdown')) return 'markdown';
  return null;
}

/** The syntax's CodeMirror language. Loaded on demand, since each is only needed for its own
    parser's notes. Code blocks are highlighted in their own language, loaded when first seen. */
export async function loadLanguage(syntax: NoteSyntax): Promise<Extension> {
  switch (syntax) {
    case 'markdown': {
      const [{ markdown }, { languages }] = await Promise.all([import('@codemirror/lang-markdown'), import('@codemirror/language-data')]);
      return markdown({ codeLanguages: languages });
    }
    case 'typst': {
      const [{ typst_lezer }, { languages }] = await Promise.all([import('codemirror-lang-typst/lezer'), import('@codemirror/language-data')]);
      return typst_lezer({ codeLanguages: languages });
    }
    case 'latex': {
      const [{ StreamLanguage }, { stex }] = await Promise.all([import('@codemirror/language'), import('@codemirror/legacy-modes/mode/stex')]);
      return StreamLanguage.define(stex);
    }
  }
}

/** A cloze in the text: its whole range, and the ranges of its opening and closing delimiters
    (including any settings, e.g. `{{[o:1]`). */
interface ClozeRange { from: number; to: number; open: [number, number]; close: [number, number] }

/** Follows the markdown parser's cloze parser: `{{[settings] ... }}`, ignoring braces in math,
    comments and escapes. */
function markdownClozes(text: string): ClozeRange[] {
  const clozes: ClozeRange[] = [];
  const open: [number, number][] = [];
  let math = false;
  let i = 0;
  while (i < text.length) {
    if (text.startsWith('<!---', i)) {
      const end = text.indexOf('--->', i + 5);
      i = end === -1 ? text.length : end + 4;
    } else if (text[i] === '\\') {
      i += 2;
    } else if (text[i] === '$') {
      i += text[i + 1] === '$' ? 2 : 1;
      math = !math;
    } else if (!math && text.startsWith('```math', i)) {
      i += 7;
      math = true;
    } else if (math && text.startsWith('```', i)) {
      i += 3;
      math = false;
    } else if (!math && text.startsWith('{{', i)) {
      let end = i + 2;
      if (text[end] === '[') {
        const close = text.indexOf(']', end);
        end = close === -1 ? text.length : close + 1;
      }
      open.push([i, end]);
      i = end;
    } else if (!math && text.startsWith('}}', i) && open.length > 0) {
      const start = open.pop()!;
      clozes.push({ from: start[0], to: i + 2, open: start, close: [i, i + 2] });
      i += 2;
    } else {
      i++;
    }
  }
  return clozes;
}

/** `\begin{cl}[settings] ... \end{cl}`, ignoring comments. */
function latexClozes(text: string): ClozeRange[] {
  const clozes: ClozeRange[] = [];
  const open: [number, number][] = [];
  const BEGIN = '\\begin{cl}';
  const END = '\\end{cl}';
  let i = 0;
  while (i < text.length) {
    if (text[i] === '%') {
      const end = text.indexOf('\n', i);
      i = end === -1 ? text.length : end + 1;
    } else if (text.startsWith(BEGIN, i)) {
      let end = i + BEGIN.length;
      if (text[end] === '[') {
        const close = text.indexOf(']', end);
        end = close === -1 ? text.length : close + 1;
      }
      open.push([i, end]);
      i = end;
    } else if (text.startsWith(END, i) && open.length > 0) {
      const start = open.pop()!;
      const end = i + END.length;
      clozes.push({ from: start[0], to: end, open: start, close: [i, end] });
      i = end;
    } else if (text[i] === '\\') {
      i += 2;
    } else {
      i++;
    }
  }
  return clozes;
}

// Typst's clozes are left to its own highlighting, as calls of `#cl`
const FIND_CLOZES: Record<NoteSyntax, ((text: string) => ClozeRange[]) | null> = {
  markdown: markdownClozes,
  typst: null,
  latex: latexClozes,
};

const clozeMark = Decoration.mark({ class: 'cm-cloze' });
const clozeDelimMark = Decoration.mark({ class: 'cm-cloze-delim' });

const clozeTheme = EditorView.baseTheme({
  '.cm-cloze': { backgroundColor: 'var(--tone-blue-bg)', borderRadius: '2px' },
  '.cm-cloze-delim, .cm-cloze-delim *': { color: 'var(--tone-blue-fg) !important', fontWeight: '600' },
});

/** Marks clozes, so a note's cards can be told apart at a glance. The whole note is scanned on
    each change, which is cheap at the size of a note. */
export function clozeHighlighting(syntax: NoteSyntax): Extension {
  const findClozes = FIND_CLOZES[syntax];
  if (findClozes === null) return [];
  const build = (view: EditorView): DecorationSet => {
    const marks: Range<Decoration>[] = [];
    for (const cloze of findClozes(view.state.doc.toString())) {
      marks.push(clozeMark.range(cloze.from, cloze.to));
      if (cloze.open[1] > cloze.open[0]) marks.push(clozeDelimMark.range(...cloze.open));
      if (cloze.close[1] > cloze.close[0]) marks.push(clozeDelimMark.range(...cloze.close));
    }
    return Decoration.set(marks, true);
  };
  const plugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) { this.decorations = build(view); }
      update(update: ViewUpdate) { if (update.docChanged) this.decorations = build(update.view); }
    },
    { decorations: v => v.decorations },
  );
  return [plugin, clozeTheme];
}
