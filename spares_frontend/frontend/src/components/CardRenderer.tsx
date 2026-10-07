import { useEffect } from 'react';
import { useAuthedFileUrl } from '../hooks/useAuthedFileUrl';
import MarkdownViewer from './MarkdownViewer';
import PdfViewer from './PdfViewer';
import TypstViewer from './TypstViewer';

interface Props {
  path: string;
  parserName: string;
  /** Raw source for parsers compiled in the browser. Takes precedence over `path`. */
  source?: string;
  /** Changing it refetches the rendered file, e.g. after it was regenerated. */
  version?: number;
  /** Called once the card is displayed, or has failed to. */
  onReady?: () => void;
}

export default function CardRenderer({ path, parserName, source, version, onReady }: Props) {
  if (source !== undefined) return <TypstViewer source={source} onReady={onReady} />;
  return <RenderedFile path={path} parserName={parserName} version={version} onReady={onReady} />;
}

function RenderedFile({ path, parserName, version, onReady }: Omit<Props, 'source'>) {
  const { url, error } = useAuthedFileUrl(path, version);

  useEffect(() => {
    if (error !== null) onReady?.();
  }, [error, onReady]);

  if (error) return <div style={{ color: 'var(--error)' }}>Error loading card: {error}</div>;
  if (url === null) return <div>Loading…</div>;
  // Chosen by the rendered file, since a parser's template decides its output, e.g. markdown to PDF
  const name = parserName.toLowerCase();
  if (name.includes('latex') || path.toLowerCase().endsWith('.pdf')) return <PdfViewer url={url} onReady={onReady} />;
  return <MarkdownViewer url={url} onReady={onReady} />;
}
