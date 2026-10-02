import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import CardTools from '../components/CardTools';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';

export default function CardsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();

  useEffect(() => {
    if (!credentials) navigate('/login');
  }, [credentials, navigate]);

  return (
    <div style={{ maxWidth: 800, margin: '0 auto', padding: 24 }}>
      <Navbar onLogout={logout} />
      <h2 style={{ marginBottom: 16 }}>Cards</h2>
      <CardTools />
    </div>
  );
}
