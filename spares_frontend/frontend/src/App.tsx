import { Navigate, Route, Routes } from 'react-router-dom'
import CardsPage from './pages/CardsPage'
import LoginPage from './pages/LoginPage'
import NotesPage from './pages/NotesPage'
import ReviewPage from './pages/ReviewPage'
import StatisticsPage from './pages/StatisticsPage'

export default function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route path="/notes" element={<NotesPage />} />
      <Route path="/cards" element={<CardsPage />} />
      <Route path="/review" element={<ReviewPage />} />
      <Route path="/statistics" element={<StatisticsPage />} />
      <Route path="*" element={<Navigate to="/review" replace />} />
    </Routes>
  )
}
