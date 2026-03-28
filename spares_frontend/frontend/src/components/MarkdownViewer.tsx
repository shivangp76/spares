import { useEffect, useState } from 'react';
import { marked } from 'marked';
import DOMPurify from 'dompurify';

interface Props { url: string }

export default function MarkdownViewer({ url }: Props) {
  const [html, setHtml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetch(url)
      .then(r => r.text())
      .then(async source => {
        const rendered = await marked.parse(source);
        const clean = DOMPurify.sanitize(rendered);
        if (!cancelled) setHtml(clean);
      })
      .catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [url]);

  if (error) return <div style={{ color: 'red' }}>Error loading markdown: {error}</div>;
  if (html === null) return <div>Loading…</div>;
  return (
    <div
      style={{ lineHeight: 1.6, padding: '0 4px' }}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
