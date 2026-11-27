import { render } from 'preact-render-to-string'
import { App } from '@/App'

export function renderPage () {
  return render(<App />)
}
