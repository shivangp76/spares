import { fileUrl } from '../api/client';
import MarkdownViewer from './MarkdownViewer';
import PdfViewer from './PdfViewer';
import TypstViewer from './TypstViewer';

interface Props {
  path: string;
  parserName: string;
}

export default function CardRenderer({ path, parserName }: Props) {
  const url = fileUrl(path);
  const name = parserName.toLowerCase();

  if (name.includes('latex')) return <PdfViewer url={url} />;
  if (name.includes('typst')) return <TypstViewer url={url} />;
  return <MarkdownViewer url={url} />;
}
