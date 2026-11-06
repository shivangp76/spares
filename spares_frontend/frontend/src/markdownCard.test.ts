import { expect, test } from 'bun:test';
import { markdownCardHtml, markSpanLength } from './markdownCard';

test('clozes are marked, with the brackets in them paired', () => {
  expect(markdownCardHtml('[card-aaa-1 _____]{.mark} is in France.')).toBe('<p><mark>card-aaa-1 _____</mark> is in France.</p>\n');
  expect(markdownCardHtml('See [a [link](https://example.com) and *more*]{.mark}.'))
    .toBe('<p>See <mark>a <a href="https://example.com">link</a> and <em>more</em></mark>.</p>\n');
  expect(markSpanLength('[a \\] b]{.mark} rest')).toBe(15);
});

test('links and other spans are left alone', () => {
  expect(markdownCardHtml('[a link](https://example.com)')).toBe('<p><a href="https://example.com">a link</a></p>\n');
  expect(markSpanLength('[text]{.smallcaps}')).toBeNull();
  expect(markSpanLength('[unclosed{.mark}')).toBeNull();
});

test('the template metadata is removed', () => {
  const source = '---\ngeometry: margin=1in\nfontsize: 14pt\n---\n<!--- spares: body --->\nText\n';
  expect(markdownCardHtml(source)).not.toContain('geometry');
  expect(markdownCardHtml(source)).toContain('<p>Text</p>');
});

test('raw HTML blocks are kept and raw LaTeX ones dropped', () => {
  const html = markdownCardHtml('```{=latex}\n\\hrulefill\n```\n\n```{=html}\n<hr>\n```\n\nText\n');
  expect(html).not.toContain('hrulefill');
  expect(html).toContain('<hr>');
  expect(html).not.toContain('<code');
  expect(markdownCardHtml('```js\nlet x;\n```\n')).toContain('<code class="language-js">');
});

test('an image occlusion names its image by path', () => {
  expect(markdownCardHtml('![Heart](/cache/spares/image_occlusions_rendered/abc.png)\n'))
    .toBe('<p><img src="/cache/spares/image_occlusions_rendered/abc.png" alt="Heart"></p>\n');
});
