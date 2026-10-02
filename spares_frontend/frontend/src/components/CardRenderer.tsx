import { fileUrl } from '../api/client';
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

  const url = version === undefined ? fileUrl(path) : `${fileUrl(path)}?v=${version}`;
  // Chosen by the rendered file, since a parser's template decides its output, e.g. markdown to PDF
  const name = parserName.toLowerCase();
  if (name.includes('latex') || path.toLowerCase().endsWith('.pdf')) return <PdfViewer url={url} onReady={onReady} />;
  return <MarkdownViewer url={url} onReady={onReady} />;
}
