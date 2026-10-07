const OS_VERSION = /^(.*?\((?:Mac OS|Windows|Linux|Ubuntu) )(\d[\d.]*)(; [^)]+\).*)$/

export function userAgentOsVersion(userAgent: string): string | null {
  return OS_VERSION.exec(userAgent)?.[2] ?? null
}

export function withUserAgentOsVersion(userAgent: string, version: string): string | null {
  const match = OS_VERSION.exec(userAgent)
  if (!match || !/^\d+(?:\.\d+)*$/.test(version.trim()))
    return null
  return `${match[1]}${version.trim()}${match[3]}`
}
