import { Navigate, Route, Routes } from 'react-router-dom'
import CardsPage from './pages/CardsPage'
import KeywordsPage from './pages/KeywordsPage'
import LoginPage from './pages/LoginPage'
import NotesPage from './pages/NotesPage'
import ParsersPage from './pages/ParsersPage'
import ReviewPage from './pages/ReviewPage'
import StatisticsPage from './pages/StatisticsPage'
import TagsPage from './pages/TagsPage'

export default function App() {
  return (
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
  )
}
