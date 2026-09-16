import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { groups, monitorResponse } from './fixtures/monitor-data.mjs'

// This opt-in fixture server never forwards requests to an actual CPR backend.
async function main() {
  let snapshot = monitorResponse(groups.map(group => group.id))
  const sample = () => snapshot = monitorResponse(groups.map(group => group.id))
  const sampling = setInterval(sample, 10_000)
  sampling.unref()
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: { host: '127.0.0.1', port: 5197, strictPort: true },
    plugins: [{
      name: 'local-group-monitor-fixture',
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
          if (request.method !== 'GET' || url.pathname !== '/dev/api/admin/account-groups/monitor') {
            response.statusCode = 405
            response.end(JSON.stringify({ code: 405, message: 'Fixture server: unsupported operation', data: null }))
            return
          }
          if (url.searchParams.get('refreshForecasts') === 'true')
            sample()
          const ids = (url.searchParams.get('groupIds') ?? '').split(',')
          response.end(JSON.stringify({
            code: 200,
            message: 'ok',
            data: { ...snapshot, items: snapshot.items.filter(item => ids.includes(item.id)) },
          }))
        })
      },
    }],
  })
  await server.listen()
  process.stdout.write('Local sample only: http://127.0.0.1:5197/tests/fixtures/group-monitor.html\n')
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
