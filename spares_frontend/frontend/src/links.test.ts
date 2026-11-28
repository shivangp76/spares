import { expect, test } from "bun:test";
import { linkedNoteName, openableUrl } from "./links";

test("openableUrl opens web pages and app files, never scripts", () => {
  const base = "https://spares.example/review";
  expect(openableUrl("https://en.wikipedia.org/wiki/Paris", base)).toBe(
    "https://en.wikipedia.org/wiki/Paris",
  );
  expect(openableUrl("/files/a/clip.mp3", base)).toBe(
    "https://spares.example/files/a/clip.mp3",
  );
  expect(openableUrl("mailto:a@example.com", base)).toBe(
    "mailto:a@example.com",
  );
  for (const href of [
    "javascript:alert(1)",
    " JavaScript:alert(1)",
    "java\tscript:alert(1)",
    "data:text/html,<script>1</script>",
    "vbscript:x",
    "file:///etc/passwd",
  ]) {
    expect(openableUrl(href, base)).toBeNull();
  }
});

test("linkedNoteName reads a linked note's ID from the path of its file, whatever its parser", () => {
  expect(linkedNoteName("/data/notes/markdown/abc.md")).toBe("abc");
  expect(linkedNoteName("/data/notes/typst/abd.typ")).toBe("abd");
  expect(linkedNoteName("/data/notes/latex-note/abe.tex")).toBe("abe");
  expect(linkedNoteName("abf.md")).toBe("abf");
  expect(linkedNoteName("/files/clip.mp3")).toBeNull();
  expect(linkedNoteName("https://example.com/notes.md")).toBeNull();
});
