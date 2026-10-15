import { useMemo } from 'react';
import CodeMirror, { EditorState, EditorView } from '@uiw/react-codemirror';
import { getCM, vim, Vim } from '@replit/codemirror-vim';
import { useResolvedTheme } from '../theme';

/** Keeps the answer on one line, joining pasted lines and lines opened with Vim's `o`. */
const singleLine = EditorState.transactionFilter.of(tr => {
  if (!tr.docChanged || tr.newDoc.lines === 1) return tr;
  const text = tr.newDoc.toString();
  return [tr, { changes: { from: 0, to: text.length, insert: text.replace(/\s*\n\s*/g, ' ') }, sequential: true }];
});

const style: React.CSSProperties = { border: '1px solid var(--border)', borderRadius: 4, fontSize: 15, background: 'var(--bg)' };

/** A one-line answer box with Vim keybindings. It starts in insert mode, since the answer is typed
    right away. Enter, in any mode, calls `onEnter` rather than moving down a line. */
export default function VimAnswerInput({ value, onChange, onEnter, placeholder, label, enterKeyHint, autoFocus }: {
  value: string;
  onChange: (value: string) => void;
  onEnter: (e: React.KeyboardEvent) => void;
  placeholder?: string;
  label: string;
  enterKeyHint: 'next' | 'done';
  autoFocus: boolean;
}) {
  const theme = useResolvedTheme();
  const extensions = useMemo(() => [
    vim(),
    singleLine,
    EditorView.contentAttributes.of({ 'aria-label': label, autocapitalize: 'off', autocorrect: 'off', spellcheck: 'false', enterkeyhint: enterKeyHint }),
  ], [label, enterKeyHint]);
  // Caught on the way down to the editor, ahead of Vim, which would take Enter in normal mode
  function onKeyDownCapture(e: React.KeyboardEvent) {
    if (e.key !== 'Enter' || e.nativeEvent.isComposing) return;
    // Not in the panel of a Vim command, e.g. `:s/a/b`
    if (!(e.target instanceof Element && e.target.classList.contains('cm-content'))) return;
    e.preventDefault();
    e.stopPropagation();
    onEnter(e);
  }
  return (
    <div className="typed-answer-input" onKeyDownCapture={onKeyDownCapture}>
      <CodeMirror
        theme={theme}
        value={value}
        onChange={onChange}
        extensions={extensions}
        placeholder={placeholder}
        autoFocus={autoFocus}
        onCreateEditor={view => {
          const cm = getCM(view);
          if (cm) Vim.handleKey(cm, 'i', 'api');
        }}
        basicSetup={{
          lineNumbers: false,
          foldGutter: false,
          highlightActiveLine: false,
          highlightActiveLineGutter: false,
          autocompletion: false,
          closeBrackets: false,
          bracketMatching: false,
          indentOnInput: false,
        }}
        style={style}
      />
    </div>
  );
}
