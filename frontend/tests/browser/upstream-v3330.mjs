import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-v3330', configResolved(config) { config.server.proxy = {} } }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  await mkdir(output, { recursive: true })
  const browser = await chromium.launch({ headless: true })
  try {
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      const errors = []
      const writes = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const request = route.request()
        const url = new URL(request.url())
        assert.equal(url.origin, base, 'only synthetic loopback API traffic is allowed')
        const path = url.pathname.replace(/^\/dev/, '')
        if (!path.startsWith('/api/'))
          return route.continue()
        if (request.method() !== 'GET')
          writes.push(path)
        let data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 } }
        if (path.endsWith('/auth/status')) {
          data = { authenticated: true }
        }
        else if (path.endsWith('/system/version')) {
          data = { version: 'synthetic', buildType: 'test' }
        }
        else if (path.endsWith('/settings')) {
          data = {
            rotationStrategy: 'smart',
            refreshMarginSeconds: 300,
            refreshConcurrency: 2,
            maxConcurrentPerAccount: 3,
            requestIntervalMs: 0,
            usageRetentionDays: 31,
            opsEventRetentionDays: 30,
            auditRetentionDays: 90,
            disableFast: false,
            requestTuning: {},
            modelMappings: {},
          }
        }
        else if (path.endsWith('/accounts')) {
          data = { items: accounts, page: { page: 1, pageSize: 20, total: 3, totalPages: 1 }, summary: { total: 3, normal: 3 } }
        }
        else if (path.endsWith('/accounts/query') || path.endsWith('/reset-credits/batches')) {
          data = []
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      await page.goto(`${base}/settings`)
      const interval = page.getByRole('spinbutton', { name: '请求间隔 ms', exact: true })
      await interval.waitFor()
      await interval.fill('123')
      await page.getByRole('radio', { name: '日志清理', exact: true }).click()
      const confirm = page.getByRole('alertdialog')
      await confirm.waitFor()
      await confirm.getByRole('button', { name: '继续编辑', exact: true }).click()
      assert.equal(new URL(page.url()).pathname, '/settings')
      assert.equal(await interval.inputValue(), '123')
      await page.getByRole('radio', { name: '日志清理', exact: true }).click()
      await confirm.waitFor()
      await page.keyboard.press('Escape')
      await confirm.waitFor({ state: 'hidden' })
      assert.equal(await interval.inputValue(), '123')
      await page.getByRole('radio', { name: '日志清理', exact: true }).click()
      await confirm.waitFor()
      await page.screenshot({ path: `${output}/unsaved-${width}.png`, fullPage: true })
      await confirm.getByRole('button', { name: '放弃并离开', exact: true }).click()
      await page.waitForURL('**/settings/cleanup')
      assert.deepEqual(writes, [])

      await page.goto(`${base}/accounts`)
      await page.getByRole('button', { name: '导入账号', exact: true }).waitFor()
      if (width < 961) {
        const toggle = page.getByRole('button', { name: '打开侧边栏', exact: true })
        await toggle.click()
        const sidebar = page.getByRole('dialog')
        await sidebar.waitFor()
        for (let i = 0; i < 30; i++) {
          await page.keyboard.press('Tab')
          assert.ok(await sidebar.evaluate(element => element.contains(document.activeElement)))
        }
        await page.screenshot({ path: `${output}/sidebar-${width}.png`, fullPage: true })
        await page.keyboard.press('Escape')
        await sidebar.waitFor({ state: 'hidden' })
        await page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === '打开侧边栏')
      }
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('PASS: unsaved edits, Escape cancellation, mobile focus loop/restore, desktop/mobile bounds.\n')
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
