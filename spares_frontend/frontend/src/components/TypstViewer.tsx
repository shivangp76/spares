import { useEffect, useState } from 'react';

interface Props { source: string }

type State =
  | { source: string; svg: string; error?: undefined }
  | { source: string; error: string; svg?: undefined };

export default function TypstViewer({ source }: Props) {
  const [result, setResult] = useState<State | null>(null);

  useEffect(() => {
    let cancelled = false;
    // Loaded lazily since the compiler is large and only needed for Typst cards.
    import('../typst/compiler').then(({ compileTypst }) => compileTypst(source)).then(
      svg => { if (!cancelled) setResult({ source, svg }); },
      (e: unknown) => { if (!cancelled) setResult({ source, error: String(e) }); },
    );
    return () => { cancelled = true; };
  }, [source]);

  // Ignore the result of a previous source while the current one compiles.
  if (result?.source !== source) return <div>Compiling Typst…</div>;
  if (result.error !== undefined) {
    return (
      <div>
        <p style={{ color: '#b00', fontSize: 13 }}>Typst compilation failed</p>
        <pre style={{ fontSize: 12, whiteSpace: 'pre-wrap' }}>{result.error}</pre>
      </div>
    );
  }
  return <div className="typst-viewer" dangerouslySetInnerHTML={{ __html: result.svg }} />;
}
