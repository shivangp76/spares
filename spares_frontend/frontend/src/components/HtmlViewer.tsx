import { useEffect, useRef, useState } from 'react';
import { fetchFile } from '../api/client';
import { audioLinkPath } from '../audio';
import { scrollClozesIntoView } from '../clozeScroll';
import { useResolvedTheme } from '../theme';
import { errorMessage, parseNoteId } from '../utils';

interface Props {
  path: string;
  version?: number;
  /** Opens a linked note in the app. Without it, note links do nothing. */
  onOpenNote?: (noteId: number) => void;
  onReady?: () => void;
  /** Plays an audio clip linked from the card. Without it, the link opens in a new tab. */
  onPlayAudio?: (path: string) => void;
  /** Scrolls the card's clozes into view once it is displayed, unless they already are. */
  scrollToCloze?: boolean;
}

// Links to linked notes point at their raw file, e.g. `/…/0012.md`, so the note's ID is its name
const NOTE_LINK = /(?:^|\/)([a-z]{1,8})\.md$/;
const STYLE_ID = 'spares-theme';

/**
 * HTML rendered by pandoc, in a sandboxed iframe so its CSS cannot style the app and nothing in it
 * can run. `allow-same-origin` lets the app size and theme it, and must never be combined with
 * `allow-scripts`, which would remove the sandbox.
 */
export default function HtmlViewer({ path, version, onOpenNote, onReady, onPlayAudio, scrollToCloze }: Props) {
  const key = `${path}?v=${version}`;
  const [loaded, setLoaded] = useState<{ key: string; html?: string; error?: string } | null>(null);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(0);
  // The key of the document the iframe has loaded, and of the last one scrolled to while shown
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const scrolledKeyRef = useRef<string | null>(null);
  const theme = useResolvedTheme();
  const themeRef = useRef(theme);
  const onOpenNoteRef = useRef(onOpenNote);
  const onPlayAudioRef = useRef(onPlayAudio);
  useEffect(() => {
    themeRef.current = theme;
    onOpenNoteRef.current = onOpenNote;
    onPlayAudioRef.current = onPlayAudio;
  }, [theme, onOpenNote, onPlayAudio]);

  useEffect(() => {
    let cancelled = false;
    fetchFile(path)
      .then(blob => blob.text())
      .then(html => { if (!cancelled) setLoaded({ key, html }); })
      .catch(e => { if (!cancelled) setLoaded({ key, error: errorMessage(e) }); });
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
    const loadedFor = current.key;
    const onLoad = () => {
      const doc = iframe.contentDocument;
      if (!doc) return;
      applyTheme(doc, themeRef.current);
      doc.addEventListener('click', e => onClick(e, onOpenNoteRef.current, onPlayAudioRef.current));
      observer = new ResizeObserver(() => setHeight(doc.documentElement.scrollHeight));
      observer.observe(doc.documentElement);
      setHeight(doc.documentElement.scrollHeight);
      setLoadedKey(loadedFor);
      onReady?.();
    };
    iframe.addEventListener('load', onLoad);
    return () => {
      iframe.removeEventListener('load', onLoad);
      observer?.disconnect();
    };
  }, [current, onReady]);

  // Once the iframe has its height, so the page is long enough to scroll to the clozes. The
  // clozes are pandoc's `[…]{.mark}` spans, which only the card's own clozes are rendered as.
  useEffect(() => {
    if (!scrollToCloze) {
      scrolledKeyRef.current = null;
      return;
    }
    const iframe = iframeRef.current;
    const doc = iframe?.contentDocument;
    if (!iframe || !doc || loadedKey !== current?.key || height === 0 || scrolledKeyRef.current === loadedKey) return;
    scrolledKeyRef.current = loadedKey;
    const offset = iframe.getBoundingClientRect().top;
    scrollClozesIntoView([...doc.querySelectorAll('mark')].map(mark => {
      const rect = mark.getBoundingClientRect();
      return { top: offset + rect.top, bottom: offset + rect.bottom };
    }));
  }, [scrollToCloze, loadedKey, current, height]);

  // Re-themes on a theme change, without reloading
  useEffect(() => {
    const doc = iframeRef.current?.contentDocument;
    if (doc?.head) applyTheme(doc, theme);
  }, [theme]);

  if (current?.error !== undefined) return <div style={{ color: 'var(--error)' }}>Error loading card: {current.error}</div>;
  if (current === null) return <div className="loading-delayed">Loading…</div>;
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

function onClick(
  e: MouseEvent,
  onOpenNote: ((noteId: number) => void) | undefined,
  onPlayAudio: ((path: string) => void) | undefined,
) {
  const link = (e.target as Element | null)?.closest?.('a[href]');
  if (!link) return;
  const href = link.getAttribute('href') ?? '';
  if (href.startsWith('#')) return;
  e.preventDefault();
  const audioPath = audioLinkPath(href);
  if (audioPath !== null && onPlayAudio) {
    onPlayAudio(audioPath);
    // Back to the app, whose hotkeys the iframe would otherwise keep
    (document.activeElement as HTMLElement | null)?.blur();
    window.focus();
    return;
  }
  const noteLink = NOTE_LINK.exec(href);
  const noteId = noteLink ? parseNoteId(noteLink[1]) : null;
  if (noteId !== null) onOpenNote?.(noteId);
  else window.open(href, '_blank', 'noopener,noreferrer');
}
