import { createHash } from 'node:crypto'
import { cpSync, createReadStream, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, extname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import wasm from 'vite-plugin-wasm'

const SVGEDIT_EDITOR_DIR = fileURLToPath(new URL('./svgedit/src/editor', import.meta.url))
const DIST_DIR = fileURLToPath(new URL('./dist', import.meta.url))

const IMAGE_OCCLUSION_TEMPLATE = fileURLToPath(new URL('../../spares_core/src/parsers/image_occlusion/template.svg', import.meta.url))

const BACKGROUND_PATH = '/__spares/background'
const IMAGE_TYPES: Record<string, string> = {
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.bmp': 'image/bmp',
}

/** Serves the standalone image occlusion editor its config (see `ext-spares/config.js`).
    `spares_frontend image-occlusion` sets it from the user's spares config, along with the image
    to open, if any. Otherwise, the editor gets the template and its own default settings. */
function imageOcclusionEditorConfig(): Plugin {
  return {
    name: 'image-occlusion-editor-config',
    configureServer(server) {
      const background = process.env.SPARES_IMAGE_OCCLUSION_BACKGROUND
      server.middlewares.use('/__spares/image-occlusion-editor.json', (_req, res) => {
        const config = process.env.SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG
          ? JSON.parse(process.env.SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG)
          : { template: readFileSync(IMAGE_OCCLUSION_TEMPLATE, 'utf8') }
        if (background) config.background = { url: BACKGROUND_PATH, name: basename(background) }
        res.setHeader('Content-Type', 'application/json')
        res.setHeader('Cache-Control', 'no-store')
        res.end(JSON.stringify(config))
      })
      // Only this one file is served, not the directory it is in
      server.middlewares.use(BACKGROUND_PATH, (_req, res) => {
        if (!background) {
          res.statusCode = 404
          res.end()
          return
        }
        res.setHeader('Content-Type', IMAGE_TYPES[extname(background).toLowerCase()] ?? 'application/octet-stream')
        res.setHeader('Cache-Control', 'no-store')
        createReadStream(background)
          .on('error', () => { res.statusCode = 404; res.end() })
          .pipe(res)
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

const SERVICE_WORKER_TEMPLATE = fileURLToPath(new URL('./service-worker.js', import.meta.url))

function listFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const path = join(dir, entry.name)
    return entry.isDirectory() ? listFiles(path) : [path]
  })
}

/** Writes `dist/sw.js` from service-worker.js, with the files to cache for offline use. These are
    the page and everything it may load except the image occlusion editor and WebAssembly modules,
    which are large and only cached once used. */
function serviceWorker(): Plugin {
  return {
    name: 'service-worker',
    apply: 'build',
    // After `copySvgeditAssets`, so the build hash covers every file
    closeBundle: {
      order: 'post',
      handler() {
        const files = listFiles(DIST_DIR)
          .map(path => relative(DIST_DIR, path).split(sep).join('/'))
          .filter(path => path !== 'sw.js')
          .sort()
        const template = readFileSync(SERVICE_WORKER_TEMPLATE, 'utf8')
        const hash = createHash('sha256').update(template)
        for (const path of files) hash.update(path).update(readFileSync(join(DIST_DIR, path)))
        const precache = files.filter(path => !path.startsWith('svgedit/') && !path.endsWith('.wasm'))
        const source = template
          .replace('__BUILD__', JSON.stringify(hash.digest('hex').slice(0, 16)))
          .replace('__PRECACHE__', JSON.stringify(precache, null, 2))
        writeFileSync(join(DIST_DIR, 'sw.js'), source)
      },
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
  plugins: [react(), wasm(), copySvgeditAssets(), serviceWorker(), imageOcclusionEditorConfig(), {
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
