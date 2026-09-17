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
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-relogin-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  const now = new Date().toISOString()
  function row(id, overrides = {}) {
    return {
      id,
      revision: 1,
      email: `${id}@example.invalid`,
      automatic: true,
      status: 'pending',
      message: '',
      planType: null,
      workspaceId: null,
      preferredWorkspaceId: null,
      credentialStatus: 'none',
      poolStatus: 'absent',
      poolAccountIds: [],
      poolAccounts: [],
      reloginAccountId: null,
      reloginCount: 0,
      lastReloginAt: null,
      verifiedAt: null,
      expiresAt: null,
      updatedAt: now,
      importedAt: now,
      ...overrides,
    }
  }
  let rows = [
    row('team-account', {
      status: 'ready',
      planType: 'self_serve_business_prolite',
      workspaceId: '12345678-1234-5678-9012-123456789012',
      credentialStatus: 'verified',
      verifiedAt: now,
      expiresAt: '2099-01-01T00:00:00Z',
      poolStatus: 'present',
      poolAccountIds: ['acct_original'],
      reloginAccountId: 'acct_original',
      poolAccounts: [{
        id: 'acct_original',
        workspaceId: '12345678-1234-5678-9012-123456789012',
        planType: 'self_serve_business_prolite',
        enabled: true,
        status: 'error',
        errorReason: 'credential_expired',
        errorMessage: 'HTTP 401: credential expired',
      }],
    }),
    row('free-account', { planType: 'free' }),
    row('waiting-account'),
  ]
  let settings = { concurrency: 1, paused: false }
  const mutations = []
  await page.route('**/dev/api/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace('/dev', '')
    let data = null
    if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/relogin') {
      data = { items: [...rows].sort((a, b) => b.importedAt.localeCompare(a.importedAt)), settings }
    }
    else if (path.startsWith('/api/admin/relogin/')) {
      const body = request.postDataJSON()
      mutations.push({ path, body })
      if (path.endsWith('/import')) {
        for (const line of body.text.split('\n').filter(Boolean)) {
          const email = line.split('----')[0].toLowerCase()
          rows.push(row(email.split('@')[0], { email, importedAt: new Date().toISOString() }))
        }
        data = { imported: 2 }
      }
      else if (path.endsWith('/queue')) {
        data = body.ids.map(id => ({ id, success: true, message: '完成' }))
        rows = rows.map(row => body.ids.includes(row.id) ? { ...row, status: 'running', revision: row.revision + 1 } : row)
      }
      else if (path.endsWith('/push')) {
        data = body.ids.map(id => ({ id, success: false, message: '资料已变化，请重新确认' }))
      }
      else if (path.endsWith('/delete')) {
        rows = rows.filter(row => !body.ids.includes(row.id))
      }
      else if (path.endsWith('/automatic')) {
        rows = rows.map(row => body.ids.includes(row.id) ? { ...row, automatic: body.enabled, revision: row.revision + 1 } : row)
      }
      else if (path.endsWith('/workspace')) {
        rows = rows.map(row => row.id === body.id ? { ...row, preferredWorkspaceId: body.workspaceId, status: 'pending' } : row)
      }
      else if (path.endsWith('/settings')) {
        settings = body
      }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'test', buildType: 'test' }
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })

  try {
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5179'}/relogin`)
    await page.getByRole('heading', { name: '失效重登', exact: true }).waitFor()
    await page.getByText('team-account@example.invalid', { exact: true }).waitFor()
    assert.equal(await page.locator('tbody tr').count(), 3)
    const team = page.locator('tbody tr[data-row-key="team-account"]')
    await team.getByText('凭据失效', { exact: true }).waitFor()
    assert.match(await team.locator('[data-column-key="pool"] [title]').getAttribute('title'), /401/)
    assert.equal(await team.getByRole('button', { name: '重登', exact: true }).count(), 1)
    assert.equal(await team.getByRole('button', { name: '推送', exact: true }).count(), 1)
    for (const width of [1920, 1440, 1280]) {
      await page.setViewportSize({ width, height: 1000 })
      const overflow = await page.locator('tbody td[data-column-key="plan"]').evaluateAll(cells => cells.map((cell) => {
        const box = cell.getBoundingClientRect()
        return [...cell.querySelectorAll('div')].some(node => node.getBoundingClientRect().right > box.right + 1)
      }))
      assert.ok(overflow.every(value => !value), `workspace overflow at ${width}`)
      const buttons = await team.locator('[data-column-key="actions"] button').evaluateAll(nodes => nodes.map((node) => {
        const rect = node.getBoundingClientRect()
        return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width, scroll: node.scrollWidth, client: node.clientWidth, labelHeight: node.textContent.trim() ? node.lastElementChild.getBoundingClientRect().height : 0 }
      }))
      assert.ok(buttons.every((button, index) => !index || button.top >= buttons[index - 1].bottom || button.left >= buttons[index - 1].right), `overlapping buttons at ${width}`)
      assert.ok(buttons.every(button => button.scroll <= button.client + 1))
      assert.ok(buttons.every(button => button.labelHeight <= 18), `wrapped action labels at ${width}`)
      await page.screenshot({ path: `${output}/desktop-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.screenshot({ path: `${output}/desktop.png`, fullPage: true })
    await page.getByRole('button', { name: '切换暗黑模式', exact: true }).click()
    await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
    await page.screenshot({ path: `${output}/desktop-dark.png`, fullPage: true })
    await page.getByRole('button', { name: '切换浅色模式', exact: true }).click()
    await team.getByRole('button', { name: '选择工作区', exact: true }).click()
    const workspaceDialog = page.getByRole('dialog')
    await workspaceDialog.waitFor()
    assert.equal(await workspaceDialog.getByRole('textbox').count(), 0)
    await workspaceDialog.getByRole('combobox', { name: '登录工作区' }).click()
    await page.getByRole('option').filter({ hasText: '12345678-1234-5678-9012-123456789012' }).waitFor()
    await page.keyboard.press('Escape')
    await page.keyboard.press('Escape')
    await workspaceDialog.waitFor({ state: 'hidden' })
    await page.getByRole('textbox', { name: '搜索邮箱', exact: true }).fill('free-account')
    await page.locator('tbody tr[data-row-key="team-account"]').waitFor({ state: 'hidden' })
    assert.equal(await page.locator('tbody tr').count(), 1)
    await page.getByRole('textbox', { name: '搜索邮箱', exact: true }).fill('')
    await page.locator('tbody tr[data-row-key="team-account"]').waitFor()
    await page.getByRole('switch', { name: 'free-account@example.invalid 自动重登', exact: true }).locator('..').click()
    await page.waitForFunction(() => document.querySelector('[aria-label="free-account@example.invalid 自动重登"]')?.checked === false)
    assert.equal(rows.find(row => row.id === 'free-account').automatic, false)

    await page.getByRole('button', { name: '导入', exact: true }).click()
    await page.getByRole('textbox', { name: '账号资料', exact: true }).fill(
      'legacy@example.invalid----p----123e4567-e89b-12d3-a456-426614174000----!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!',
    )
    await page.getByText('格式错误', { exact: true }).waitFor()
    assert.equal(await page.getByRole('button', { name: '确认导入', exact: true }).isDisabled(), true)
    assert.equal(await page.getByRole('dialog').getByText('邮箱验证码', { exact: false }).count(), 0)
    await page.getByRole('textbox', { name: '账号资料', exact: true }).fill(
      'new-one@example.invalid----p%----JBSWY3DPEHPK3PXP\nnew-two@example.invalid----p----JBSWY3DPEHPK3PXP',
    )
    await page.getByRole('button', { name: '确认导入', exact: true }).click()
    await page.getByText('new-one@example.invalid', { exact: true }).waitFor()
    assert.match(await page.locator('tbody tr').first().getAttribute('data-row-key'), /^new-/)
    assert.equal(mutations.filter(item => item.path.endsWith('/queue')).length, 0)
    assert.equal(mutations.filter(item => item.path.endsWith('/push')).length, 0)

    await page.getByRole('checkbox', { name: '全选当前页', exact: true }).locator('..').click()
    await page.getByText('已选 5 项', { exact: true }).waitFor()
    await page.getByRole('button', { name: '取消选择', exact: true }).click()
    await page.locator('tbody tr').first().locator('[data-swipe-select-handle]').last().scrollIntoViewIfNeeded()
    const first = page.locator('tbody tr').nth(0).locator('[data-swipe-select-handle]').last()
    const third = page.locator('tbody tr').nth(2).locator('[data-swipe-select-handle]').last()
    const a = await first.boundingBox()
    const b = await third.boundingBox()
    await page.mouse.move(a.x + 10, a.y + 10)
    await page.mouse.down()
    await page.mouse.move(b.x + 10, b.y + 10, { steps: 10 })
    await page.mouse.up()
    await page.getByText('已选 3 项', { exact: true }).waitFor()
    await page.mouse.move(a.x + 10, a.y + 10)
    await page.mouse.down()
    await page.mouse.move(b.x + 10, b.y + 10, { steps: 10 })
    await page.mouse.up()
    await page.getByText('已选 0 项', { exact: true }).waitFor()

    await team.getByRole('button', { name: '推送', exact: true }).click()
    await page.getByRole('alertdialog').waitFor()
    assert.equal(mutations.filter(item => item.path.endsWith('/push')).length, 0)
    await page.getByRole('alertdialog').getByRole('button', { name: '确认', exact: true }).click()
    await page.getByRole('alert').filter({ hasText: '资料已变化' }).waitFor()
    const push = mutations.find(item => item.path.endsWith('/push'))
    assert.deepEqual(push.body, { ids: ['team-account'], revisions: { 'team-account': 1 } })
    rows = rows.map(row => row.id === 'team-account' ? { ...row, status: 'uncertain' } : row)
    await page.getByRole('button', { name: '刷新重登列表', exact: true }).click()
    await team.getByText('推送待核实', { exact: true }).waitFor()
    assert.ok(await team.getByRole('button', { name: '推送', exact: true }).isDisabled())
    await team.getByText('新凭据已验证', { exact: true }).waitFor()
    rows = rows.map(row => row.id === 'team-account'
      ? { ...row, status: 'ready', poolStatus: 'synced', poolAccounts: row.poolAccounts.map(account => ({ ...account, enabled: false, status: 'normal', errorReason: null, errorMessage: null })) }
      : row)
    await page.getByRole('button', { name: '刷新重登列表', exact: true }).click()
    await team.getByText('已同步到号池', { exact: true }).waitFor()
    await team.getByText('暂停调度', { exact: true }).waitFor()

    await page.getByRole('spinbutton', { name: '重登并发', exact: true }).fill('2')
    await Promise.all([
      page.waitForResponse(response => response.url().endsWith('/relogin/settings')),
      page.getByRole('button', { name: '保存并发设置', exact: true }).click(),
    ])
    assert.equal(settings.concurrency, 2)

    await page.getByRole('checkbox', { name: '选择 new-one@example.invalid', exact: true }).locator('..').click()
    await page.getByRole('checkbox', { name: '选择 new-two@example.invalid', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量重登', exact: true }).click()
    await page.locator('tbody tr').filter({ hasText: 'new-one@example.invalid' }).getByText('重登中', { exact: true }).waitFor()
    assert.deepEqual(mutations.find(item => item.path.endsWith('/queue')).body.ids, ['new-one', 'new-two'])
    await page.getByRole('button', { name: '批量删除资料', exact: true }).click()
    await page.getByRole('alertdialog').getByRole('button', { name: '确认', exact: true }).click()
    await page.locator('tbody tr[data-row-key="new-one"]').waitFor({ state: 'hidden' })
    await page.getByRole('alertdialog').waitFor({ state: 'hidden' })
    await page.getByText('重登资料已删除，号池账号保持不变', { exact: true }).waitFor({ state: 'hidden' })

    await page.setViewportSize({ width: 390, height: 844 })
    await page.getByRole('heading', { name: '失效重登', exact: true }).waitFor()
    await page.screenshot({ path: `${output}/mobile.png`, fullPage: true })
    const overflow = await page.evaluate(() => ({
      width: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
    }))
    assert.ok(overflow.scrollWidth <= overflow.width + 1, JSON.stringify(overflow))
    await page.setViewportSize({ width: 320, height: 740 })
    await page.screenshot({ path: `${output}/mobile-320.png`, fullPage: true })
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1))
    await page.setViewportSize({ width: 390, height: 844 })
    await page.getByRole('button', { name: '导入', exact: true }).click()
    await page.getByRole('dialog').waitFor()
    await page.screenshot({ path: `${output}/mobile-import.png`, fullPage: true })
    const dialog = await page.getByRole('dialog').boundingBox()
    assert.ok(dialog.x >= 0 && dialog.x + dialog.width <= 391)
    assert.deepEqual(errors, [])
    console.log(JSON.stringify({ result: 'passed', assertions: 'import, explicit push, versions, multi-select, drag, concurrency, batch queue/delete, desktop/mobile', output }))
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    console.error((await page.locator('body').textContent()).slice(0, 5000))
    throw error
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
