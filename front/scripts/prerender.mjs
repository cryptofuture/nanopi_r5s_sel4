// Injects the server-rendered app into dist/index.html so crawlers and
// first paint see real content before the client bundle hydrates it.
import { readFile, writeFile } from 'node:fs/promises'
import { renderPage } from '../dist/entry-server.js'

const distIndex = new URL('../dist/index.html', import.meta.url)
const marker = '<div id="app"></div>'
const html = await readFile(distIndex, 'utf8')

if (!html.includes(marker)) {
  console.error(`prerender: marker "${marker}" not found in dist/index.html`)
  process.exit(1)
}

const app = renderPage()
const out = html.replace(marker, () => `<div id="app">${app}</div>`)

await writeFile(distIndex, out)
console.log(`prerender: injected ${app.length} bytes of markup into dist/index.html`)
