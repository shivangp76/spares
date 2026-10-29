// pandoc's own WebAssembly build (`pandoc.wasm`, from its releases), run as pandoc's
// `wasm/pandoc.js` runs it: started once, then each conversion through its `convert` export, which
// takes the options a defaults file has, as JSON. Its files (stdin, stdout, stderr and the
// warnings) are in memory.
import {
  ConsoleStdout,
  File,
  OpenFile,
  PreopenDirectory,
  WASI,
} from "@bjorn3/browser_wasi_shim";

/** What pandoc printed for one conversion. `warnings` is pandoc's JSON log of them. */
export interface Converted {
  stdout: string;
  stderr: string;
  warnings: { type: string; verbosity: string; [key: string]: unknown }[];
}

export interface Pandoc {
  /** Converts `input` with `options`, as a defaults file has them. */
  convert(options: object, input: string): Converted;
  /** `pandoc --version`'s version, e.g. `3.12.1`. */
  version(): string;
}

interface Exports {
  memory: WebAssembly.Memory;
  malloc(size: number): number;
  __wasm_call_ctors(): void;
  hs_init_with_rtsopts(argc: number, argv: number): void;
  convert(options: number, length: number): void;
  query(options: number, length: number): void;
}

// Without the `+RTS -H64m` that pandoc's own pandoc.js passes, which more than doubles its heap
// for the same speed
const ARGS = ["pandoc.wasm"];

const decoder = new TextDecoder("utf-8", { fatal: true });
const encoder = new TextEncoder();

// Options that name files, which a device doesn't have: pandoc stops at one it can't read. So the
// web defaults file's Lua filters, stylesheets, templates and includes are left out.
const FILE_OPTIONS = [
  "abbreviations",
  "bibliography",
  "citation-abbreviations",
  "csl",
  "css",
  "data-dir",
  "embed-resources",
  "extract-media",
  "filters",
  "include-after-body",
  "include-before-body",
  "include-in-header",
  "input-file",
  "input-files",
  "log-file",
  "output-file",
  "reference-doc",
  "resource-path",
  "self-contained",
  "syntax-definition",
  "syntax-definitions",
  "template",
];

/**
 * The options the server renders a card's HTML for web clients with (`render_web_file` in
 * spares_core's Markdown parser), with `webDefaults`, its web defaults file, on top. Without
 * `--embed-resources`: the web app shows a card's images itself.
 */
export function cardOptions(
  webDefaults: Record<string, unknown> | null,
  pagetitle = "card",
): Record<string, unknown> {
  const defaults = { ...webDefaults };
  for (const option of FILE_OPTIONS) delete defaults[option];
  return {
    from: "markdown",
    to: "html5",
    standalone: true,
    "html-math-method": "mathml",
    ...defaults,
    metadata: {
      "document-css": false,
      pagetitle,
      ...(defaults.metadata as object | undefined),
    },
  };
}

/** Starts pandoc from its compiled module. */
export async function startPandoc(module: WebAssembly.Module): Promise<Pandoc> {
  const files = new Map<string, File>();
  const wasi = new WASI(
    ARGS,
    [],
    [
      new OpenFile(new File(new Uint8Array(), { readonly: true })),
      ConsoleStdout.lineBuffered((message) =>
        console.log(`[pandoc] ${message}`),
      ),
      ConsoleStdout.lineBuffered((message) =>
        console.warn(`[pandoc] ${message}`),
      ),
      new PreopenDirectory("/", files),
    ],
    { debug: false },
  );
  const instance = await WebAssembly.instantiate(module, {
    wasi_snapshot_preview1: wasi.wasiImport,
  });
  // `initialize` takes a reactor's exports, which pandoc's are
  wasi.initialize(
    instance as {
      exports: { memory: WebAssembly.Memory; _initialize?: () => unknown };
    },
  );
  const exports = instance.exports as unknown as Exports;
  exports.__wasm_call_ctors();

  const view = () => new DataView(exports.memory.buffer);
  const argc = exports.malloc(4);
  view().setUint32(argc, ARGS.length, true);
  const argv = exports.malloc(4 * (ARGS.length + 1));
  ARGS.forEach((arg, i) => {
    const bytes = encoder.encode(arg);
    const pointer = exports.malloc(bytes.length + 1);
    new Uint8Array(exports.memory.buffer, pointer, bytes.length).set(bytes);
    view().setUint8(pointer + bytes.length, 0);
    view().setUint32(argv + 4 * i, pointer, true);
  });
  view().setUint32(argv + 4 * ARGS.length, 0, true);
  const argvPointer = exports.malloc(4);
  view().setUint32(argvPointer, argv, true);
  exports.hs_init_with_rtsopts(argc, argvPointer);

  /** Calls `fn` (`convert` or `query`) with `options`, and returns the files it wrote. */
  const call = (fn: "convert" | "query", options: object, input: string) => {
    const bytes = encoder.encode(JSON.stringify(options));
    const pointer = exports.malloc(bytes.length);
    new Uint8Array(exports.memory.buffer, pointer, bytes.length).set(bytes);
    files.clear();
    const out = {
      stdout: new File(new Uint8Array()),
      stderr: new File(new Uint8Array()),
      warnings: new File(new Uint8Array()),
    };
    files.set("stdin", new File(encoder.encode(input), { readonly: true }));
    for (const [name, file] of Object.entries(out)) files.set(name, file);
    exports[fn](pointer, bytes.length);
    return {
      stdout: decoder.decode(out.stdout.data),
      stderr: decoder.decode(out.stderr.data),
      warnings: decoder.decode(out.warnings.data),
    };
  };

  return {
    convert(options, input) {
      const { stdout, stderr, warnings } = call("convert", options, input);
      return { stdout, stderr, warnings: warnings ? JSON.parse(warnings) : [] };
    },
    version() {
      return JSON.parse(call("query", { query: "version" }, "").stdout);
    },
  };
}
