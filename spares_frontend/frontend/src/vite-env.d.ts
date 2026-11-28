/// <reference types="vite/client" />

/** Where the build has pandoc.wasm (see pandoc-wasm.ts), e.g. `/pandoc/pandoc-3.12.1.wasm` */
declare const __PANDOC_WASM__: string;

/** Where the demo loads the Typst compiler's WebAssembly module from, or null to load the build's (see vite.config.ts) */
declare const __TYPST_COMPILER_CDN_URL__: string | null;
