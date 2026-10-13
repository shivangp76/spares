import { useEffect } from 'react';
import { useAuthedFileUrl } from '../hooks/useAuthedFileUrl';
import HtmlViewer from './HtmlViewer';
import PdfViewer from './PdfViewer';
import TypstViewer from './TypstViewer';

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
}

export default function CardRenderer({ path, source, webPath, onOpenNote, version, onReady }: Props) {
  if (source !== undefined) return <TypstViewer source={source} onReady={onReady} />;
  if (webPath !== undefined) return <HtmlViewer path={webPath} version={version} onOpenNote={onOpenNote} onReady={onReady} />;
  return <RenderedFile path={path} version={version} onReady={onReady} />;
}

function RenderedFile({ path, version, onReady }: Pick<Props, 'path' | 'version' | 'onReady'>) {
  const { url, error } = useAuthedFileUrl(path, version);

  useEffect(() => {
    if (error !== null) onReady?.();
  }, [error, onReady]);

  if (error) return <div style={{ color: 'var(--error)' }}>Error loading card: {error}</div>;
  if (url === null) return <div>Loading…</div>;
  return <PdfViewer url={url} onReady={onReady} />;
}
