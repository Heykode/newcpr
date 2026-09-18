import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { accounts, reloginEntries } from './fixtures/relogin-count-data.mjs'
import { layoutEntries } from './fixtures/relogin-layout-data.mjs'

// Read-only synthetic data. No request is forwarded to the normal backend proxy.
async function main() {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: { host: '127.0.0.1', port: Number(process.env.QA_PORT || 5198), strictPort: true },
    plugins: [{
      name: 'local-relogin-count-fixture',
      configResolved(config) {
        config.server.proxy = {}
      },
      configureServer(vite) {
        vite.middlewares.use((request, response, next) => {
          const url = new URL(request.url ?? '/', 'http://127.0.0.1')
          if (!url.pathname.startsWith('/dev/'))
            return next()
          response.setHeader('Content-Type', 'application/json')
          response.setHeader('Cache-Control', 'no-store')
          let data
          if (request.method === 'GET') {
            switch (url.pathname.replace('/dev', '')) {
              case '/api/admin/auth/status':
                data = { authenticated: true }
                break
              case '/api/admin/system/version':
                data = { version: 'local-sample', buildType: 'test' }
                break
              case '/api/admin/accounts': {
                const items = accounts.map(account => ({ ...account }))
                const stateModelCount = Number(process.env.QA_STATE_MODEL_COUNT || 0)
                if (Number.isInteger(stateModelCount) && stateModelCount > 0 && stateModelCount <= 10) {
                  const models = Array.from({ length: stateModelCount }, (_, index) => ({
                    model: `model-${index + 1}`,
                    refreshStatus: index === 1 ? 'refreshing' : 'ready',
                    active: index === 1 ? null : { chars: 332, expiresAt: new Date(Date.now() + 40 * 60_000).toISOString() },
                    standby: index === 1 ? null : { chars: 332, expiresAt: new Date(Date.now() + 45 * 60_000).toISOString() },
                  }))
                  items[0].turnStateInjectionEnabled = true
                  items[0].turnState = {
                    requiredModels: models.map(item => item.model),
                    readyModels: models.filter(item => item.active).map(item => ({ model: item.model, expiresAt: item.active.expiresAt })),
                    models,
                  }
                }
                if (url.searchParams.get('sortBy') === 'reloginCount') {
                  const direction = url.searchParams.get('sortDirection') === 'asc' ? 1 : -1
                  items.sort((a, b) => direction * (a.reloginCount - b.reloginCount))
                }
                data = {
                  items,
                  page: { page: 1, pageSize: 20, total: 3, totalPages: 1 },
                  summary: { total: 3, normal: 3, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
                }
                break
              }
              case '/api/admin/account-groups':
                data = { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } }
                break
              case '/api/admin/relogin':
                data = { items: process.env.QA_RELOGIN_LAYOUT ? layoutEntries : reloginEntries, settings: { concurrency: 1, paused: false } }
                break
            }
          }
          response.statusCode = data === undefined ? 405 : 200
          response.end(JSON.stringify({
            code: response.statusCode,
            message: data === undefined ? 'Local fixture: unsupported operation' : 'ok',
            data: data ?? null,
          }))
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
