// pandoc.wasm converts a card as the server renders it (`render_web_file` in spares_core's Markdown
// parser), when both are the same version. Needs pandoc.wasm downloaded by a build, and the
// `pandoc` of the server's version (`PANDOC_VERSION` in scripts/setup_vps.sh), or it is skipped.
import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parse } from "yaml";
import { cachedPandocWasm, pandocVersion } from "../../pandoc-wasm";
import { cardOptions, startPandoc } from "./pandocWasm";

const version = pandocVersion();
const wasmPath = cachedPandocWasm(version);
const cliVersion = spawnSync("pandoc", ["--version"], {
  encoding: "utf8",
}).stdout?.split("\n")[0];
const runnable = existsSync(wasmPath) && cliVersion === `pandoc ${version}`;

const CARDS = [
  // As the template has it, with a cloze, math, a table and raw blocks
  `---
geometry: margin=1in
output: pdf_document
documentclass: extarticle
fontsize: 14pt
---
## Eigenvalues

For a square matrix $A$, a scalar $\\lambda$ is an eigenvalue if
$$\\det(A - \\lambda I) = 0.$$

The [characteristic polynomial]{.mark} of $A$ has degree $n$.

| Matrix | Eigenvalues |
|--------|-------------|
| $I$    | $1$         |

\`\`\`{=latex}
\\vspace{1em}
\`\`\`

\`\`\`{=html}
<hr>
\`\`\`
`,
  '[See [the note][li1] and *this*]{.mark}.\n\n[li1]: /cache/markdown/raw/notes/abc.md\n\n```python\nprint("hi")\n```\n',
  'Footnote[^1] and smart "quotes" -- dashes.\n\n[^1]: The note.\n\n::: {.columns}\nA div\n:::\n',
];

// A web defaults file, parsed for the device as pandoc.worker.ts does
const WEB_DEFAULTS_YAML =
  "number-sections: true\nmetadata:\n  lang: fr\n  document-css: true\n";
const WEB_DEFAULTS = parse(WEB_DEFAULTS_YAML) as Record<string, unknown>;

/**
 * What the server writes for `source`, with its arguments but `--embed-resources`, which the device
 * leaves off (it also minifies pandoc's styles and writes some characters as entities).
 */
function serverHtml(
  dir: string,
  source: string,
  webDefaults: string | null,
): string {
  const input = join(dir, "card.md");
  const output = join(dir, "card.html");
  writeFileSync(input, source);
  const args = [
    input,
    "-o",
    output,
    "-t",
    "html5",
    "--standalone",
    "--math-method=mathml",
    "--metadata=document-css=false",
    "--metadata",
    "pagetitle=card",
  ];
  if (webDefaults !== null) {
    writeFileSync(join(dir, "web-defaults.yaml"), webDefaults);
    args.push("--defaults", join(dir, "web-defaults.yaml"));
  }
  const result = spawnSync("pandoc", args, { encoding: "utf8" });
  expect(result.status, result.stderr).toBe(0);
  return readFileSync(output, "utf8");
}

test.skipIf(!runnable)(
  "pandoc.wasm converts cards as the server renders them",
  async () => {
    const pandoc = await startPandoc(
      await WebAssembly.compile(readFileSync(wasmPath)),
    );
    expect(pandoc.version()).toBe(version);
    const dir = mkdtempSync(join(tmpdir(), "spares-pandoc-"));
    try {
      for (const source of CARDS) {
        const html = pandoc.convert(cardOptions(null), source).stdout;
        expect(html).toBe(serverHtml(dir, source, null));
        expect(pandoc.convert(cardOptions(WEB_DEFAULTS), source).stdout).toBe(
          serverHtml(dir, source, WEB_DEFAULTS_YAML),
        );
      }
      const first = pandoc.convert(cardOptions(null), CARDS[0]).stdout;
      expect(first).toContain('<math display="block"');
      expect(first).toContain("<mark>characteristic polynomial</mark>");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  },
  60_000,
);

test("options naming files are left out of the web defaults", () => {
  const options = cardOptions({
    filters: ["a.lua"],
    css: ["a.css"],
    toc: true,
    metadata: { lang: "de" },
  });
  expect(options).not.toHaveProperty("filters");
  expect(options).not.toHaveProperty("css");
  expect(options).toMatchObject({
    toc: true,
    to: "html5",
    metadata: { lang: "de", "document-css": false, pagetitle: "card" },
  });
});
