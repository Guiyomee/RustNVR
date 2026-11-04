import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import './index.css'
import { initTheme } from './lib/theme'

// Before the first render: no flash of the other theme.
initTheme()

const root = document.getElementById('root')
if (!root) throw new Error('#root element missing')
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
