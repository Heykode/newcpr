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
      name: 'isolated-location-qa',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-request-location-qa'
  await mkdir(output, { recursive: true })
  try {
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      const errors = []
      const writes = []
      let settings = {
        modelMappings: {},
        refreshMarginSeconds: 1800,
        refreshConcurrency: 4,
        maxConcurrentPerAccount: 5,
        requestIntervalMs: 0,
        rotationStrategy: 'smart',
        minCodexDesktopVersion: null,
        minCodexCliVersion: null,
        usageRetentionDays: 31,
        opsEventRetentionDays: 30,
        auditRetentionDays: 90,
        requestTuning: {},
        updatedAt: '2026-09-17T00:00:00Z',
      }
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const request = route.request()
        const url = new URL(request.url())
        if (url.origin !== base)
          return route.abort()
        const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
        if (!path.startsWith('/api/'))
          return route.continue()
        let data
        if (path === '/api/admin/settings/update') {
          settings = { ...settings, ...request.postDataJSON() }
          writes.push(settings)
          data = settings
        }
        else if (path === '/api/admin/proxies/create') {
          writes.push(request.postDataJSON())
          data = { record: {}, configRevision: 2 }
        }
        else if (path === '/api/admin/auth/status') {
          data = { authenticated: true }
        }
        else if (path === '/api/admin/settings') {
          data = settings
        }
        else if (path === '/api/admin/settings/admin-api-key') {
          data = { exists: false }
        }
        else if (path === '/api/admin/notifications/channels') {
          data = {
            smtp: { enabled: false, host: '', port: 587, security: 'startTls', passwordSet: false },
            bark: { enabled: false, serverUrl: '', deviceKeySet: false },
          }
        }
        else if (path === '/api/admin/settings/openai-user-agent') {
          data = {
            mode: 'default',
            defaultUserAgent: 'Synthetic/1.0',
            effectiveUserAgent: 'Synthetic/1.0',
            effectiveDesktopUserAgent: 'Synthetic/1.0',
            customUserAgent: null,
            coreVersion: '1.0.0',
            desktopVersion: '1.0.0',
            osType: 'Linux',
            osVersion: 'test',
            arch: 'x64',
            terminal: 'test',
            verified: false,
            defaultVerifiedAt: null,
          }
        }
        else if (path === '/api/admin/system/version') {
          data = { version: 'test', hasUpdate: false, deploymentMode: 'test' }
        }
        else if (path === '/api/admin/proxies') {
          data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 } }
        }
        else {
          errors.push(`unexpected ${request.method()} ${path}`)
          return route.fulfill({ status: 404, json: { message: 'Unexpected fixture route' } })
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      await page.goto(`${base}/settings`)
      const enabled = page.getByRole('switch', { name: 'OpenAI 搜索地区与时区覆盖', exact: true })
      await enabled.waitFor()
      assert.equal(await enabled.isChecked(), false)
      await enabled.setChecked(true, { force: true })
      await page.getByRole('switch', { name: '自定义全局地区（关闭时沿用启动配置）' }).setChecked(true, { force: true })
      await page.getByRole('textbox', { name: '城市', exact: true }).fill('Los Angeles')
      await page.getByRole('textbox', { name: '时区', exact: true }).fill('America/Los_Angeles')
      await page.getByRole('textbox', { name: '地区', exact: true }).fill('California')
      await Promise.all([
        page.waitForResponse(response => response.url().endsWith('/api/admin/settings/update')),
        page.getByRole('button', { name: '保存', exact: true }).click(),
      ])
      assert.equal(writes.at(-1).requestTuning.openaiRequestLocation.city, 'Los Angeles')
      await page.getByRole('textbox', { name: '城市', exact: true }).scrollIntoViewIfNeeded()
      await page.screenshot({ path: `${output}/settings-${width}.png` })
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))

      await page.goto(`${base}/proxies`)
      await page.getByRole('button', { name: '新增代理', exact: true }).click()
      await page.getByRole('textbox', { name: '代理名称' }).fill('Location fixture')
      await page.getByLabel('代理 URL', { exact: true }).fill('http://127.0.0.1:8080')
      await page.getByText('手动独立地区（地区总开关开启时生效）', { exact: true }).click()
      assert.equal(await page.getByRole('switch', { name: '手动独立地区（地区总开关开启时生效）' }).isChecked(), true)
      await page.getByRole('textbox', { name: '城市', exact: true }).fill('Tokyo')
      await page.screenshot({ path: `${output}/proxy-${width}.png` })
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      await page.getByRole('button', { name: '保存代理', exact: true }).click()
      await page.getByRole('heading', { name: '新增代理', exact: true }).waitFor({ state: 'hidden' })
      assert.equal(writes.at(-1).requestLocation.city, 'Tokyo')
      assert.equal(writes.at(-1).proxyUrl, 'http://127.0.0.1:8080')
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Location UI passed at 1440, 390 and 320 pixels; all API traffic was synthetic.\n')
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
