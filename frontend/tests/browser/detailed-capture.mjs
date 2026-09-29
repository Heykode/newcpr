import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5193'
  const origin = `http://127.0.0.1:${port}`
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-detailed-capture-ui'
  await mkdir(output, { recursive: true })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 150; i++) {
      try {
        if ((await fetch(`${origin}/tests/fixtures/detailed-capture.html`)).ok) {
          ready = true
          break
        }
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 200))
    }
    assert.ok(ready, 'Preview server did not start')
    browser = await chromium.launch({ headless: true, channel: process.env.PLAYWRIGHT_CHANNEL })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 900 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      let config = { enabled: false, globalErrors: false, includeMedia: false, quotaMib: 1024, retentionDays: 7 }
      let lookups = 0
      let bodies = 0
      let posts = 0
      let rejectNextSave = true
      const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
      await page.route('**/dev/**', async (route) => {
        const url = new URL(route.request().url())
        if (url.pathname.endsWith('/request-captures/config')) {
          if (route.request().method() === 'POST') {
            posts++
            if (rejectNextSave) {
              rejectNextSave = false
              return route.fulfill({ status: 503, json: { code: 503, message: 'Fixture save rejected', data: null } })
            }
            config = route.request().postDataJSON()
            return fulfill(route, null)
          }
          return fulfill(route, { config, globalActive: config.enabled && config.globalErrors, storageFault: false, skipped: 0 })
        }
        if (url.pathname.endsWith('/request-captures/by-request')) {
          lookups++
          assert.equal(url.searchParams.get('requestId'), 'req_capture_fixture')
          return fulfill(route, [{
            id: 'capture-fixture',
            requestId: 'req_capture_fixture',
            taskId: 'global-fixture',
            bytes: 400,
            incomplete: true,
            createdAt: '2026-01-01T00:00:00Z',
          }])
        }
        if (url.pathname.endsWith('/request-captures/records')) {
          bodies++
          assert.equal(url.searchParams.get('id'), 'capture-fixture')
          const offset = Number(url.searchParams.get('offset'))
          return fulfill(route, {
            text: offset
              ? '{"stage":"downstream.body","body":{"error":"fixture"}}'
              : `{"stage":"client.request.body","body":{"input":"<script>window.captureExecuted=true</script>","long":"${'x'.repeat(300)}"}}`,
            nextOffset: offset ? null : 256,
          })
        }
        if (url.pathname.endsWith('/usage/records/detail'))
          return fulfill(route, { requestId: 'req_capture_fixture', attemptsComplete: true, attempts: [], trace: null })
        if (url.pathname.endsWith('/auth/status'))
          return fulfill(route, { authenticated: true })
        return route.fulfill({ status: 404, json: { code: 404, message: 'No fixture for this API' } })
      })
      await page.goto(`${origin}/tests/fixtures/detailed-capture.html?theme=${width === 390 ? 'dark' : 'light'}`)
      const toggle = page.getByRole('switch', { name: '详细错误采集（全局）' })
      await toggle.waitFor()
      await page.waitForFunction(() => !document.querySelector('input[role="switch"]')?.disabled)
      assert.equal(lookups, 0)
      assert.equal(bodies, 0)
      assert.equal(await toggle.isChecked(), false)
      await toggle.locator('..').click()
      await page.getByText('保存详细采集设置失败，设置未确认', { exact: true }).waitFor()
      await page.waitForFunction(() => !document.querySelector('input[role="switch"]')?.disabled)
      assert.equal(await toggle.isChecked(), false, 'failed saves must not leave the capture switch enabled')
      assert.equal(config.enabled, false)
      await toggle.locator('..').click()
      await page.waitForFunction(() => document.querySelector('input[role="switch"]')?.checked && !document.querySelector('input[role="switch"]')?.disabled)
      assert.equal(config.globalErrors, true)
      await page.getByRole('button', { name: '采集容量与保留时间', exact: true }).click()
      const settings = page.getByRole('dialog', { name: '详细错误采集', exact: true })
      await settings.waitFor()
      await settings.getByRole('spinbutton', { name: '采集文件总容量', exact: true }).fill('2048')
      await settings.getByRole('spinbutton', { name: '保留天数', exact: true }).fill('3')
      await page.screenshot({ path: `${output}/capture-settings-${width}.png`, fullPage: true })
      await settings.getByRole('button', { name: '保存', exact: true }).click()
      await settings.waitFor({ state: 'detached' })
      assert.equal(config.quotaMib, 2048)
      assert.equal(config.retentionDays, 3)
      assert.equal(config.includeMedia, false)
      assert.equal(posts, 3)
      assert.equal(bodies, 0)
      await page.getByRole('button', { name: '查看错误明细', exact: true }).click()
      const dialog = page.getByRole('dialog', { name: '错误明细', exact: true })
      await dialog.locator('pre').filter({ hasText: '<script>' }).waitFor()
      assert.equal(lookups, 1)
      assert.equal(bodies, 1)
      assert.equal(await page.evaluate(() => window.captureExecuted), undefined)
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/capture-detail-${width}.png`, fullPage: true })
      await dialog.getByRole('button', { name: '下一页', exact: true }).click()
      await dialog.locator('pre').filter({ hasText: 'downstream.body' }).waitFor()
      await dialog.getByRole('button', { name: '关闭', exact: true }).click()
      await dialog.waitFor({ state: 'detached' })
      assert.equal(await page.locator('pre').count(), 0)
      await toggle.locator('..').click()
      await page.waitForFunction(() => !document.querySelector('input[role="switch"]')?.checked && !document.querySelector('input[role="switch"]')?.disabled)
      assert.equal(config.enabled, false)
      await page.getByRole('button', { name: '查看错误明细', exact: true }).click()
      await dialog.locator('pre').filter({ hasText: '<script>' }).waitFor()
      assert.equal(bodies, 3, 'old captures remain available after collection is disabled')
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Detailed capture desktop/mobile checks passed\n')
  }
  finally {
    await browser?.close()
    if (server.exitCode === null) {
      const stopped = new Promise(resolve => server.once('exit', resolve))
      server.kill('SIGTERM')
      await stopped
    }
  }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
