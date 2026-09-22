/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const require = createRequire(import.meta.url)

function source(path) {
  return readFileSync(new URL(path, import.meta.url), 'utf8')
}

function loadApi() {
  const calls = []
  const { outputText } = ts.transpileModule(source('../src/api/modules/notifications.ts'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, {
    exports,
    require: name => name === '../request' ? async request => calls.push(request) : require(name),
  })
  return { api: exports, calls }
}

test('notification API keeps channel, policy and test routes separate', async () => {
  const { api, calls } = loadApi()
  await api.getNotificationChannels()
  await api.updateNotificationChannels({ smtp: { enabled: false }, bark: { enabled: false } })
  await api.getGroupAlertPolicy('grp_demo')
  await api.updateGroupAlertPolicy({ groupId: 'grp_demo', updatedAt: 'readonly-time' })
  await api.testNotification({ channel: 'bark', target: 'default', groupId: 'grp_demo' })
  assert.deepEqual(calls.map(call => call.url), [
    '/api/admin/notifications/channels',
    '/api/admin/notifications/channels/update',
    '/api/admin/account-groups/alert-policy',
    '/api/admin/account-groups/alert-policy/update',
    '/api/admin/notifications/test',
  ])
  assert.equal(calls[2].params.groupId, 'grp_demo')
  assert.equal(calls[4].data.groupId, 'grp_demo')
  assert.equal(Object.hasOwn(calls[3].data, 'updatedAt'), false)
})

test('settings notifications are collapsed and independently testable', () => {
  const component = source('../src/views/settings/components/NotificationChannelsCard.vue')
  assert.match(component, /cpr\.settings\.notifications\.open/)
  assert.match(component, /cpr\.settings\.notifications\.smtp\.open/)
  assert.match(component, /cpr\.settings\.notifications\.bark\.open/)
  assert.match(component, /发送测试邮件/)
  assert.match(component, /发送测试通知/)
  assert.doesNotMatch(component, /\{\{\s*form\.smtp\.password\s*\}\}/)
  assert.doesNotMatch(component, /\{\{\s*form\.bark\.deviceKey\s*\}\}/)
})

test('group card replaces monitor info with settings left of pin', () => {
  const card = source('../src/views/accounts/components/AccountGroupMonitorCard.vue')
  const settings = card.search(/设置 \$\{group\.name\} 预警/u)
  const pin = card.indexOf('`${pinned ? \'取消置顶\'')
  assert.ok(settings >= 0)
  assert.ok(pin > settings)
  assert.doesNotMatch(card, /查看监控口径/)
  const modal = source('../src/views/accounts/components/GroupAlertSettingsModal.vue')
  for (const text of ['并发达到阈值', '预计可支撑时间不足', '剩余额度为 0', '可调度账号为 0', '监控口径'])
    assert.match(modal, new RegExp(text))
  assert.match(modal, /预计过期额度仅供参考，不参与预警/)
})
