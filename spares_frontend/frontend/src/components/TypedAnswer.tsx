import { lazy, Suspense, useRef, useState } from 'react';
import { useVimKeybindings } from '../preferences';
import type { TypedAnswerInfo } from '../types/spares';
import { compareTypedAnswer, isExact, type DiffSegment, type SegmentKind } from '../typedAnswer';

const SEGMENT_STYLES: Record<SegmentKind, React.CSSProperties> = {
  good: { color: 'var(--tone-green-fg)', background: 'var(--tone-green-bg)' },
  bad: { color: 'var(--tone-red-fg)', background: 'var(--tone-red-bg)', textDecoration: 'line-through' },
  missed: { color: 'var(--tone-gray-fg)', background: 'var(--tone-gray-bg)', textDecoration: 'underline' },
};

const answerText: React.CSSProperties = { fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: 15, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' };

const box: React.CSSProperties = { border: '1px solid var(--border)', borderRadius: 4, padding: 12, marginBottom: 16 };

/** `Blank 1`, `Blank 2`, …, or nothing if the card has a single blank. */
function blankLabel(info: TypedAnswerInfo, index: number): string | null {
  return info.blanks.length > 1 ? `Blank ${index + 1}` : null;
}

const VimAnswerInput = lazy(() => import('./VimAnswerInput'));

/** An input box per blank of the card front, with Vim keybindings if they are on in the settings.
    Enter moves to the next box, and Enter in the last box (or Ctrl/Cmd+Enter in any) shows the
    answer. */
export function TypedAnswerInputs({ info, answers, onChange, onSubmit }: {
  info: TypedAnswerInfo;
  answers: string[];
  onChange: (answers: string[]) => void;
  onSubmit: () => void;
}) {
  const vimEnabled = useVimKeybindings();
  const form = useRef<HTMLFormElement>(null);
  const last = info.blanks.length - 1;
  function onEnter(e: React.KeyboardEvent, index: number) {
    if (index === last || e.ctrlKey || e.metaKey) {
      onSubmit();
      return;
    }
    const next = form.current?.querySelector(`[data-blank="${index + 1}"] :is(input, .cm-content)`);
    if (next instanceof HTMLElement) next.focus();
  }
  function setAnswer(index: number, value: string) {
    const next = info.blanks.map((_, j) => answers[j] ?? '');
    next[index] = value;
    onChange(next);
  }
  return (
    <form ref={form} className="typed-answer-inputs" onSubmit={e => e.preventDefault()}>
      {info.blanks.map((blank, i) => {
        const label = blankLabel(info, i);
        const placeholder = blank.hint ?? (label ? undefined : 'Type the answer');
        const input = (
          <input
            value={answers[i] ?? ''}
            onChange={e => setAnswer(i, e.target.value)}
            onKeyDown={e => {
              if (e.key !== 'Enter' || e.nativeEvent.isComposing) return;
              e.preventDefault();
              onEnter(e, i);
            }}
            placeholder={placeholder}
            aria-label={label ?? 'Answer'}
            // The answer is typed right after the card is shown
            autoFocus={i === 0}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            enterKeyHint={i === last ? 'done' : 'next'}
            style={{ ...answerText, padding: '8px 10px', borderRadius: 4, border: '1px solid var(--border)', minWidth: 0 }}
          />
        );
        return (
          <label key={i} data-blank={i} style={{ display: 'grid', gap: 4, minWidth: 0 }}>
            {label && <span style={{ fontSize: 13, color: 'var(--text-secondary)' }}>{label}</span>}
            {vimEnabled ? (
              // The plain box stands in while the editor loads
              <Suspense fallback={input}>
                <VimAnswerInput
                  value={answers[i] ?? ''}
                  onChange={value => setAnswer(i, value)}
                  onEnter={e => onEnter(e, i)}
                  placeholder={placeholder}
                  label={label ?? 'Answer'}
                  enterKeyHint={i === last ? 'done' : 'next'}
                  autoFocus={i === 0}
                />
              </Suspense>
            ) : input}
          </label>
        );
      })}
    </form>
  );
}

function Segments({ segments }: { segments: DiffSegment[] }) {
  return <>{segments.map((s, i) => <span key={i} style={SEGMENT_STYLES[s.kind]}>{s.text}</span>)}</>;
}

/** The answer typed into one blank, compared with its expected answer. */
function BlankDiff({ expected, typed, ignoreCase, ignoreCombining }: { expected: string; typed: string; ignoreCase: boolean; ignoreCombining: boolean }) {
  if (!typed.trim()) return <div style={answerText}>{expected}</div>;
  const diff = compareTypedAnswer(expected, typed, ignoreCase, ignoreCombining);
  if (isExact(diff)) return <div style={answerText}><Segments segments={diff.typed} /></div>;
  return (
    <div style={answerText}>
      <div><Segments segments={diff.typed} /></div>
      <div aria-hidden style={{ color: 'var(--text-muted)' }}>↓</div>
      <div><Segments segments={diff.expected} /></div>
    </div>
  );
}

/** The typed answers on the card back: compared with the expected answers, or shown for the reviewer to compare. */
export function TypedAnswerResult({ info, answers }: { info: TypedAnswerInfo; answers: string[] }) {
  const [manual, setManual] = useState(info.mode.kind === 'manual');
  const { ignore_case: ignoreCase, ignore_combining: ignoreCombining } = info.mode.kind === 'diff' ? info.mode : { ignore_case: false, ignore_combining: false };
  return (
    <div style={box}>
      <div style={{ display: 'grid', gap: 10 }}>
        {info.blanks.map((blank, i) => {
          const label = blankLabel(info, i);
          const typed = answers[i] ?? '';
          return (
            <div key={i}>
              {label && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 2 }}>{label}</div>}
              {manual ? (
                <div>
                  <span style={{ fontSize: 13, color: 'var(--text-secondary)' }}>You typed: </span>
                  {typed.trim() ? <span style={answerText}>{typed.trim()}</span> : <em style={{ color: 'var(--text-muted)' }}>nothing</em>}
                </div>
              ) : (
                <BlankDiff expected={blank.expected} typed={typed} ignoreCase={ignoreCase} ignoreCombining={ignoreCombining} />
              )}
            </div>
          );
        })}
      </div>
      <label style={{ display: 'inline-flex', gap: 6, alignItems: 'center', marginTop: 10, fontSize: 13, color: 'var(--text-secondary)', cursor: 'pointer' }}>
        <input type="checkbox" checked={manual} onChange={e => setManual(e.target.checked)} />
        Compare manually
      </label>
    </div>
  );
}
