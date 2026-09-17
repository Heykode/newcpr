interface ReloginAvailability {
  email: string
  hasTotp: boolean
}

interface AccountEmail {
  email: string | null
}

function normalizedEmail(email: string | null) {
  return email?.trim().toLowerCase() ?? ''
}

export function reloginTotpEmailSet(entries: ReloginAvailability[]) {
  return new Set(entries
    .filter(entry => entry.hasTotp)
    .map(entry => normalizedEmail(entry.email))
    .filter(Boolean))
}

export function accountHasReloginTotp(account: AccountEmail, emails: ReadonlySet<string>) {
  const email = normalizedEmail(account.email)
  return email !== '' && emails.has(email)
}
