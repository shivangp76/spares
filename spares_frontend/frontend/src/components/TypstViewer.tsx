import { useEffect, useState } from 'react';

interface Props { url: string }

// All loaded from CDN to avoid Vite trying to resolve @myriaddreamin/typst-ts-web-compiler,
// which is not installed. The bundle sets window.$typst and bundles the web-compiler JS;
// only the WASM binaries are fetched separately at runtime.
const BUNDLE_SRC =
  'https://cdn.jsdelivr.net/npm/@myriaddreamin/typst.ts@0.7.0-rc2/dist/esm/contrib/all-in-one-lite.bundle.js';
const COMPILER_WASM =
  'https://cdn.jsdelivr.net/npm/@myriaddreamin/typst-ts-web-compiler@0.7.0-rc2/pkg/typst_ts_web_compiler_bg.wasm';
const RENDERER_WASM =
  'https://cdn.jsdelivr.net/npm/@myriaddreamin/typst-ts-renderer@0.7.0-rc2/pkg/typst_ts_renderer_bg.wasm';

type TypstGlobal = {
  svg(opts: { mainContent: string }): Promise<string>;
  setCompilerInitOptions(o: { getModule(): string }): void;
  setRendererInitOptions(o: { getModule(): string }): void;
};

// Singleton promise — injects the CDN script tag once and resolves when $typst is ready.
let typstReady: Promise<TypstGlobal> | null = null;

function loadTypst(): Promise<TypstGlobal> {
  if (typstReady) return typstReady;
  typstReady = new Promise<TypstGlobal>((resolve, reject) => {
    const script = document.createElement('script');
    script.type = 'module';
    script.src = BUNDLE_SRC;
    script.addEventListener('load', () => {
      const $typst = (window as unknown as { $typst: TypstGlobal }).$typst;
      $typst.setCompilerInitOptions({ getModule: () => COMPILER_WASM });
      $typst.setRendererInitOptions({ getModule: () => RENDERER_WASM });
      resolve($typst);
    });
    script.addEventListener('error', () => reject(new Error('Failed to load Typst bundle from CDN')));
    document.head.appendChild(script);
  });
  return typstReady;
}

export default function TypstViewer({ url }: Props) {
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function render() {
      try {
        const [source, $typst] = await Promise.all([
          fetch(url).then(r => r.text()),
          loadTypst(),
        ]);
        const result = await $typst.svg({ mainContent: source });
        if (!cancelled) setSvg(result);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    }
    render();
    return () => { cancelled = true; };
  }, [url]);

  if (error) {
    return (
      <div>
        <p style={{ color: '#888', fontSize: 13 }}>Typst rendering unavailable: {error}</p>
        <a href={url} target="_blank" rel="noreferrer">Download .typ source</a>
      </div>
    );
  }
  if (!svg) return <div>Compiling Typst…</div>;
  return <div dangerouslySetInnerHTML={{ __html: svg }} />;
}
