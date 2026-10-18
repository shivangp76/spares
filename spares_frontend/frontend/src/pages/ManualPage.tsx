import { useEffect, useMemo } from 'react';
import { Link, Navigate, useLocation, useNavigate, useParams } from 'react-router-dom';
import { Marked } from 'marked';
import Navbar from '../components/Navbar';
import { MANUAL_PAGES, headingAnchor, manualHref, manualPath } from '../help/pages';
import { useAuth } from '../hooks/useAuth';

// Bundled with the app, so the manual works offline and matches this version of spares
const FILES = import.meta.glob<string>('../manual/*.md', { query: '?raw', import: 'default', eager: true });

const escapeAttribute = (text: string) => text.replace(/&/g, '&amp;').replace(/"/g, '&quot;');

const marked = new Marked({
  renderer: {
    // Ids, so the help's links and the files' links to each other can go to a heading
    heading({ tokens, depth, text }) {
      return `<h${depth} id="${headingAnchor(text)}">${this.parser.parseInline(tokens)}</h${depth}>\n`;
    },
    link({ href, title, tokens }) {
      const target = manualHref(href);
      const external = /^https?:/.test(target);
      return `<a href="${escapeAttribute(target)}"${title ? ` title="${escapeAttribute(title)}"` : ''}`
        + `${external ? ' target="_blank" rel="noreferrer"' : ''}>${this.parser.parseInline(tokens)}</a>`;
    },
  },
});

/** The manual, at `/manual/<file>`. Readable without logging in, since the login page links to it. */
export default function ManualPage() {
  const { page = MANUAL_PAGES[0].file.replace(/\.md$/, '') } = useParams();
  const { hash } = useLocation();
  const navigate = useNavigate();
  const { credentials, logout } = useAuth();
  const file = `${page}.md`;
  const text = FILES[`../manual/${file}`];
  const html = useMemo(() => (text === undefined ? '' : marked.parse(text, { async: false })), [text]);

  useEffect(() => {
    const heading = hash && document.getElementById(decodeURIComponent(hash.slice(1)));
    if (heading) heading.scrollIntoView();
    else window.scrollTo(0, 0);
  }, [html, hash]);

  if (text === undefined) return <Navigate to={manualPath(MANUAL_PAGES[0].file)} replace />;

  // Links to other pages of the manual stay in the app instead of reloading it
  function followLink(e: React.MouseEvent) {
    const link = e.target instanceof Element ? e.target.closest('a') : null;
    const href = link?.getAttribute('href');
    if (!href?.startsWith('/') || e.ctrlKey || e.metaKey || e.shiftKey) return;
    e.preventDefault();
    navigate(href);
  }

  return (
    <div className="page">
      {credentials ? <Navbar onLogout={logout} /> : <div style={{ marginBottom: 16 }}><Link to="/login">Log in</Link></div>}
      <div className="manual">
        <nav className="manual-contents" aria-label="Pages of the manual">
          {MANUAL_PAGES.map(p =>
            p.file === file
              ? <span key={p.file} aria-current="page">{p.title}</span>
              : <Link key={p.file} to={manualPath(p.file)}>{p.title}</Link>
          )}
        </nav>
        <article className="manual-text" onClick={followLink} dangerouslySetInnerHTML={{ __html: html }} />
      </div>
    </div>
  );
}
