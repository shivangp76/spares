import { useEffect } from 'react';

interface Props { url: string; onReady?: () => void }

export default function PdfViewer({ url, onReady }: Props) {
  // The embedded PDF viewer has no reliable load event, so the PDF counts as ready once embedded
  useEffect(() => { onReady?.(); }, [url, onReady]);

  return (
    <div>
      <embed src={url} type="application/pdf" width="100%" height="600px" />
      <p style={{ marginTop: 8, fontSize: 12, color: '#666' }}>
        <a href={url} target="_blank" rel="noreferrer">Open PDF in new tab</a>
      </p>
    </div>
  );
}
