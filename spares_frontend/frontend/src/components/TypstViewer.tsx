import { useEffect, useMemo, useRef, useState } from 'react';
import { audioLinkPath } from '../audio';
import type { PagesPerRow } from '../preferences';
import { scrollClozesIntoView } from '../clozeScroll';
import { setTypstThemePreference, useTypstTheme } from '../theme';
import { splitPages } from '../typst/pages';
import { widthInputs } from '../typst/width';
import { errorMessage } from '../utils';

interface Props {
  source: string;
  onReady?: () => void;
  /** Plays an audio clip linked from the card. Without it, the link is followed. */
  onPlayAudio?: (path: string) => void;
  /** Called with each width a source of one page is compiled for, as the viewer is resized. */
  onWidth?: (width: number) => void;
  /** Scrolls the card's cloze into view when it is displayed or moves, unless it is in view. */
  scrollToCloze?: boolean;
  /** Compiles the source for a viewer this many times narrower, which the page is then scaled up
      from, so its text is larger. Pages of a fixed size already fill the viewer and are unchanged. */
  zoom?: number;
  /** How many pages of a multi-page document are shown side by side. One unless given. */
  pagesPerRow?: PagesPerRow;
}

const XLINK = 'http://www.w3.org/1999/xlink';
// The template draws the card's ID (`cloze_tag_str` in spares_core) as invisible text at its first
// answer cloze, for PDF viewers to search for. The SVG keeps it as text in its selection layer.
const CARD_ID = /^card-[a-z]+-\d+$/;

type State =
  | { source: string; width: number; svg: string; error?: undefined }
  | { source: string; width: number; error: string; svg?: undefined };

// The viewer's width (divided by the zoom) is passed to sources (see `widthInputs`). It is rounded
// down to a step and debounced so small resizes do not recompile.
const WIDTH_STEP = 16;
const RESIZE_DEBOUNCE_MS = 150;
// With `pagesPerRow` 'auto', pages are shown two to a row from this viewer width
const TWO_PAGES_MIN_WIDTH = 1000;
// Between pages shown side by side, as `.typst-pages` sets
const PAGE_GAP = 16;

function stepWidth(width: number): number {
  return Math.max(WIDTH_STEP, Math.floor(width / WIDTH_STEP) * WIDTH_STEP);
}

function useViewerWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(el.clientWidth);
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

