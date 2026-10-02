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
}

export default function CardRenderer({ path, parserName, source, version }: Props) {
  if (source !== undefined) return <TypstViewer source={source} />;

  const url = version === undefined ? fileUrl(path) : `${fileUrl(path)}?v=${version}`;
  const name = parserName.toLowerCase();
  if (name.includes('latex')) return <PdfViewer url={url} />;
  return <MarkdownViewer url={url} />;
}
