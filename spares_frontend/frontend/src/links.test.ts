import { expect, test } from "bun:test";
import { openableUrl } from "./links";

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
