import { Navigate, Route, Routes } from 'react-router-dom'
import LoginPage from './pages/LoginPage'
import NotesPage from './pages/NotesPage'
import ReviewPage from './pages/ReviewPage'

export default function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route path="/notes" element={<NotesPage />} />
      <Route path="/review" element={<ReviewPage />} />
      <Route path="*" element={<Navigate to="/review" replace />} />
    </Routes>
  )
}
