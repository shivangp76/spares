import { useEffect, useRef, useState } from 'react';
import { fetchFile } from '../api/client';
import { useResolvedTheme } from '../theme';

interface Props {
  path: string;
  version?: number;
  /** Opens a linked note in the app. Without it, note links do nothing. */
  onOpenNote?: (noteId: number) => void;
  onReady?: () => void;
}

// Links to linked notes point at their raw file, e.g. `/…/0012.md`, so the note's ID is its name
const NOTE_LINK = /(?:^|\/)(\d+)\.md$/;
const STYLE_ID = 'spares-theme';

/**
 * HTML rendered by pandoc, in a sandboxed iframe so its CSS cannot style the app and nothing in it
 * can run. `allow-same-origin` lets the app size and theme it, and must never be combined with
 * `allow-scripts`, which would remove the sandbox.
 */
export default function HtmlViewer({ path, version, onOpenNote, onReady }: Props) {
  const key = `${path}?v=${version}`;
  const [loaded, setLoaded] = useState<{ key: string; html?: string; error?: string } | null>(null);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(0);
  const theme = useResolvedTheme();
  const themeRef = useRef(theme);
  const onOpenNoteRef = useRef(onOpenNote);
  useEffect(() => {
    themeRef.current = theme;
    onOpenNoteRef.current = onOpenNote;
  }, [theme, onOpenNote]);

  useEffect(() => {
    let cancelled = false;
    fetchFile(path)
      .then(blob => blob.text())
      .then(html => { if (!cancelled) setLoaded({ key, html }); })
      .catch(e => { if (!cancelled) setLoaded({ key, error: String(e) }); });
    return () => { cancelled = true; };
  }, [key, path]);

  const current = loaded?.key === key ? loaded : null;

  useEffect(() => {
    if (current?.error !== undefined) onReady?.();
  }, [current, onReady]);

  // Sizes, themes and wires up links each time a document loads
  useEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe || current?.html === undefined) return;
    let observer: ResizeObserver | null = null;
    const onLoad = () => {
      const doc = iframe.contentDocument;
      if (!doc) return;
      applyTheme(doc, themeRef.current);
      doc.addEventListener('click', e => onClick(e, onOpenNoteRef.current));
      observer = new ResizeObserver(() => setHeight(doc.documentElement.scrollHeight));
      observer.observe(doc.documentElement);
      setHeight(doc.documentElement.scrollHeight);
      onReady?.();
    };
    iframe.addEventListener('load', onLoad);
    return () => {
      iframe.removeEventListener('load', onLoad);
      observer?.disconnect();
    };
  }, [current, onReady]);

  // Re-themes on a theme change, without reloading
  useEffect(() => {
    const doc = iframeRef.current?.contentDocument;
    if (doc?.head) applyTheme(doc, theme);
  }, [theme]);

  if (current?.error !== undefined) return <div style={{ color: 'var(--error)' }}>Error loading card: {current.error}</div>;
  if (current === null) return <div>Loading…</div>;
  return (
    <iframe
      ref={iframeRef}
      title="Rendered card"
      sandbox="allow-same-origin"
      srcDoc={current.html}
      style={{ display: 'block', width: '100%', height, border: 'none', colorScheme: theme }}
    />
  );
}

/**
 * Styles the document like the app, before the document's own styles so a stylesheet from the web
 * defaults file can override them. pandoc's own page styles are left out when rendering. The app's colours are `light-dark()` values, which the iframe
 * resolves with the same `color-scheme`.
 */
function applyTheme(doc: Document, theme: 'light' | 'dark') {
  const app = getComputedStyle(document.documentElement);
  const appVar = (name: string) => app.getPropertyValue(name).trim();
  const css = `
    :root { color-scheme: ${theme}; }
    html { color: ${appVar('--text')}; background: transparent; font-family: ${getComputedStyle(document.body).fontFamily}; }
    body { margin: 0; padding: 0 4px; line-height: 1.6; overflow-wrap: break-word; }
    a { color: ${appVar('--link')}; }
    img, svg { max-width: 100%; height: auto; }
    math[display="block"] { overflow-x: auto; }
    pre { overflow-x: auto; }
  `;
  let style = doc.getElementById(STYLE_ID);
  if (!style) {
    style = doc.createElement('style');
    style.id = STYLE_ID;
    doc.head.prepend(style);
  }
  style.textContent = css;
}

function onClick(e: MouseEvent, onOpenNote: ((noteId: number) => void) | undefined) {
  const link = (e.target as Element | null)?.closest?.('a[href]');
  if (!link) return;
  const href = link.getAttribute('href') ?? '';
  if (href.startsWith('#')) return;
  e.preventDefault();
  const noteLink = NOTE_LINK.exec(href);
  if (noteLink) onOpenNote?.(parseInt(noteLink[1], 10));
  else window.open(href, '_blank', 'noopener,noreferrer');
}
