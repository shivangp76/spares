import { lazy, Suspense } from 'react'
import { Navigate, Route, Routes } from 'react-router-dom'

// Pages are loaded on demand so a page doesn't wait for the others' code, e.g. the note editor
const CardsPage = lazy(() => import('./pages/CardsPage'))
const KeywordsPage = lazy(() => import('./pages/KeywordsPage'))
const LoginPage = lazy(() => import('./pages/LoginPage'))
const NotesPage = lazy(() => import('./pages/NotesPage'))
const ParsersPage = lazy(() => import('./pages/ParsersPage'))
const ReviewPage = lazy(() => import('./pages/ReviewPage'))
const StatisticsPage = lazy(() => import('./pages/StatisticsPage'))
const TagsPage = lazy(() => import('./pages/TagsPage'))

export default function App() {
  return (
    <Suspense fallback={<div style={{ padding: 24 }}>Loading…</div>}>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/notes" element={<NotesPage />} />
        <Route path="/cards" element={<CardsPage />} />
        <Route path="/tags" element={<TagsPage />} />
        <Route path="/parsers" element={<ParsersPage />} />
        <Route path="/keywords" element={<KeywordsPage />} />
        <Route path="/review" element={<ReviewPage />} />
        <Route path="/statistics" element={<StatisticsPage />} />
        <Route path="*" element={<Navigate to="/review" replace />} />
      </Routes>
    </Suspense>
  )
}
