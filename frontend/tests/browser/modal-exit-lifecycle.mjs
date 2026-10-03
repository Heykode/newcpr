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
    plugins: [{ name: 'isolated-modal-lifecycle', configResolved: config => config.server.proxy = {} }],
  })
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-modal-lifecycle-qa'
  await mkdir(output, { recursive: true })
  const errors = []
  const unexpected = []
  const row = {
    id: 'key-example',
    name: 'example-key',
    label: null,
    prefix: 'example',
    enabled: true,
    maxConcurrency: 2,
    requestsPerMinute: 3,
    dailyLimitUsd: '1',
    weeklyLimitUsd: '5',
    dailyUsedUsd: '0',
    weeklyUsedUsd: '0',
    dailyResetsAt: null,
    weeklyResetsAt: null,
    createdAt: '2026-10-01T00:00:00Z',
    updatedAt: '2026-10-01T00:00:00Z',
    lastUsedAt: null,
    routingScope: 'all',
    groups: [],
    providerKinds: ['openai'],
  }
  try {
    await server.listen()
    const address = server.httpServer.address()
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'no-preference' })
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/dev/api/**', (route) => {
        const path = new URL(route.request().url()).pathname.replace(/^\/dev/, '')
        const data = {
          '/api/admin/auth/status': { authenticated: true },
          '/api/admin/system/version': { version: 'synthetic', buildType: 'test' },
          '/api/admin/client-keys': { items: [row], nextCursor: null, total: 1 },
          '/api/admin/account-groups': { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } },
          '/api/admin/client-keys/reveal': { id: row.id, plaintextKey: 'synthetic-revealed-plaintext' },
        }[path]
        if (data)
          return route.fulfill({ json: { code: 200, message: 'ok', data } })
        unexpected.push(path)
        return route.fulfill({ status: 405, json: { code: 405, message: 'Unsupported synthetic operation', data: null } })
      })
      await page.goto(`http://127.0.0.1:${address.port}/api-keys`)
      const edit = page.getByRole('button', { name: '编辑密钥', exact: true })
      await edit.click()
      const dialog = page.getByRole('dialog')
      await dialog.getByLabel('名称', { exact: true }).fill('exit-frame-name')
      await page.waitForTimeout(220)
      await dialog.getByRole('button', { name: '关闭', exact: true }).evaluate(button => button.click())
      const leaving = page.locator('.cp-modal-leave-active')
      await leaving.waitFor({ state: 'attached' })
      assert.equal(await leaving.getByLabel('名称', { exact: true }).inputValue(), 'exit-frame-name')
      assert.equal(await leaving.getByRole('heading').textContent(), '编辑 API Key')
      await page.screenshot({ path: `${output}/exit-frame-${width}.png` })
      await edit.evaluate(button => button.click())
      await page.waitForTimeout(260)
      assert.equal(await dialog.getByLabel('名称', { exact: true }).inputValue(), row.name)
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      await dialog.waitFor({ state: 'detached' })
      await page.getByRole('button', { name: '使用密钥', exact: true }).click()
      await dialog.getByText(/synthetic-revealed-plaintext/).first().waitFor()
      const bounds = await dialog.boundingBox()
      assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.waitForTimeout(220)
      await dialog.getByRole('button', { name: '关闭', exact: true }).first().evaluate(button => button.click())
      await leaving.waitFor({ state: 'attached' })
      assert.match(await leaving.textContent(), /synthetic-revealed-plaintext/)
      await dialog.waitFor({ state: 'detached' })
      assert.equal(await page.getByText(/synthetic-revealed-plaintext/).count(), 0)
      await page.close()
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [])
    process.stdout.write('Passed: real modal leave frames, rapid reopen and plaintext removal at 1440/390px; synthetic API only.\n')
  }
  finally {
    await browser.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
