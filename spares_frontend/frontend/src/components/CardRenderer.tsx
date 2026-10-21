import { useEffect } from 'react';
import { useAuthedFileUrl } from '../hooks/useAuthedFileUrl';
import HtmlViewer from './HtmlViewer';
import PdfViewer from './PdfViewer';
import TypstViewer from './TypstViewer';
import { useCardZoom } from '../preferences';

interface Props {
  path: string;
  /** Raw source for parsers compiled in the browser. Takes precedence over `path`. */
  source?: string;
  /** HTML for web clients, shown instead of the rendered file at `path`. */
  webPath?: string;
  /** Opens a note linked from the card. */
  onOpenNote?: (noteId: number) => void;
  /** Changing it refetches the rendered file, e.g. after it was regenerated. */
  version?: number;
  /** Called once the card is displayed, or has failed to. */
  onReady?: () => void;
  /** Plays an audio clip linked from the card. Without it, the link is followed. */
  onPlayAudio?: (path: string) => void;
  /** Called with each width a Typst source is compiled for. */
  onTypstWidth?: (width: number) => void;
  /** Scrolls the card's clozes into view once it is displayed, e.g. while its side is shown. */
  scrollToCloze?: boolean;
}

export default function CardRenderer({ path, source, webPath, onOpenNote, version, onReady, onPlayAudio, onTypstWidth, scrollToCloze }: Props) {
  // The card text size setting, wherever a card is shown
  const zoom = Number(useCardZoom()) / 100;
  if (source !== undefined) return <TypstViewer source={source} onReady={onReady} onPlayAudio={onPlayAudio} onWidth={onTypstWidth} scrollToCloze={scrollToCloze} zoom={zoom} />;
  if (webPath !== undefined) return <HtmlViewer path={webPath} version={version} onOpenNote={onOpenNote} onReady={onReady} onPlayAudio={onPlayAudio} scrollToCloze={scrollToCloze} zoom={zoom} />;
  return <RenderedFile path={path} version={version} onReady={onReady} />;
}

function RenderedFile({ path, version, onReady }: Pick<Props, 'path' | 'version' | 'onReady'>) {
  const { url, error } = useAuthedFileUrl(path, version);

  useEffect(() => {
    if (error !== null) onReady?.();
  }, [error, onReady]);

  if (error) return <div style={{ color: 'var(--error)' }}>Error loading card: {error}</div>;
  if (url === null) return <div className="loading-delayed">Loading…</div>;
  return <PdfViewer url={url} onReady={onReady} />;
}
