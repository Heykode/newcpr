import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const port = '5197'
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-row-refresh'
  await mkdir(output, { recursive: true })
  let browser
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/accounts`)).ok)
          break
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ viewport: { width, height: 900 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      const rows = structuredClone(accounts)
      let listReads = 0
      let quotaWrites = 0
      const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
      await page.route('**/dev/api/admin/accounts?*', (route) => {
        listReads++
        return fulfill(route, {
          items: rows,
          page: { page: 1, pageSize: 20, total: rows.length, totalPages: 1 },
          summary: { total: rows.length, normal: rows.length, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
        })
      })
      await page.route('**/dev/api/admin/accounts/quota/refresh', (route) => {
        assert.equal(route.request().postDataJSON().accountId, rows[0].id)
        quotaWrites++
        rows[0].quota.refreshedAtDisplay = `refresh-complete-${quotaWrites}`
        return fulfill(route, { account: rows[0] })
      })
      await page.goto(`http://127.0.0.1:${port}/accounts`)
      const row = page.locator(`tr[data-row-key="${rows[0].id}"]`)
      await row.waitFor()
      await row.locator('button[title="展开统计"]').click()
      const refresh = page.getByRole('button', { name: '刷新额度', exact: true })
      await refresh.waitFor()
      const originalRow = await row.elementHandle()
      const reads = listReads
      await refresh.click()
      await page.getByText('最近刷新: refresh-complete-1', { exact: false }).waitFor()
      assert.equal(quotaWrites, 1)
      assert.equal(listReads, reads, 'quota refresh must not issue another full-list request')
      assert.equal(await originalRow.evaluate(element => element.isConnected), true)
      assert.equal(await row.locator('button[title="收起统计"]').count(), 1)
      await refresh.scrollIntoViewIfNeeded()
      await page.screenshot({ path: `${output}/account-row-refresh-${width}.png` })
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Account row refresh: desktop and narrow viewport passed\n')
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    await new Promise(resolve => server.exitCode !== null ? resolve() : server.once('exit', resolve))
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
