import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'

// Browser regressions intercept all APIs; this server cannot reach a real backend.
async function main() {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: { host: '127.0.0.1', port: 5198, strictPort: true },
    plugins: [{
      name: 'isolated-import-tasks-preview',
      configResolved(config) {
        config.server.proxy = {}
      },
      configureServer(vite) {
        vite.middlewares.use((request, response, next) => {
          if (!/^\/(?:dev\/)?api\//.test(request.url ?? ''))
            return next()
          response.statusCode = 503
          response.setHeader('Content-Type', 'application/json')
          response.end(JSON.stringify({ message: 'Isolated preview: API interception required' }))
        })
      },
    }],
  })
  await server.listen()
  process.stdout.write('Isolated browser regression server: http://127.0.0.1:5198/accounts\n')
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
