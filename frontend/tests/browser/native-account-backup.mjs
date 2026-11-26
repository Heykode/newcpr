/* eslint-disable no-console -- Standalone isolated browser validation. */
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const browser = await chromium.launch({ headless: true })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5177'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-native-account-backup'
  await mkdir(output, { recursive: true })
  const now = new Date().toISOString()
  const agy = { client: 'desktop', platform: 'linux', versionMode: 'latest', clientVersion: null, arch: 'x64', systemPrompt: null }
  const openai = { client: 'cli', platform: 'linux', versionMode: 'latest', originator: 'fixture', osType: 'Linux', osVersion: 'fixture', arch: 'x64', terminal: 'fixture', codexVersion: null, desktopVersion: null, desktopBuild: null }
  try {
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce', locale: 'zh-CN' })
      const errors = []
      const writes = []
      const quotaAccounts = [
        {
          ...accounts[0],
          provider: 'antigravity',
          planType: null,
          planTypeDisplay: '—',
          quota: {
            ...accounts[0].quota,
            windows: ['Gemini Models', 'Future Models'].map((limitName, index) => ({
              key: `group-${index}`,
              group: 'weekly',
              limitId: `model-group-${index}`,
              limitName,
              role: null,
              windowSeconds: 604800,
              labelDisplay: `${limitName} weekly`,
              windowLabelDisplay: 'weekly',
              usedPercent: index ? null : 35,
              usedPercentDisplay: index ? '—' : '35%',
              limitReached: false,
              resetAt: new Date(Date.now() + 7200000).toISOString(),
              resetAtDisplay: 'Synthetic reset time',
            })),
          },
          usage: { ...accounts[0].usage, windowKind: 'recent_week', windowLabelDisplay: '近七日' },
        },
        { ...accounts[1], provider: 'antigravity', planType: null, planTypeDisplay: '—', usage: { ...accounts[1].usage, windowKind: 'recent_week', windowLabelDisplay: '近七日' } },
      ]
      let backup = { storageRevision: 1, endpoint: 'https://storage.example.com', region: 'auto', bucket: 'fixture', accessKeyId: 'fixture-id', secretConfigured: true, prefix: 'backups/', forcePathStyle: false, verified: true, scheduleEnabled: false, cronExpression: '0 2 * * *', scheduleTimezone: 'UTC', retentionDays: 7, retentionCount: 7, updatedAt: now }
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/dev/api/**', async (route) => {
        const req = route.request()
        const url = new URL(req.url())
        const path = url.pathname.replace(/^\/dev/u, '')
        let data
        if (req.method() === 'POST')
          writes.push({ path, body: req.postDataJSON() })
        if (path === '/api/admin/accounts') {
          data = { items: quotaAccounts, page: { page: 1, pageSize: 20, total: 2, totalPages: 1 }, summary: { total: 2, normal: 2, disabled: 0, error: 0, rateLimited: 0, quotaExhausted: 0 } }
        }
        else if (path === '/api/admin/system/update/detail') {
          data = { enabled: false, status: 'idle', currentVersion: 'isolated-test' }
        }
        else if (path === '/api/admin/accounts/oauth/start') {
          assert.equal(req.postDataJSON().provider, 'antigravity')
          data = { flowId: 'fixture-flow', authorizationUrl: 'https://accounts.example.com/authorize?state=fixture-state', expiresAt: new Date(Date.now() + 300000).toISOString() }
        }
        else if (path === '/api/admin/client-keys') {
          data = { items: [], total: 0, nextCursor: null, configRevision: 1 }
        }
        else if (path === '/api/admin/account-groups') {
          data = { items: [{ id: 'grp_00000000000000000000000000000001', name: 'Synthetic long group name for native platforms', enabled: true, providerCounts: { antigravity: 1 } }], page: { page: 1, total: 1, totalPages: 1 } }
        }
        else if (path === '/api/admin/settings/client-profiles') {
          const isAgy = url.searchParams.get('provider') === 'antigravity'
          const configuration = isAgy ? agy : openai
          data = { defaults: configuration, globalConfiguration: configuration, maxVersionLag: 5, recognizedUserAgentPrefixes: ['Codex Desktop/', 'codex-tui/', 'codex_exec/', 'codex_cli_rs/'], presets: [{ configuration, defaults: configuration, automaticAvailable: true, reason: null }], defaultSystemPrompt: 'Synthetic server instruction', systemPromptSource: 'fixture' }
        }
        else if (path === '/api/admin/settings/client-profiles/preview') {
          const isAgy = req.postDataJSON().provider === 'antigravity'
          data = { source: 'global', configuration: isAgy ? agy : openai, userAgent: 'fixture/0.0.0', versionSource: 'official', checkedAt: null, verifiedAt: null, error: null, product: 'fixture', osType: 'Linux', osVersion: 'fixture', arch: 'x64', terminal: 'fixture', originator: 'fixture', codexVersion: '0.0.0', clientVersion: '0.0.0', desktopVersion: null, desktopBuild: null }
        }
        else if (path === '/api/admin/settings/backups') {
          data = backup
        }
        else if (path === '/api/admin/settings/backups/records') {
          data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 } }
        }
        else if (path === '/api/admin/settings/backups/storage/update') {
          const body = req.postDataJSON()
          assert.equal(body.storageRevision, backup.storageRevision)
          backup = { ...backup, ...body, storageRevision: backup.storageRevision + 1, secretConfigured: true, verified: false, scheduleEnabled: false }
          delete backup.secretAccessKey
          data = backup
        }
        else if (path === '/api/admin/settings/backups/storage/test') {
          assert.equal(req.postDataJSON().storageRevision, backup.storageRevision)
          backup.verified = true
          data = { storageRevision: backup.storageRevision, ok: true, stage: 'read', code: null, message: 'ok' }
        }
        else if (path === '/api/admin/settings/backups/schedule/update') {
          assert.equal(req.postDataJSON().storageRevision, backup.storageRevision)
          backup = { ...backup, ...req.postDataJSON() }
          data = backup
        }
        else {
          return route.continue()
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      try {
        await page.goto(`${base}/accounts`)
        await page.getByRole('button', { name: '导入账号', exact: true }).waitFor({ timeout: 90000 })
        const providerFilter = page.getByRole('radio', { name: 'Antigravity', exact: true }).locator('..')
        assert.equal(await providerFilter.getByRole('radio').count(), 4)
        assert.equal(await providerFilter.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true, 'All four platform options must fit without horizontal scrolling')
        await page.getByRole('button', { name: '展开统计', exact: true }).first().click()
        const quotaPanel = page.locator('section').filter({ has: page.getByRole('heading', { name: '账号额度', exact: true }) }).last()
        await quotaPanel.getByText('Gemini 模型 · 周额度', { exact: true }).waitFor()
        await quotaPanel.getByText('Future Models · 周额度', { exact: true }).waitFor()
        assert.equal(await quotaPanel.getByRole('progressbar').first().getAttribute('aria-valuenow'), '35')
        assert.equal(await quotaPanel.getByRole('progressbar').nth(1).getAttribute('aria-valuenow'), null)
        assert.equal(await quotaPanel.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
        await quotaPanel.screenshot({ path: `${output}/agy-quota-${width}.png` })
        await providerFilter.screenshot({ path: `${output}/account-platforms-${width}.png` })
        await page.getByRole('button', { name: '收起统计', exact: true }).first().click()
        await page.getByRole('button', { name: '导入账号', exact: true }).click()
        let dialog = page.getByRole('dialog')
        await dialog.getByRole('button', { name: 'Antigravity', exact: true }).click()
        await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
        await dialog.getByText('Antigravity OAuth 授权', { exact: true }).waitFor()
        await dialog.getByRole('button', { name: '生成授权链接', exact: true }).click()
        await dialog.getByRole('button', { name: '重新生成授权链接', exact: true }).waitFor()
        await dialog.getByRole('textbox', { name: '完整回调地址', exact: true }).fill('http://localhost:8085/auth/callback?code=fixture-code&state=fixture-state')
        assert.equal(await dialog.getByRole('button', { name: '完成导入', exact: true }).isEnabled(), true)
        await page.screenshot({ path: `${output}/agy-oauth-${width}.png` })
        assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
        await dialog.getByRole('radio', { name: '账号文件', exact: true }).click()
        await dialog.getByRole('textbox', { name: '账号文件', exact: true }).fill('{"provider":"antigravity","accounts":[]}')
        assert.equal(await dialog.getByRole('button', { name: '创建导入任务', exact: true }).isEnabled(), true)
        assert.equal(await dialog.getByRole('radio', { name: 'AT', exact: true }).count(), 0)
        await page.keyboard.press('Escape')
        await dialog.waitFor({ state: 'hidden' })

        await page.goto(`${base}/api-keys`)
        await page.getByRole('button', { name: /^创建 API/u }).click()
        dialog = page.getByRole('dialog', { name: '创建 API Key', exact: true })
        await dialog.getByRole('radio', { name: '独立配置', exact: true }).click()
        await dialog.getByRole('combobox', { name: '版本策略', exact: true }).click()
        await page.getByRole('option', { name: '自定义版本', exact: true }).click()
        const userAgent = dialog.getByRole('textbox', { name: '用户代理', exact: true })
        await userAgent.fill('codex_cli_rs/1.2.3 (Linux; x86_64)')
        assert.equal(await dialog.getByRole('textbox', { name: 'originator', exact: true }).count(), 0)
        await userAgent.fill('fixture-client/1.2.3')
        await dialog.getByRole('textbox', { name: 'originator', exact: true }).fill('fixture-client')
        await dialog.getByRole('textbox', { name: 'Core 版本', exact: true }).fill('1.2.3')
        await page.screenshot({ path: `${output}/codex-custom-identity-${width}.png` })
        const sharing = dialog.getByRole('switch', { name: 'CPR 互联', exact: true })
        assert.equal(await sharing.isChecked(), false)
        await sharing.locator('..').click()
        const plans = dialog.getByRole('checkbox', { name: '共享套餐构成', exact: true })
        assert.equal(await plans.isDisabled(), true)
        await dialog.getByRole('checkbox', { name: '共享账号池概览', exact: true }).locator('..').click()
        assert.equal(await plans.isEnabled(), true)
        await plans.locator('..').click()
        await dialog.getByRole('checkbox', { name: '共享账号池概览', exact: true }).locator('..').click()
        assert.equal(await plans.isChecked(), false)
        await dialog.getByRole('radio', { name: 'Antigravity', exact: true }).click()
        await dialog.getByRole('radio', { name: '独立配置', exact: true }).click()
        await dialog.getByRole('textbox', { name: '系统提示词', exact: true }).fill('Synthetic key instruction')
        await page.screenshot({ path: `${output}/agy-key-${width}.png` })
        assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
        await dialog.getByRole('button', { name: '取消', exact: true }).click()
        await dialog.waitFor({ state: 'hidden' })

        await page.goto(`${base}/settings/backup`)
        const storage = page.locator('section').filter({ has: page.getByRole('heading', { name: 'S3 存储配置', exact: true }) })
        const secret = storage.getByLabel('Secret Access Key', { exact: true })
        await secret.waitFor()
        await page.getByText('已配置，留空保留原密钥；输入新值后更换', { exact: true }).waitFor()
        assert.equal(await secret.inputValue(), '')
        await storage.getByRole('textbox', { name: 'Key 前缀', exact: true }).fill('changed/')
        assert.equal(await storage.getByRole('button', { name: '测试连接', exact: true }).isDisabled(), true)
        const schedule = page.locator('section').filter({ has: page.getByRole('heading', { name: '定时备份', exact: true }) })
        const enabled = schedule.getByRole('checkbox')
        assert.equal(await enabled.isDisabled(), true)
        await Promise.all([page.waitForResponse(res => res.url().endsWith('/storage/update')), storage.getByRole('button', { name: '保存', exact: true }).click()])
        const saved = writes.findLast(row => row.path.endsWith('/storage/update')).body
        assert.equal(Object.hasOwn(saved, 'secretAccessKey'), false)
        assert.equal(saved.storageRevision, 1)
        await Promise.all([page.waitForResponse(res => res.url().endsWith('/storage/test')), storage.getByRole('button', { name: '测试连接', exact: true }).click()])
        await enabled.locator('..').click()
        await Promise.all([page.waitForResponse(res => res.url().endsWith('/schedule/update')), schedule.getByRole('button', { name: '保存', exact: true }).click()])
        assert.equal(writes.findLast(row => row.path.endsWith('/schedule/update')).body.storageRevision, 2)
        await secret.fill('synthetic-replacement-value')
        await Promise.all([page.waitForResponse(res => res.url().endsWith('/storage/update')), storage.getByRole('button', { name: '保存', exact: true }).click()])
        assert.equal(writes.findLast(row => row.path.endsWith('/storage/update')).body.secretAccessKey, 'synthetic-replacement-value')
        await page.waitForFunction(input => input.value === '', await secret.elementHandle())
        assert.equal(await enabled.isDisabled(), true)
        assert.equal(await enabled.isChecked(), false)
        await page.screenshot({ path: `${output}/backup-${width}.png` })
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), true)
        assert.deepEqual(errors, [])
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failure-${width}.png`, fullPage: true })
        console.error(JSON.stringify({ width, errors, pageText: await page.locator('body').textContent() }))
        throw error
      }
      finally { await page.close() }
    }
    console.log('AGY account controls, Key sharing and backup revisions passed at 1440/390/320px')
  }
  finally { await browser.close() }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
