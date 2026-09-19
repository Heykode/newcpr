import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-state-proxy-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage()
  const errors = []
  page.on('pageerror', error => errors.push(error.stack))
  let catalogFailed = false
  let saved = {
    turnStateInjectionEnabled: true,
    turnStateModels: ['model-a'],
    turnStateProbeProxyId: null,
    modelMappings: {},
    refreshMarginSeconds: 1800,
    refreshConcurrency: 4,
    maxConcurrentPerAccount: 5,
    requestIntervalMs: 25,
    rotationStrategy: 'smart',
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
  }
  await page.route('**/dev/api/**', async (route) => {
    const url = new URL(route.request().url())
    const path = url.pathname.replace('/dev', '')
    let data = {}
    if (path === '/api/admin/auth/status') {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/system/version') {
      data = { version: '3.12.0', gitSha: 'qa', latestVersion: '3.12.0', hasUpdate: false }
    }
    else if (path === '/api/admin/settings') {
      data = saved
    }
    else if (path === '/api/admin/settings/update') {
      saved = route.request().postDataJSON()
      assert.equal(Object.hasOwn(saved, 'proxyUrl'), false)
      data = saved
    }
    else if (path === '/api/admin/proxies') {
      if (catalogFailed) {
        await route.fulfill({ status: 503, json: { code: 503, message: 'Unavailable' } })
        return
      }
      data = {
        items: [
          { id: 'proxy-good', name: 'Test proxy', lastTest: { success: true } },
          { id: 'proxy-untested', name: 'Untested proxy', lastTest: null },
        ],
        page: { page: 1, pageSize: 200, total: 2, totalPages: 1 },
      }
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  try {
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5208'}/settings`)
    const select = page.getByRole('combobox', { name: 'State 探测出口' })
    await select.waitFor()
    assert.match(await select.textContent(), /IPv6 池/)
    await select.click()
    assert.equal(await page.getByRole('option', { name: 'Untested proxy（未通过测试）' }).isDisabled(), true)
    await page.getByRole('option', { name: 'Test proxy', exact: true }).click()
    const firstSave = page.waitForResponse(response => response.url().endsWith('/settings/update'))
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await firstSave
    assert.equal(saved.turnStateProbeProxyId, 'proxy-good')
    await page.reload()
    await select.waitFor()
    await page.waitForFunction(() => document.querySelector('[aria-label="State 探测出口"]')?.textContent.includes('Test proxy'))
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      await select.scrollIntoViewIfNeeded()
      const box = await select.boundingBox()
      assert.ok(box.width > 50 && box.x >= 0 && box.x + box.width <= width)
      await page.screenshot({ path: `${output}/${width}.png`, fullPage: true })
    }
    catalogFailed = true
    await page.reload()
    await select.waitFor()
    await page.waitForFunction(() => document.querySelector('[aria-label="State 探测出口"]')?.textContent.includes('已保存代理'))
    const preserveSave = page.waitForResponse(response => response.url().endsWith('/settings/update'))
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await preserveSave
    assert.equal(saved.turnStateProbeProxyId, 'proxy-good')
    catalogFailed = false
    await page.getByRole('button', { name: '刷新代理列表' }).click()
    await page.waitForFunction(() => document.querySelector('[aria-label="State 探测出口"]')?.textContent.includes('Test proxy'))
    await select.click()
    await page.getByRole('option', { name: 'IPv6 池', exact: true }).click()
    const response = page.waitForResponse(response => response.url().endsWith('/settings/update'))
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await response
    assert.equal(saved.turnStateProbeProxyId, null)
    assert.deepEqual(errors, [])
    process.stdout.write('Passed: proxy selection, persistence, IPv6 reset, untested exclusion, catalog failure preservation, desktop/mobile geometry.\n')
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
