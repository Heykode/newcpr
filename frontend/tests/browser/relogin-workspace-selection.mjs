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
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-relogin-workspace-selection-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const mutations = []
  const longId = `workspace-${'b'.repeat(110)}`
  const longName = 'SharedBusinessWorkspace'.repeat(5)
  const choices = [
    { id: 'workspace-first', name: 'Team Alpha', planType: 'business' },
    { id: longId, name: longName, planType: 'business' },
  ]
  const rows = ['select', 'stale', 'failed'].map(id => ({
    id,
    revision: 7,
    email: `${id}@example.invalid`,
    hasTotp: true,
    automatic: true,
    status: 'awaiting_workspace',
    message: '有多个同级工作区，请选择后继续',
    workspaceChoices: choices,
    workspaceMode: 'highest',
    workspaceId: null,
    preferredWorkspaceId: 'workspace-previous',
    planType: null,
    pushTargets: [],
    credentialStatus: 'none',
    poolStatus: 'absent',
    poolAccountIds: [],
    poolAccounts: [],
    reloginCount: 0,
    lastReloginAt: null,
    reloginAccountId: null,
    verifiedAt: null,
    expiresAt: null,
    importedAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    recovery: { state: 'awaiting_workspace', retriesUsed: 0, maxRetries: 2 },
  }))
  page.on('pageerror', error => errors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'warning' && message.text().includes('[Vue warn]'))
      errors.push(message.text())
  })
  // This runner cannot contact a live backend, even if its default proxy is configured.
  await page.route('**/dev/api/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace('/dev', '')
    let data = null
    if (request.method() === 'POST') {
      assert.equal(path, '/api/admin/relogin/workspace/resume')
      const body = request.postDataJSON()
      mutations.push({ path, body })
      if (body.id === 'failed') {
        await route.fulfill({ status: 409, json: { code: 409, message: '原账号已变化，请重新获取工作区', data: null } })
        return
      }
      const row = rows.find(row => row.id === body.id)
      assert.equal(body.revision, row.revision)
      row.revision += 1
      row.status = 'queued'
      row.workspaceChoices = []
      row.recovery.state = 'queued'
    }
    else if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/relogin') {
      data = { items: rows, settings: { concurrency: 1, paused: false, maxRetries: 2, retryIntervalMinutes: 5 } }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'test', buildType: 'test' }
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  const row = id => page.locator(`tr[data-row-key="${id}"]`)
  const dialog = page.getByRole('dialog', { name: '选择登录工作区', exact: true })
  const submit = () => dialog.getByRole('button', { name: '选择并继续', exact: true })
  const open = async (id) => {
    await row(id).getByRole('button', { name: '待选择工作区', exact: true }).click()
    await dialog.waitFor()
  }
  const choose = async (name) => {
    await dialog.getByRole('combobox', { name: '登录工作区' }).click()
    await page.getByRole('option').filter({ hasText: name }).click()
  }
  const close = async () => {
    await dialog.getByRole('button', { name: '关闭', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
  }
  try {
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5245'}/relogin`)
    await open('select')
    assert.ok(await submit().isDisabled(), 'no candidate is implicitly selected')
    assert.equal(await dialog.getByRole('textbox').count(), 0, 'no free-form workspace ID')
    await close()
    assert.equal(mutations.length, 0, 'cancel sends no request')
    await open('select')
    await choose(longName)
    await dialog.locator('dd').filter({ hasText: longId }).waitFor()
    assert.equal(await dialog.locator('dd').filter({ hasText: longName }).count(), 1)
    assert.equal(await dialog.getByText('BUSINESS', { exact: true }).count(), 1)
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      const box = await dialog.boundingBox()
      assert.ok(box.x >= 0 && box.x + box.width <= width + 1)
      assert.ok(await dialog.evaluate(node => node.scrollWidth <= node.clientWidth + 1))
      assert.ok(await dialog.locator('dd, [role="combobox"]').evaluateAll(nodes => nodes.every((node) => {
        const rect = node.getBoundingClientRect()
        const panel = node.closest('[role="dialog"]').getBoundingClientRect()
        return node.scrollWidth <= node.clientWidth + 1 && rect.left >= panel.left && rect.right <= panel.right
      })), 'content must fit inside the modal, not just its own oversized box')
      await page.screenshot({ path: `${output}/selection-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    // A synchronous double click must submit once; no push/queue follow-up is allowed.
    await submit().evaluate((button) => {
      button.click()
      button.click()
    })
    await dialog.waitFor({ state: 'hidden' })
    await row('select').getByText('排队中', { exact: true }).waitFor()
    assert.deepEqual(mutations, [{
      path: '/api/admin/relogin/workspace/resume',
      body: { id: 'select', revision: 7, workspaceId: longId },
    }])
    assert.ok(await row('select').getByRole('button', { name: '推送', exact: true }).isDisabled())

    // A background list refresh must not silently replace the confirmation version.
    await open('stale')
    await choose('Team Alpha')
    rows.find(row => row.id === 'stale').revision += 1
    await dialog.getByText('账号状态已更新，请关闭后重新选择。', { exact: true }).waitFor({ timeout: 12000 })
    assert.ok(await submit().isDisabled())
    assert.equal(mutations.length, 1)
    await page.keyboard.press('Escape')
    await dialog.waitFor({ state: 'hidden' })
    await open('stale')
    assert.ok(await submit().isDisabled())
    await choose('Team Alpha')
    await submit().click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(mutations.at(-1).body, { id: 'stale', revision: 8, workspaceId: 'workspace-first' })

    await open('failed')
    await choose('Team Alpha')
    await submit().click()
    await dialog.getByRole('alert').filter({ hasText: '原账号已变化' }).waitFor()
    assert.ok(await dialog.isVisible())
    await page.waitForTimeout(5500)
    assert.equal(mutations.length, 3, 'polling never retries a failed continuation')
    assert.ok(await dialog.getByRole('alert').isVisible())
    await close()
    assert.deepEqual(errors, [])
    console.log(JSON.stringify({ result: 'passed', output, assertions: 'explicit choice, cancel, exact revision, no push, double click, stale candidates, failed save, desktop/mobile' }))
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    throw error
  }
  finally { await browser.close() }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
