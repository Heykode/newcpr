/* eslint-disable no-console -- standalone browser regression runner. */
import assert from 'node:assert/strict'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { fixtures, isolateNetwork } from './usage-columns.mjs'

async function main() {
  const base = new URL(process.env.QA_BASE_URL || 'http://127.0.0.1:5287')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/newcpr-v3280-usage-ui'
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const browser = await chromium.launch({ channel: 'chrome', headless: true })
  const report = { blocked: [], api: [], errors: [], calls: [], widths: [] }
  await mkdir(output, { recursive: true })
  try {
    for (const width of [1440, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 1000 }, serviceWorkers: 'block', reducedMotion: 'reduce' })
      await isolateNetwork(context, base, report)
      const data = fixtures()
      const request = data.get('/api/admin/usage/records').items[0]
      await context.route('**/*', async (route) => {
        const url = new URL(route.request().url())
        const path = url.pathname.replace(/^\/dev(?=\/api\/)/u, '')
        if (url.origin !== base.origin || !path.startsWith('/api/'))
          return route.fallback()
        const query = Object.fromEntries(url.searchParams)
        report.calls.push({ path, query })
        let result
        if (path === '/api/admin/usage/records') {
          const statuses = query.recordStatus === 'all' ? ['succeeded', 'failed', 'incomplete'] : [query.recordStatus === 'error' ? 'failed' : 'succeeded']
          result = { items: statuses.map(outcome => ({ ...request, id: outcome, outcome })), currentPage: 1, pageSize: 10, total: statuses.length }
        }
        else if (path === '/api/admin/operations/errors') {
          result = structuredClone(data.get(path))
          result.items[0].requestId = null
        }
        else if (path === '/api/admin/log-cleanup') {
          result = { tasks: [], policies: [] }
        }
        else if (path === '/api/admin/usage/diagnostics/export') {
          return route.fulfill({ json: { schemaVersion: 1, target: query, requests: [], gaps: [] } })
        }
        else {
          return route.fallback()
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data: result } })
      })
      const page = await context.newPage()
      page.setDefaultTimeout(15000)
      page.on('pageerror', error => report.errors.push(error.message))
      await page.goto(new URL('/usage', base).href)
      const selector = page.getByRole('radiogroup', { name: '请求状态', exact: true })
      await selector.waitFor()
      await page.locator('[data-column-key="outcome"]').filter({ hasText: '成功' }).first().waitFor()
      const summaries = () => report.calls.filter(call => call.path.endsWith('/summary')).length
      const before = summaries()
      for (const [label, status] of [['全部', 'all'], ['错误', 'error']]) {
        const response = page.waitForResponse(r => r.url().includes('/usage/records?') && r.url().includes(`recordStatus=${status}`))
        await selector.getByRole('radio', { name: label, exact: true }).click()
        await response
        await page.locator('[data-column-key="outcome"]').filter({ hasText: '失败' }).first().waitFor()
        assert.equal(report.calls.filter(call => call.path.endsWith('/usage/records')).at(-1).query.currentPage, '1')
        assert.equal(summaries(), before, 'status selection must not change cost summaries')
      }
      await page.screenshot({ path: `${output}/records-${width}.png`, fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      await page.getByRole('radio', { name: '错误排查', exact: true }).click()
      await page.getByRole('button', { name: '查看错误详情', exact: true }).first().click()
      const dialog = page.getByRole('dialog')
      const download = page.waitForEvent('download')
      await dialog.getByRole('button', { name: '导出诊断包', exact: true }).click()
      const artifact = await download
      const contents = JSON.parse(await readFile(await artifact.path(), 'utf8'))
      assert.deepEqual(contents.target, { errorId: 'ops_columns_1' })
      await page.screenshot({ path: `${output}/orphan-export-${width}.png` })
      await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
      await dialog.waitFor({ state: 'hidden' })
      await page.getByRole('button', { name: '查看错误详情', exact: true }).nth(1).click()
      const requestDownload = page.waitForEvent('download')
      await page.getByRole('button', { name: '导出诊断包', exact: true }).click()
      const requestArtifact = await requestDownload
      const requestContents = JSON.parse(await readFile(await requestArtifact.path(), 'utf8'))
      assert.deepEqual(requestContents.target, { requestId: 'req_columns_2' })
      report.widths.push(width)
      await context.close()
    }
    assert.deepEqual(report.errors, [])
    assert.deepEqual(report.blocked, [])
    await writeFile(`${output}/report.json`, JSON.stringify(report, null, 2))
    console.log(JSON.stringify({ widths: report.widths, screenshots: output, errors: report.errors }))
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
