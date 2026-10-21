/**
 * Each page of a compiled document as an SVG of its own, or null for a single page. typst.ts draws
 * the pages one below the other as `.typst-page` groups, which share the glyphs and styles before
 * them, so each copy keeps those and its own page, with the view cropped to it.
 */
export function splitPages(svg: string): string[] | null {
  // Parsed as HTML, as the viewer shows it
  const template = document.createElement('template');
  template.innerHTML = svg;
  const root = template.content.querySelector('svg');
  if (!root) return null;
  const pages = [...root.children].filter(el => el.classList.contains('typst-page'));
  if (pages.length < 2) return null;
  pages.forEach(page => page.remove());
  // typst.ts's script for its own viewer, which an HTML parser never runs
  root.querySelectorAll('script').forEach(script => script.remove());
  return pages.map(page => {
    const [, x = '0', y = '0'] = /translate\(\s*([-\d.]+)[\s,]+([-\d.]+)\s*\)/.exec(page.getAttribute('transform') ?? '') ?? [];
    const width = page.getAttribute('data-page-width') ?? '0';
    const height = page.getAttribute('data-page-height') ?? '0';
    const copy = root.cloneNode(true) as Element;
    copy.append(page);
    copy.setAttribute('viewBox', `${x} ${y} ${width} ${height}`);
    for (const [name, value] of [['width', width], ['height', height]]) {
      copy.setAttribute(name, value);
      copy.setAttribute(`data-${name}`, value);
    }
    return copy.outerHTML;
  });
}
