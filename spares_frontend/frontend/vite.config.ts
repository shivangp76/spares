import { cpSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import wasm from 'vite-plugin-wasm'

const SVGEDIT_EDITOR_DIR = fileURLToPath(new URL('./svgedit/src/editor', import.meta.url))
const DIST_DIR = fileURLToPath(new URL('./dist', import.meta.url))

const IMAGE_OCCLUSION_TEMPLATE = fileURLToPath(new URL('../../spares_core/src/parsers/image_occlusion/template.svg', import.meta.url))

/** Serves the standalone image occlusion editor its config (see `ext-spares/config.js`).
    `spares_frontend --image-occlusion` sets it from the user's spares config. Otherwise, the editor
    gets the template and its own default settings. */
function imageOcclusionEditorConfig(): Plugin {
  return {
    name: 'image-occlusion-editor-config',
    configureServer(server) {
      server.middlewares.use('/__spares/image-occlusion-editor.json', (_req, res) => {
        const config = process.env.SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG
          ?? JSON.stringify({ template: readFileSync(IMAGE_OCCLUSION_TEMPLATE, 'utf8') })
        res.setHeader('Content-Type', 'application/json')
        res.setHeader('Cache-Control', 'no-store')
        res.end(config)
      })
    },
  }
}

/** svgedit fetches its icons, shape libraries, etc. by URL at runtime, so they are copied next to
    its page rather than bundled. Its scripts are bundled from the entry. */
function copySvgeditAssets(): Plugin {
  return {
    name: 'copy-svgedit-assets',
    apply: 'build',
    closeBundle() {
      for (const dir of ['images', 'extensions']) {
        cpSync(`${SVGEDIT_EDITOR_DIR}/${dir}`, `${DIST_DIR}/svgedit/src/editor/${dir}`, {
          recursive: true,
          filter: source => !source.endsWith('.js'),
        })
      }
    },
  }
}

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
  build: {
    rollupOptions: {
      // The image occlusion editor is its own page, embedded by the app in an iframe
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        svgedit: fileURLToPath(new URL('./svgedit/src/editor/index.html', import.meta.url)),
      },
    },
  },
  server: {
    open: process.env.SPARES_OPEN ?? '/',
    port: 5173,
  },
  plugins: [react(), wasm(), copySvgeditAssets(), imageOcclusionEditorConfig(), {
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
