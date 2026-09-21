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
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-relogin-workspace-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const mutations = []
  const target = (accountId, planType, workspaceId, switchWorkspace = true, available = true) =>
    ({ accountId, planType, workspaceId, switchWorkspace, available })
  const rows = [
    ['single', [target('free-row', 'free', 'workspace-free')]],
    ['multiple', [target('free-1', 'free', 'workspace-free-1'), target('free-2', 'free', 'workspace-free-2')]],
    ['destination', [target('old-free', 'free', 'workspace-free', true, false), target('business-row', 'team', 'workspace-business', false)]],
  ].map(([id, pushTargets]) => ({
    id,
    revision: 7,
    email: `${id}@example.invalid`,
    hasTotp: true,
    automatic: true,
    status: 'ready',
    message: '',
    planType: 'team',
    workspaceId: 'workspace-business',
    workspaceMode: 'highest',
    pushTargets,
    preferredWorkspaceId: null,
    credentialStatus: 'verified',
    poolStatus: 'pending_push',
    poolAccountIds: pushTargets.map(target => target.accountId),
    poolAccounts: pushTargets.map(target => ({
      id: target.accountId,
      workspaceId: target.workspaceId,
      planType: target.planType,
      enabled: true,
      status: 'normal',
    })),
    reloginAccountId: null,
    reloginCount: 0,
    lastReloginAt: null,
    verifiedAt: new Date().toISOString(),
    expiresAt: '2099-01-01T00:00:00Z',
    updatedAt: new Date().toISOString(),
  }))
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/dev/api/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace('/dev', '')
    let data = null
    if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/relogin') {
      data = { items: rows, settings: { concurrency: 1, paused: false, maxRetries: 2, retryIntervalMinutes: 5 } }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'test', buildType: 'test' }
    }
    else if (request.method() === 'POST') {
      assert.ok(['/api/admin/relogin/queue', '/api/admin/relogin/push'].includes(path))
      const body = request.postDataJSON()
      mutations.push({ path, body })
      data = body.ids.map(id => ({ id, success: true, message: '完成' }))
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  const row = id => page.locator(`tr[data-row-key="${id}"]`)
  const pushDialog = page.getByRole('alertdialog', { name: '确认推送到号池', exact: true })
  const queueDialog = page.getByRole('alertdialog', { name: '确认重登', exact: true })
  const confirm = dialog => dialog.getByRole('button', { name: '确认', exact: true }).click()
  try {
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5243'}/relogin`)
    await row('single').getByText('single@example.invalid', { exact: true }).waitFor()
    await row('single').getByRole('button', { name: '重登', exact: true }).click()
    assert.equal(mutations.length, 0)
    await queueDialog.getByText('最高套餐', { exact: true }).waitFor()
    await confirm(queueDialog)
    await queueDialog.waitFor({ state: 'hidden' })
    assert.deepEqual(mutations.at(-1).body, { ids: ['single'], workspaceMode: 'highest' })
    await row('single').getByRole('button', { name: '重登', exact: true }).click()
    await queueDialog.getByRole('combobox').click()
    await page.getByRole('option', { name: '原工作区', exact: true }).click()
    await confirm(queueDialog)
    await queueDialog.waitFor({ state: 'hidden' })
    assert.equal(mutations.at(-1).body.workspaceMode, 'original')
    await row('single').getByRole('button', { name: '推送', exact: true }).click()
    await pushDialog.getByText('将原 FREE 账号切换为 TEAM，不另建账号。', { exact: true }).waitFor()
    assert.equal(await pushDialog.getByRole('textbox', { name: '本批账号名称' }).count(), 0)
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      const box = await pushDialog.boundingBox()
      assert.ok(box.x >= 0 && box.x + box.width <= width + 1)
      assert.ok(await pushDialog.evaluate(node => node.scrollWidth <= node.clientWidth + 1))
      await page.screenshot({ path: `${output}/switch-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await confirm(pushDialog)
    await pushDialog.waitFor({ state: 'hidden' })
    assert.deepEqual(mutations.at(-1).body, {
      ids: ['single'],
      revisions: { single: 7 },
      selections: { single: { accountId: 'free-row', switchWorkspace: true } },
    })
    await row('multiple').getByRole('button', { name: '推送', exact: true }).click()
    assert.ok(await pushDialog.getByRole('button', { name: '确认', exact: true }).isDisabled())
    await pushDialog.getByRole('combobox').click()
    await page.getByRole('option', { name: 'FREE · workspace-free-2', exact: true }).click()
    await confirm(pushDialog)
    await pushDialog.waitFor({ state: 'hidden' })
    assert.equal(mutations.at(-1).body.selections.multiple.accountId, 'free-2')
    await row('destination').getByRole('button', { name: '推送', exact: true }).click()
    await pushDialog.getByText('更新所选账号，工作区不变。', { exact: true }).waitFor()
    await confirm(pushDialog)
    await pushDialog.waitFor({ state: 'hidden' })
    assert.deepEqual(mutations.at(-1).body.selections.destination, { accountId: 'business-row', switchWorkspace: false })
    assert.deepEqual(errors, [])
    console.log(JSON.stringify({ result: 'passed', output, assertions: 'highest/original, explicit target, same-row switch, existing destination, multiple choices, desktop/mobile' }))
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
