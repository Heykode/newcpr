import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const output = process.env.QA_OUTPUT_DIR
  await mkdir(output, { recursive: true })
  const port = '5196'
  const base = `http://127.0.0.1:${port}`
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  let browser
  try {
    for (let i = 0; i < 150; i++) {
      try {
        if ((await fetch(`${base}/accounts`)).ok)
          break
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    browser = await chromium.launch({ headless: true })
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      page.setDefaultTimeout(30000)
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.origin, base, 'fixture must not contact any external backend')
        if (url.pathname === '/dev/api/admin/client-keys') {
          return route.fulfill({ json: { code: 200, message: 'ok', data: { items: [{
            id: 'synthetic-key',
            name: 'Synthetic key',
            label: null,
            prefix: 'qa',
            enabled: true,
            maxConcurrency: 2,
            requestsPerMinute: 10,
            dailyLimitUsd: '10',
            weeklyLimitUsd: '50',
            dailyUsedUsd: '1',
            weeklyUsedUsd: '2',
            dailyResetsAt: null,
            weeklyResetsAt: null,
            createdAt: '2026-01-01T00:00:00Z',
            updatedAt: '2026-01-01T00:00:00Z',
            lastUsedAt: null,
            routingScope: 'all',
            groups: [],
            providerKinds: ['openai'],
          }], nextCursor: null, total: 1 } } })
        }
        return route.continue()
      })
      for (const [path, endpoint, key] of [
        ['accounts', 'accounts', 'email'],
        ['api-keys', 'client-keys', 'name'],
      ]) {
        await page.goto(`${base}/${path}`)
        const storageKey = `codex-proxy:table-sort:${path}`
        if (width === 1440) {
          const query = page.waitForRequest((request) => {
            const url = new URL(request.url())
            return url.pathname === `/dev/api/admin/${endpoint}` && url.searchParams.get('sortBy') === key
          })
          await page.locator('th[data-column-key="identity"] [data-sort-button]').click()
          assert.equal(new URL((await query).url()).searchParams.get('sortDirection'), 'asc')
        }
        else {
          await page.evaluate(({ storageKey, key }) => localStorage.setItem(storageKey, JSON.stringify({ key, direction: 'asc' })), { storageKey, key })
        }
        const query = page.waitForRequest(request => new URL(request.url()).pathname === `/dev/api/admin/${endpoint}`)
        await page.reload()
        const params = new URL((await query).url()).searchParams
        assert.equal(params.get('sortBy'), key)
        assert.equal(params.get('sortDirection'), 'asc')
        if (width === 1440)
          assert.equal(await page.locator('th[data-column-key="identity"]').getAttribute('aria-sort'), 'ascending')
        await page.screenshot({ path: `${output}/${path}-sort-${width}.png`, fullPage: true })
      }
      await page.evaluate(() => localStorage.setItem('codex-proxy:table-sort:accounts', '{broken'))
      const fallback = page.waitForRequest(request => new URL(request.url()).pathname === '/dev/api/admin/accounts')
      await page.goto(`${base}/accounts`)
      const params = new URL((await fallback).url()).searchParams
      assert.equal(params.get('sortBy'), 'addedAt')
      assert.equal(params.get('sortDirection'), 'desc')
      assert.deepEqual(errors, [])
      await page.close()
      process.stdout.write(`PASS ${width}px: account and key sort persist; corrupt preference uses defaults\n`)
    }
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    await new Promise(resolve => server.exitCode !== null ? resolve() : server.once('exit', resolve))
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
