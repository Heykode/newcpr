import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
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
      name: 'isolated-wire-profile-qa',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-wire-profile-qa'
  let browser
  try {
    await server.listen()
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({
      headless: true,
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    })
    await mkdir(output, { recursive: true })
    for (const theme of ['light', 'dark']) {
      for (const width of [1440, 390, 320]) {
        const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
        const errors = []
        page.on('pageerror', error => errors.push(error.message))
        await page.route('**/*', (route) => {
          const url = new URL(route.request().url())
          if (url.origin === base && !url.pathname.includes('/api/'))
            return route.continue()
          errors.push(`unexpected request: ${url.origin}${url.pathname}`)
          return route.abort()
        })
        await page.goto(`${base}/tests/fixtures/wire-profile-card.html?theme=${theme}`)
        await page.locator('[data-case="custom-failed"]').waitFor()
        await page.evaluate(() => document.fonts.ready)
        for (const name of ['custom-failed', 'custom-aligned']) {
          const card = page.locator(`[data-case="${name}"]`)
          assert.equal(await card.getByText('自定义 UA 生效', { exact: true }).count(), 1)
          assert.equal(await card.getByText('0.156.1', { exact: true }).count(), 1)
          assert.equal(await card.getByLabel('默认 Desktop 画像更新').count(), 1)
        }
        assert.equal(await page.locator('[data-case="custom-failed"]').getByText('默认画像检查失败', { exact: true }).count(), 1)
        assert.equal(await page.locator('[data-case="custom-failed"]').getByText('Codex Desktop artifact does not contain exactly one bundled Core', { exact: true }).count(), 1)
        assert.equal(await page.locator('[data-case="custom-aligned"]').getByText('默认画像制品一致', { exact: true }).count(), 1)
        assert.equal(await page.locator('[data-case="default-failed"]').getByText('检查失败', { exact: true }).count(), 1)
        assert.equal(await page.locator('[data-case="default-aligned"]').getByText('制品一致', { exact: true }).count(), 1)
        const overflow = await page.evaluate(() => {
          const nodes = [...document.querySelectorAll('[data-case], [aria-label="默认 Desktop 画像更新"], [aria-label="默认 Desktop 画像更新"] span')]
          return {
            page: document.documentElement.scrollWidth > window.innerWidth,
            nodes: nodes.filter(node => node.scrollWidth > node.clientWidth + 1).map(node => node.textContent),
          }
        })
        assert.equal(overflow.page, false, `${theme} ${width}: page overflow`)
        assert.deepEqual(overflow.nodes, [], `${theme} ${width}: clipped diagnostics`)
        assert.deepEqual(errors, [])
        await page.screenshot({ path: `${output}/${theme}-${width}.png`, fullPage: true })
        await page.close()
        process.stdout.write(`PASS ${theme} ${width}: active identity, diagnostics, no overflow\n`)
      }
    }
  }
  finally {
    await browser?.close()
    await server.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
