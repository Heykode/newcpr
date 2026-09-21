import { realpathSync } from 'node:fs'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { accounts as samples } from './fixtures/relogin-count-data.mjs'

// Isolated synthetic queue; never forward credentials or requests to a real backend.
async function main() {
  const accounts = structuredClone(samples)
  const library = ['new-one', 'new-two'].map(id => ({
    id,
    revision: 1,
    email: `${id}@example.invalid`,
    hasTotp: true,
    automatic: true,
    status: 'ready',
    message: '',
    planType: 'team',
    workspaceId: `workspace-${id}`,
    preferredWorkspaceId: null,
    credentialStatus: 'verified',
    poolStatus: 'absent',
    poolAccountIds: [],
    poolAccounts: [],
    reloginAccountId: null,
    reloginCount: 0,
    lastReloginAt: null,
    verifiedAt: new Date().toISOString(),
    expiresAt: '2099-01-01T00:00:00Z',
    importedAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  }))
  library.push({
    ...library[0],
    id: 'existing',
    email: accounts[0].email,
    poolAccountIds: [accounts[0].id],
    poolStatus: 'present',
    workspaceId: accounts[0].accountId,
  })
  const templates = []
  const reloginSettings = { concurrency: 1, paused: false, maxRetries: 2, retryIntervalMinutes: 5 }
  const groups = [{
    id: 'grp_00000000000000000000000000000091',
    name: '测试分组',
    color: '#34A853FF',
    enabled: true,
    memberCount: 0,
  }]
  let nextTemplateId = 0
  const actions = accounts.slice(0, 2).map(account => ({
    accountId: account.id,
    entryId: `entry-${account.id}`,
    revision: 1,
    target: { account_id: account.id, credential_revision: 1, user_id: account.userId, workspace_id: account.accountId },
    status: 'pending',
    message: '',
    busy: false,
    blockedReason: null,
    syncedAt: null,
  }))
  const deadlines = new Map()
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: {
      host: '127.0.0.1',
      port: Number(process.env.QA_PORT || 5216),
      strictPort: true,
      fs: {
        allow: [
          fileURLToPath(new URL('..', import.meta.url)),
          ...['inter', 'jetbrains-mono'].map(name => realpathSync(new URL(`../node_modules/@fontsource-variable/${name}`, import.meta.url))),
        ],
      },
    },
    plugins: [{
      name: 'local-account-relogin-fixture',
      configResolved(config) {
        config.server.proxy = {}
      },
      configureServer(vite) {
        vite.middlewares.use(async (request, response, next) => {
          const url = new URL(request.url ?? '/', 'http://127.0.0.1')
          if (!url.pathname.startsWith('/dev/'))
            return next()
          response.setHeader('Content-Type', 'application/json')
          response.setHeader('Cache-Control', 'no-store')
          let data
          let status = 200
          const path = url.pathname.replace('/dev', '')
          if (request.method === 'GET') {
            switch (path) {
              case '/api/admin/auth/status':
                data = { authenticated: true }
                break
              case '/api/admin/system/version':
                data = { version: 'local-sample', buildType: 'test' }
                break
              case '/api/admin/accounts':
                data = {
                  items: accounts,
                  page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
                  summary: { total: 3, normal: 3, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
                }
                break
              case '/api/admin/account-groups':
                data = { items: groups, page: { page: 1, pageSize: 200, total: groups.length, totalPages: 1 } }
                break
              case '/api/admin/proxies':
                data = { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } }
                break
              case '/api/admin/ipv6-egress':
                data = { revision: 1, defaultMode: 'unchanged', addresses: [], accountOverrides: {}, fixedBindings: {} }
                break
              case '/api/admin/relogin':
                data = { settings: reloginSettings, items: library }
                break
              case '/api/admin/relogin/templates':
                data = templates
                break
              case '/api/admin/accounts/import-tasks':
                data = { items: [] }
                break
            }
          }
          else if (request.method === 'POST' && path === '/api/admin/relogin/settings') {
            let raw = ''
            for await (const chunk of request)
              raw += chunk
            const body = JSON.parse(raw)
            Object.assign(reloginSettings, body)
            data = null
          }
          else if (request.method === 'POST' && ['/api/admin/accounts/update', '/api/admin/accounts/batch-update'].includes(path)) {
            let raw = ''
            for await (const chunk of request)
              raw += chunk
            const body = JSON.parse(raw)
            const ids = body.accountIds ?? [body.accountId]
            if (ids.some(id => !accounts.some(account => account.id === id))) {
              status = 404
            }
            else {
              for (const account of accounts.filter(account => ids.includes(account.id))) {
                for (const field of ['customName', 'enabled', 'turnStateInjectionEnabled', 'concurrencyLimit', 'weight']) {
                  if (Object.hasOwn(body, field))
                    account[field] = field === 'customName' ? body[field]?.trim() || null : body[field]
                }
                if (body.groupIds)
                  account.groups = groups.filter(group => body.groupIds.includes(group.id))
              }
              data = { accountIds: ids, configRevision: 2 }
            }
          }
          else if (request.method === 'POST' && path === '/api/admin/accounts/apply-template') {
            let raw = ''
            for await (const chunk of request)
              raw += chunk
            const body = JSON.parse(raw)
            const template = templates.find(row => row.id === body.template?.id && row.revision === body.template?.revision)
            if (!template || !body.accountIds?.length || body.accountIds.some(id => !accounts.some(account => account.id === id))) {
              status = 409
            }
            else {
              for (const account of accounts.filter(account => body.accountIds.includes(account.id))) {
                const config = template.config
                Object.assign(account, {
                  enabled: config.enabled,
                  concurrencyLimit: config.concurrencyLimit,
                  weight: config.weight,
                  groups: groups.filter(group => config.groupIds.includes(group.id)),
                  outboundProxyEndpoint: null,
                })
                if (config.turnStateInjectionEnabled != null)
                  account.turnStateInjectionEnabled = config.turnStateInjectionEnabled
              }
              data = { accountIds: body.accountIds, configRevision: 2 }
            }
          }
          else if (request.method === 'POST' && path.startsWith('/api/admin/relogin/') && !path.startsWith('/api/admin/relogin/accounts/')) {
            let raw = ''
            for await (const chunk of request)
              raw += chunk
            const body = JSON.parse(raw)
            if (path.endsWith('/templates/save')) {
              const index = templates.findIndex(row => row.id === body.selection?.id)
              if (body.selection && (index < 0 || templates[index].revision !== body.selection.revision)) {
                status = 409
              }
              else {
                data = {
                  id: body.selection?.id ?? `template-${++nextTemplateId}`,
                  revision: (body.selection?.revision ?? 0) + 1,
                  config: body.config,
                }
                if (index < 0)
                  templates.push(data)
                else templates[index] = data
              }
            }
            else if (path.endsWith('/templates/delete')) {
              const index = templates.findIndex(row => row.id === body.id && row.revision === body.revision)
              if (index < 0) {
                status = 409
              }
              else {
                templates.splice(index, 1)
                data = null
              }
            }
            else if (path.endsWith('/queue')) {
              data = body.ids.map((id) => {
                const row = library.find(row => row.id === id)
                if (!row)
                  return { id, success: false, message: '测试资料不存在' }
                row.workspaceMode = body.workspaceMode ?? 'original'
                row.pushTargets = row.workspaceMode === 'highest'
                  ? row.poolAccountIds.map(accountId => ({
                      accountId,
                      workspaceId: row.workspaceId,
                      planType: row.planType,
                      switchWorkspace: true,
                      available: true,
                    }))
                  : []
                if (row.workspaceMode === 'highest') {
                  row.workspaceId = `workspace-business-${id}`
                  row.planType = 'team'
                }
                row.status = 'ready'
                row.poolStatus = row.poolAccountIds.length ? 'pending_push' : 'absent'
                row.credentialStatus = 'verified'
                row.message = '本地合成凭据，待确认推送'
                row.revision++
                return { id, success: true, message: '完成' }
              })
            }
            else if (path.endsWith('/push')) {
              if (body.template && !templates.some(row => row.id === body.template.id && row.revision === body.template.revision))
                status = 409
              else data = body.ids.map(id => ({ id, success: true, message: '凭据已同步到号池' }))
            }
          }
          else if (request.method === 'POST' && path.startsWith('/api/admin/relogin/accounts/')) {
            let raw = ''
            for await (const chunk of request)
              raw += chunk
            const body = JSON.parse(raw)
            if (path.endsWith('/query')) {
              for (const action of actions) {
                if (!action.busy || (deadlines.get(action.accountId) ?? Infinity) > Date.now())
                  continue
                action.busy = false
                action.blockedReason = null
                action.status = 'ready'
                action.message = '凭据已同步到号池'
                action.syncedAt = new Date().toISOString()
                action.revision++
                action.target.credential_revision++
                const account = accounts.find(account => account.id === action.accountId)
                account.reloginCount++
                account.lastReloginAt = action.syncedAt
              }
              data = actions.filter(action => body.ids.includes(action.accountId))
            }
            else if (path.endsWith('/queue')) {
              const action = actions.find(action => action.entryId === body.entryId)
              if (!action || action.busy || action.revision !== body.revision || JSON.stringify(action.target) !== JSON.stringify(body.target)) {
                status = 409
              }
              else {
                action.revision++
                action.status = 'running'
                action.busy = true
                action.syncedAt = null
                action.message = '正在登录并验证工作区'
                action.blockedReason = '该邮箱已有重登任务'
                deadlines.set(action.accountId, Date.now() + 3000)
                data = null
              }
            }
          }
          response.statusCode = data === undefined && status === 200 ? 405 : status
          response.end(JSON.stringify({ code: response.statusCode, message: response.statusCode === 200 ? 'ok' : 'Local fixture: unsupported or stale operation', data: data ?? null }))
        })
      },
    }],
  })
  await server.listen()
  process.stdout.write(`Local sample only: http://127.0.0.1:${server.config.server.port}/accounts\n`)
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