export default function TypstViewer({ source, onReady, onPlayAudio, onWidth, scrollToCloze, zoom = 1, pagesPerRow = '1' }: Props) {
  const [ref, viewerWidth] = useViewerWidth();
  const [result, setResult] = useState<State | null>(null);
  const resultPages = useMemo(() => (result?.svg === undefined || pagesPerRow === '1' ? null : splitPages(result.svg)), [result, pagesPerRow]);
  // Pages are shown side by side once the source is known to have several, after which each is
  // compiled for its share of the row. Kept for the source so a layout that fits on one page at
  // that width does not switch back and forth.
  const [multiPageSource, setMultiPageSource] = useState<string | null>(null);
  if (resultPages && result?.source === source && multiPageSource !== source) setMultiPageSource(source);
  const maxColumns = pagesPerRow === 'auto' ? (viewerWidth !== null && viewerWidth >= TWO_PAGES_MIN_WIDTH ? 2 : 1) : Number(pagesPerRow);
  const columns = multiPageSource === source ? maxColumns : 1;
  const width = viewerWidth === null ? null : stepWidth((viewerWidth - PAGE_GAP * (columns - 1)) / columns / zoom);
  // The width a source of one page is compiled for, which the next cards are likely compiled for
  const singlePageWidth = viewerWidth === null ? null : stepWidth(viewerWidth / zoom);
  // Expanding compiles the source without the width input, so the template lays it out at its
  // default size, e.g. for long equations clipped at the screen's width. Tied to a source so the
  // next card starts fitted.
  const [expandedSource, setExpandedSource] = useState<string | null>(null);
  const expanded = expandedSource === source;
  const [natural, setNatural] = useState<{ source: string; svg?: string; error?: string } | null>(null);
  const theme = useTypstTheme();

  useEffect(() => {
    if (singlePageWidth !== null) onWidth?.(singlePageWidth);
  }, [singlePageWidth, onWidth]);

  useEffect(() => {
    if (width === null) return;
    let cancelled = false;
    // Loaded lazily since the compiler is large and only needed for Typst cards.
    import('../typst/compiler').then(({ compileTypst }) => compileTypst(source, widthInputs(width))).then(
      svg => { if (!cancelled) setResult({ source, width, svg }); },
      (e: unknown) => { if (!cancelled) setResult({ source, width, error: errorMessage(e) }); },
    );
    return () => { cancelled = true; };
  }, [source, width]);

  const naturalSource = natural?.source;
  useEffect(() => {
    if (!expanded || naturalSource === source) return;
    let cancelled = false;
    import('../typst/compiler').then(({ compileTypst }) => compileTypst(source)).then(
      svg => { if (!cancelled) setNatural({ source, svg }); },
      (e: unknown) => { if (!cancelled) setNatural({ source, error: errorMessage(e) }); },
    );
    return () => { cancelled = true; };
  }, [source, expanded, naturalSource]);

  const done = result?.source === source;
  useEffect(() => {
    if (done) onReady?.();
  }, [done, onReady]);

  // Also on each recompile, e.g. for a new width or on expanding, since the cloze may have moved
  const shownSvg = expanded && naturalSource === source ? natural?.svg : done ? result.svg : undefined;
  useEffect(() => {
    if (!scrollToCloze || shownSvg === undefined || !ref.current) return;
    const id = [...ref.current.querySelectorAll('.tsel')].find(el => CARD_ID.test(el.textContent?.trim() ?? ''));
    // The text's SVG group, since browsers place HTML inside transformed SVG inconsistently
    const rect = id?.closest('.typst-text')?.getBoundingClientRect();
    if (rect) scrollClozesIntoView([{ top: rect.top, bottom: rect.bottom }]);
  }, [scrollToCloze, shownSvg, ref]);

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
        <div className={`typst-viewer typst-viewer-natural typst-viewer-${theme}`} dangerouslySetInnerHTML={{ __html: natural?.svg ?? '' }} />
      </div>
    );
  // Ignore the result of a previous source while the current one compiles. A result for a previous
  // width is kept until the new one is ready so resizing does not flicker.
  } else if (result?.source !== source) {
    content = <div>Compiling Typst…</div>;
  } else if (result.error !== undefined) {
    content = errorView(result.error);
  } else if (resultPages && columns > 1) {
    canExpand = true;
    content = (
      <div className="typst-pages" style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}>
        {resultPages.map((page, i) => <div key={i} className={`typst-viewer typst-viewer-${theme}`} dangerouslySetInnerHTML={{ __html: page }} />)}
      </div>
    );
  } else {
    canExpand = true;
    content = <div className={`typst-viewer typst-viewer-${theme}`} dangerouslySetInnerHTML={{ __html: result.svg }} />;
  }
  // Links are `<a xlink:href>` over the linked text in the SVG
  function onClick(e: React.MouseEvent) {
    if (!onPlayAudio) return;
    const link = e.target instanceof Element ? e.target.closest('a') : null;
    const path = audioLinkPath(link && (link.getAttribute('href') ?? link.getAttributeNS(XLINK, 'href')));
    if (path === null) return;
    e.preventDefault();
    onPlayAudio(path);
  }

  return (
    <div ref={ref} style={{ position: 'relative' }} onClick={onClick}>
      {content}
      {canExpand && (
        <div className="typst-tools">
          <button
            className="touch-target-small"
            onClick={() => setTypstThemePreference(theme === 'dark' ? 'light' : 'dark')}
            title={theme === 'dark' ? 'Show Typst pages in light mode' : 'Show Typst pages in dark mode'}
          >
            {theme === 'dark' ? 'Light' : 'Dark'}
          </button>
          <button
            className="touch-target-small"
            onClick={() => setExpandedSource(expanded ? null : source)}
            title={expanded ? 'Fit to screen' : 'Show at original size'}
          >
            {!expanded ? 'Expand' : naturalSource === source ? 'Fit' : 'Expanding…'}
          </button>
        </div>
      )}
    </div>
  );
}
