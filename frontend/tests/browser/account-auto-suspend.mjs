import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-auto-suspend-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ reducedMotion: 'reduce' })
  const errors = []
  const mutations = []
  let recovered = false
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/dev/api/admin/accounts?*', async (route) => {
    if (route.request().method() !== 'GET') {
      mutations.push(route.request().method())
      return route.abort()
    }
    const items = accounts.map((account, index) => ({
      ...account,
      enabled: index !== 2,
      status: index === 2 ? 'disabled' : index === 1 && !recovered ? 'error' : 'normal',
      errorReason: index === 1 && !recovered ? 'credential_expired' : null,
    }))
    await route.fulfill({ json: {
      code: 200,
      message: 'ok',
      data: {
        items,
        page: { page: 1, pageSize: 20, total: 3, totalPages: 1 },
        summary: { total: 3, normal: recovered ? 2 : 1, error: recovered ? 0 : 1, disabled: 1, rateLimited: 0, quotaExhausted: 0 },
      },
    } })
  })
  try {
    const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5198'
    await page.goto(`${base}/accounts`)
    const cells = page.locator('td[data-column-key="status"]')
    await cells.first().waitFor()
    assert.equal(await cells.nth(0).getByRole('switch').isChecked(), true)
    assert.equal(await cells.nth(1).getByRole('switch').isChecked(), false)
    assert.equal(await cells.nth(1).getByRole('switch').isDisabled(), true)
    assert.match(await cells.nth(1).textContent(), /自动停调/)
    assert.equal(await cells.nth(2).getByRole('switch').isChecked(), false)
    assert.equal(await cells.nth(2).getByRole('switch').isDisabled(), false)
    assert.match(await cells.nth(2).textContent(), /暂停/)
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 900 })
      if (await page.locator('html').getAttribute('data-theme') !== theme) {
        await page.getByRole('button', {
          name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式',
          exact: true,
        }).click()
      }
      await page.waitForFunction(value => document.documentElement.dataset.theme === value, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 })
        await cells.nth(1).evaluate(element => element.scrollIntoView({ block: 'center', inline: 'center' }))
        assert.ok(await cells.nth(1).evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        await page.screenshot({ path: `${output}/${theme}-${width}.png`, fullPage: true })
      }
    }
    recovered = true
    await page.getByRole('button', { name: '刷新账号列表', exact: true }).click()
    await page.waitForFunction(() => {
      const cells = document.querySelectorAll('td[data-column-key="status"]')
      return cells[1]?.querySelector('input')?.checked === true
    })
    assert.equal(await cells.nth(2).getByRole('switch').isChecked(), false)
    assert.deepEqual(errors, [])
    assert.deepEqual(mutations, [])
    process.stdout.write('Passed: automatic stop, recovery, manual pause and desktop/mobile layout.\n')
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
