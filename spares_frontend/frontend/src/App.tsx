import { lazy, Suspense } from 'react'
import { Navigate, Outlet, Route, Routes } from 'react-router-dom'
import Navbar from './components/Navbar'
import { useAuth } from './hooks/useAuth'

// Pages are loaded on demand so a page doesn't wait for the others' code, e.g. the note editor
const CardsPage = lazy(() => import('./pages/CardsPage'))
const GraphPage = lazy(() => import('./pages/GraphPage'))
const ImageOcclusionPage = lazy(() => import('./pages/ImageOcclusionPage'))
const KeywordsPage = lazy(() => import('./pages/KeywordsPage'))
const LoginPage = lazy(() => import('./pages/LoginPage'))
const ManualPage = lazy(() => import('./pages/ManualPage'))
const NotesPage = lazy(() => import('./pages/NotesPage'))
const ParsersPage = lazy(() => import('./pages/ParsersPage'))
const ReviewPage = lazy(() => import('./pages/ReviewPage'))
const SettingsPage = lazy(() => import('./pages/SettingsPage'))
const StatisticsPage = lazy(() => import('./pages/StatisticsPage'))
const TagsPage = lazy(() => import('./pages/TagsPage'))

/** The navbar above every page but the login, kept between pages so its buttons don't flicker. */
function NavbarLayout() {
  const { credentials, logout } = useAuth()
  const page = <Suspense fallback={<div className="page">Loading…</div>}><Outlet /></Suspense>
  return credentials ? <Navbar onLogout={logout}>{page}</Navbar> : page
}

export default function App() {
  return (
    <Suspense fallback={<div style={{ padding: 24 }}>Loading…</div>}>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route element={<NavbarLayout />}>
          <Route path="/notes" element={<NotesPage />} />
          <Route path="/graph" element={<GraphPage />} />
          <Route path="/cards" element={<CardsPage />} />
          <Route path="/tags" element={<TagsPage />} />
          <Route path="/parsers" element={<ParsersPage />} />
          <Route path="/keywords" element={<KeywordsPage />} />
          <Route path="/image-occlusion" element={<ImageOcclusionPage />} />
          <Route path="/review" element={<ReviewPage />} />
          <Route path="/statistics" element={<StatisticsPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/manual/:page?" element={<ManualPage />} />
        </Route>
        <Route path="*" element={<Navigate to="/review" replace />} />
      </Routes>
    </Suspense>
  )
}
