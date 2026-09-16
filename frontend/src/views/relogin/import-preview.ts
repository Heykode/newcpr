export interface ImportPreviewRow {
  line: number
  email: string
  valid: boolean
}

// This projection never exposes password or TOTP fields.
export function importPreview(text: string): ImportPreviewRow[] {
  const seen = new Set<string>()
  return text.replace(/^\uFEFF/, '').split(/\r?\n/).flatMap((line, index) => {
    if (!line.trim())
      return []
    const first = line.indexOf('----')
    const last = line.lastIndexOf('----')
    const email = first >= 0 ? line.slice(0, first).trim().toLowerCase() : ''
    const password = line.slice(first + 4, last)
    const lastPart = line.slice(last + 4)
    const secret = lastPart.replace(/[\t\n\r \f\v-]/g, '').toUpperCase().replace(/=+$/, '')
    const legacyMailbox = /----[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(password)
    const totp = /^[A-Z2-7]{16,128}$/.test(secret)
      && [0, 2, 4, 5, 7].includes(secret.length % 8)
    const valid = first > 0 && last > first + 4
      && /^[^@\s\p{Cc}]+@[^@\s\p{Cc}]+$/u.test(email)
      && new TextEncoder().encode(email).length <= 254
      && new TextEncoder().encode(password).length <= 1024
      && !/\p{Cc}/u.test(password)
      && totp && !legacyMailbox
      && !seen.has(email)
    seen.add(email)
    return { line: index + 1, email, valid }
  })
}
