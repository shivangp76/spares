import { defineConfig } from 'vite'

export default defineConfig({
  assetsInclude: [
    'svgedit/src/editor/panels/*.html',
    'svgedit/src/editor/templates/*.html',
    'svgedit/src/editor/dialogs/*.html',
    'svgedit/src/editor/extensions/*/*.html',
  ],
  server: {
    open: '/svgedit/src/editor/index.html?storagePrompt=false',
    port: 5173,
  },
  plugins: [{
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
