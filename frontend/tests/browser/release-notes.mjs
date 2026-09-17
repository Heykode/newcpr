import assert from 'node:assert/strict'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'isolated-release-notes-qa',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  let browser
  try {
    browser = await chromium.launch({
      headless: true,
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    })
    const page = await browser.newPage()
    await page.route('**/*', route => new URL(route.request().url()).origin === base
      ? route.continue()
      : route.abort())
    // A blank same-origin document imports the real renderer without starting the admin app.
    await page.route(`${base}/qa-blank`, route => route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Release notes test</title><main></main>',
    }))
    await page.goto(`${base}/qa-blank`)
    const result = await page.evaluate(async () => {
      const { renderReleaseNotes } = await import('/src/layout/components/SystemUpdateModal/markdown.ts')
      const inputs = [
        '<script>globalThis.releaseNoteExecuted = true</script>',
        '<img src=x onerror="globalThis.releaseNoteExecuted = true">',
        '<svg onload="globalThis.releaseNoteExecuted = true"></svg>',
        '[unsafe](javascript:alert%281%29)',
        '<a href="jav&#x61;script:alert(1)">unsafe</a>',
        '<iframe srcdoc="<script>alert(1)</script>"></iframe>',
        '<math><mtext><table><mglyph><style><!--</style><img title="--><img src=x onerror=alert(1)>">',
      ]
      const bad = []
      for (const source of inputs) {
        const element = document.createElement('div')
        element.innerHTML = renderReleaseNotes(source)
        document.querySelector('main').append(element)
        if (element.querySelector('script, iframe, object, embed'))
          bad.push('active-element')
        for (const node of element.querySelectorAll('*')) {
          for (const attribute of node.attributes) {
            if (/^on/i.test(attribute.name)
              || (['href', 'src', 'xlink:href'].includes(attribute.name)
                && /^\s*javascript:/i.test(attribute.value))) {
              bad.push('active-attribute')
            }
          }
        }
        element.remove()
      }
      const safe = document.createElement('div')
      safe.innerHTML = renderReleaseNotes('# Release\n\n**Fixed** `code`\n\n[Details](https://example.com/notes)')
      const link = safe.querySelector('a')
      return {
        bad,
        executed: globalThis.releaseNoteExecuted === true,
        empty: [renderReleaseNotes(), renderReleaseNotes(null), renderReleaseNotes('  ')],
        heading: safe.querySelector('h1')?.textContent,
        strong: safe.querySelector('strong')?.textContent,
        code: safe.querySelector('code')?.textContent,
        link: [link?.href, link?.target, link?.rel],
      }
    })
    assert.deepEqual(result, {
      bad: [],
      executed: false,
      empty: ['', '', ''],
      heading: 'Release',
      strong: 'Fixed',
      code: 'code',
      link: ['https://example.com/notes', '_blank', 'noreferrer'],
    })
    process.stdout.write('Release notes: safe Markdown renders; active HTML and script URLs are removed.\n')
  }
  finally {
    await browser?.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
