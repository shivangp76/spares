import { Link, useLocation } from 'react-router-dom';

interface NavbarProps {
  onLogout: () => void;
  extra?: React.ReactNode;
}

const NAV_LINKS = [
  { to: '/review', label: 'Review' },
  { to: '/notes', label: 'Notes' },
];

export default function Navbar({ onLogout, extra }: NavbarProps) {
  const { pathname } = useLocation();
  return (
    <nav style={{ marginBottom: 20, display: 'flex', gap: 16, alignItems: 'center', flexWrap: 'wrap' }}>
      {NAV_LINKS.map(({ to, label }) =>
        pathname === to ? (
          <span key={to} style={{ color: '#999', cursor: 'default' }}>{label}</span>
        ) : (
          <Link key={to} to={to}>{label}</Link>
        )
      )}
      <a href="/svgedit/src/editor/index.html?storagePrompt=false">Image Occlusion Editor</a>
      <div style={{ marginLeft: 'auto', display: 'flex', gap: 16, alignItems: 'center' }}>
        {extra}
        <button onClick={onLogout}>Logout</button>
      </div>
    </nav>
  );
}
