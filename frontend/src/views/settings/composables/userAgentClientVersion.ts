const CLIENT = /^(Codex Desktop|codex-tui|codex_cli_rs|codex_exec)\/(\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?) (\([^\r\n]+)$/
const VERSION = /^\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?$/

export function userAgentClientVersion(userAgent: string): string | null {
  return CLIENT.exec(userAgent)?.[2] ?? null
}

export function withUserAgentClientVersion(userAgent: string, version: string): string | null {
  const match = CLIENT.exec(userAgent)
  const normalized = version.trim()
  if (!match || !VERSION.test(normalized))
    return null
  const [, client, previous, target] = match
  let suffix = target!
  // Desktop's suffix is its separate application version, not the Core version.
  if (client !== 'Codex Desktop') {
    const old = ` (${client}; ${previous})`
    if (suffix.endsWith(old))
      suffix = `${suffix.slice(0, -old.length)} (${client}; ${normalized})`
  }
  return `${client}/${normalized} ${suffix}`
}
