/* eslint-disable no-console -- standalone browser regression runner. */
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
  const output = process.env.QA_OUTPUT_DIR || '/tmp/newcpr-fast-groups-qa'
  await mkdir(output, { recursive: true })
  const now = new Date().toISOString()
  const errors = []
  const unexpected = []
  const writes = []
  const group = {
    id: 'grp_00000000000000000000000000000001',
    name: 'Legacy Fast policy',
    description: 'Synthetic group',
    color: '#2563EBFF',
    enabled: true,
    disableFast: true,
    memberCount: 0,
    providerCounts: {},
    clientKeyCount: 0,
    accountSummary: { available: 0, limited: 0, total: 0 },
    capacity: { usedSlots: 0, totalSlots: 0 },
    usage: { todayUsd: '0', retainedTotalUsd: '0' },
    createdAt: now,
    updatedAt: now,
  }
  try {
    const page = await browser.newPage({ reducedMotion: 'reduce' })
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/dev/api/**', async (route) => {
      const request = route.request()
      const path = new URL(request.url()).pathname.replace(/^\/dev/u, '')
      let data
      if (path === '/api/admin/auth/refresh' || path === '/api/admin/auth/status') {
        data = { authenticated: true, expiresAt: new Date(Date.now() + 3600000).toISOString() }
      }
      else if (path === '/api/admin/account-groups') {
        data = { items: [group], page: { page: 1, pageSize: 20, total: 1, totalPages: 1 }, configRevision: 1 }
      }
      else if (path === '/api/admin/client-keys') {
        data = { items: [], nextCursor: null, configRevision: 1 }
      }
      else if (path === '/api/admin/system/version') {
        data = { version: 'local-test', buildType: 'test' }
      }
      else if (path === '/api/admin/account-groups/update' || path === '/api/admin/account-groups/create') {
        const body = request.postDataJSON()
        writes.push({ path, body })
        Object.assign(group, body, { disableFast: body.fastMode === 'disabled' })
        data = { id: group.id, record: group, configRevision: writes.length + 1 }
      }
      else {
        unexpected.push(`${request.method()} ${path}`)
        return route.abort()
      }
      return route.fulfill({ json: { code: 200, message: 'ok', data } })
    })
    const url = `${process.env.QA_BASE_URL || 'http://127.0.0.1:5187'}/account-groups`
    const choose = async (dialog, label) => {
      const trigger = dialog.getByRole('combobox', { name: 'Fast 模式', exact: true })
      await trigger.click()
      const listbox = page.locator(`[id="${await trigger.getAttribute('aria-controls')}"]`)
      await listbox.getByRole('option', { name: label, exact: true }).click()
    }
    for (const [name, width, height] of [['desktop', 1440, 1000], ['mobile', 390, 844]]) {
      delete group.fastMode
      group.disableFast = true
      await page.setViewportSize({ width, height })
      await page.goto(url)
      const row = page.getByRole('row').filter({ hasText: group.name })
      await row.getByText('关闭', { exact: true }).waitFor()
      await row.getByRole('button', { name: '编辑分组', exact: true }).click()
      const dialog = page.getByRole('dialog')
      const fast = dialog.getByRole('combobox', { name: 'Fast 模式', exact: true })
      assert.match(await fast.textContent(), /关闭/u)
      for (const [mode, label] of [['enabled', '开启'], ['disabled', '关闭'], ['default', '默认（跟随客户端）']]) {
        await choose(dialog, label)
        await page.screenshot({ path: `${output}/${name}-${mode}.png` })
        assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth), true)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
        const saved = page.waitForResponse(response => response.url().endsWith('/account-groups/update'))
        await dialog.getByRole('button', { name: '保存分组', exact: true }).click()
        await saved
        assert.equal(writes.at(-1).body.fastMode, mode)
        assert.equal(Object.hasOwn(writes.at(-1).body, 'disableFast'), false)
        await page.reload()
        await page.getByRole('button', { name: '编辑分组', exact: true }).click()
        assert.match(await fast.textContent(), new RegExp(label.replace(/[()（）]/gu, '.'), 'u'))
      }
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      await page.getByRole('button', { name: '创建分组', exact: true }).click()
      await dialog.getByRole('textbox', { name: '分组名称', exact: true }).fill('Created Fast policy')
      assert.match(await fast.textContent(), /默认/u)
      await choose(dialog, '开启')
      const created = page.waitForResponse(response => response.url().endsWith('/account-groups/create'))
      await dialog.getByRole('button', { name: '保存分组', exact: true }).click()
      await created
      assert.equal(writes.at(-1).body.fastMode, 'enabled')
      assert.equal(writes.at(-1).body.disableFast, false)
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [])
    console.log(JSON.stringify({ screenshots: output, writes: writes.length, errors }))
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
