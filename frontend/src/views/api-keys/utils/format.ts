export function formatBudgetAmount(value: string): string {
  const match = value.trim().match(/^([+-]?)(\d+)(?:\.(\d+))?$/)
  if (!match)
    return value

  const sign = match[1] === '-' ? '-' : ''
  let integer = BigInt(match[2]!)
  const fraction = (match[3] ?? '').padEnd(3, '0')
  let cents = BigInt(fraction.slice(0, 2) || '0')

  if (Number(fraction[2] ?? 0) >= 5)
    cents += 1n
  if (cents >= 100n) {
    integer += 1n
    cents = 0n
  }

  const decimal = cents === 0n
    ? ''
    : `.${cents.toString().padStart(2, '0').replace(/0+$/, '')}`
  return `${sign}${integer.toLocaleString('en-US')}${decimal}`
}
