import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import wasm from 'vite-plugin-wasm'
import topLevelAwait from 'vite-plugin-top-level-await'

export default defineConfig({
  optimizeDeps: {
    exclude: ['@myriaddreamin/typst.ts', '@myriaddreamin/typst-ts-renderer'],
  },
  assetsInclude: [
    'svgedit/src/editor/panels/*.html',
    'svgedit/src/editor/templates/*.html',
    'svgedit/src/editor/dialogs/*.html',
    'svgedit/src/editor/extensions/*/*.html',
  ],
  server: {
    open: process.env.SPARES_OPEN ?? '/',
    port: 5173,
  },
  plugins: [react(), wasm(), topLevelAwait(), {
    name: 'html-import-transformer',
    transform(code, id) {
      // Only transform JS/TS files
      if (!id.match(/\.(js|ts|jsx|tsx)$/)) return;

      // Regex to match import statements with .html files
      // This handles both single and double quotes
      const htmlImportRegex = /(import\s+[^'"`]*?from\s+['"`].*?)\.html(['"`])/g;

      // Replace all matches by adding ?raw before the closing quote
      const transformedCode = code.replace(htmlImportRegex, '$1.html?raw$2');

      // Only return if we made changes
      if (transformedCode !== code) {
        return {
          code: transformedCode,
          map: null
        };
      }
    }
  }],
})
