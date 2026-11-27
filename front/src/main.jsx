import { render } from 'preact'
import CoreHero from '@/components/ui/syntra-hero'
import { Download, Flash, Architecture, Performance } from '@/components/sections'
import './index.css'

const App = () => (
  <>
    <CoreHero />
    <Download />
    <Flash />
    <Architecture />
    <Performance />
  </>
)

render(<App />, document.getElementById('app'))
