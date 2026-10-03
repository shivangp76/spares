import { useEffect, useRef, useState } from 'react';

interface Props { source: string; onReady?: () => void }

type State =
  | { source: string; width: number; svg: string; error?: undefined }
  | { source: string; width: number; error: string; svg?: undefined };

// The viewer's width is passed to sources as `sys.inputs.at("spares-width")`, in CSS pixels, so
// templates can lay out pages to fit the screen. It is rounded down to a step and debounced so
// small resizes do not recompile.
const WIDTH_INPUT = 'spares-width';
const WIDTH_STEP = 16;
const RESIZE_DEBOUNCE_MS = 150;

function useStepWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(Math.max(WIDTH_STEP, Math.floor(el.clientWidth / WIDTH_STEP) * WIDTH_STEP));
    measure();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(measure, RESIZE_DEBOUNCE_MS);
    });
    observer.observe(el);
    return () => { observer.disconnect(); clearTimeout(timer); };
  }, []);

  return [ref, width] as const;
}

export default function TypstViewer({ source, onReady }: Props) {
  const [ref, width] = useStepWidth();
  const [result, setResult] = useState<State | null>(null);
  // Expanding compiles the source without the width input, so the template lays it out at its
  // default size, e.g. for long equations clipped at the screen's width. Tied to a source so the
  // next card starts fitted.
  const [expandedSource, setExpandedSource] = useState<string | null>(null);
  const expanded = expandedSource === source;
  const [natural, setNatural] = useState<{ source: string; svg?: string; error?: string } | null>(null);

  useEffect(() => {
    if (width === null) return;
    let cancelled = false;
    // Loaded lazily since the compiler is large and only needed for Typst cards.
    import('../typst/compiler').then(({ compileTypst }) => compileTypst(source, { [WIDTH_INPUT]: String(width) })).then(
      svg => { if (!cancelled) setResult({ source, width, svg }); },
      (e: unknown) => { if (!cancelled) setResult({ source, width, error: String(e) }); },
    );
    return () => { cancelled = true; };
  }, [source, width]);

  const naturalSource = natural?.source;
  useEffect(() => {
    if (!expanded || naturalSource === source) return;
    let cancelled = false;
    import('../typst/compiler').then(({ compileTypst }) => compileTypst(source)).then(
      svg => { if (!cancelled) setNatural({ source, svg }); },
      (e: unknown) => { if (!cancelled) setNatural({ source, error: String(e) }); },
    );
    return () => { cancelled = true; };
  }, [source, expanded, naturalSource]);

  const done = result?.source === source;
  useEffect(() => {
    if (done) onReady?.();
  }, [done, onReady]);

  const errorView = (error: string) => (
    <div>
      <p style={{ color: 'var(--danger)', fontSize: 13 }}>Typst compilation failed</p>
      <pre style={{ fontSize: 12, whiteSpace: 'pre-wrap' }}>{error}</pre>
    </div>
  );

  let content;
  let canExpand = false;
  // The fitted render is kept on screen while the expanded one compiles.
  if (expanded && naturalSource === source) {
    canExpand = true;
    content = natural?.error !== undefined ? errorView(natural.error) : (
      <div style={{ overflowX: 'auto' }}>
        <div className="typst-viewer typst-viewer-natural" dangerouslySetInnerHTML={{ __html: natural?.svg ?? '' }} />
      </div>
    );
  // Ignore the result of a previous source while the current one compiles. A result for a previous
  // width is kept until the new one is ready so resizing does not flicker.
  } else if (result?.source !== source) {
    content = <div>Compiling Typst…</div>;
  } else if (result.error !== undefined) {
    content = errorView(result.error);
  } else {
    canExpand = true;
    content = <div className="typst-viewer" dangerouslySetInnerHTML={{ __html: result.svg }} />;
  }
  return (
    <div ref={ref} style={{ position: 'relative' }}>
      {content}
      {canExpand && (
        <button
          className="typst-expand touch-target-small"
          onClick={() => setExpandedSource(expanded ? null : source)}
          title={expanded ? 'Fit to screen' : 'Show at original size'}
        >
          {!expanded ? 'Expand' : naturalSource === source ? 'Fit' : 'Expanding…'}
        </button>
      )}
    </div>
  );
}
