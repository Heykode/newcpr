import { locale } from '@/i18n'

const formatters = new Map<string, Intl.NumberFormat>()
function formatter(compact = false) {
  const key = `${locale.value}:${compact}`
  let value = formatters.get(key)
  if (!value) {
    value = new Intl.NumberFormat(locale.value, compact ? { notation: 'compact', maximumFractionDigits: 1 } : {})
    formatters.set(key, value)
  }
  return value
}

const metricUnits = [
  ['P', 1_000_000_000_000_000],
  ['T', 1_000_000_000_000],
  ['B', 1_000_000_000],
  ['M', 1_000_000],
  ['K', 1_000],
] as const

export function formatInteger(value: number) {
  return formatter().format(value)
}

export function formatLocalizedCompactNumber(value: number) {
  return formatter(true).format(Number.isFinite(value) ? value : 0)
}

export function formatCompactNumber(value: number) {
  const normalized = Math.max(0, Math.round(value))
  if (normalized < 1_000)
    return formatInteger(normalized)

  for (const [unit, threshold] of metricUnits) {
    if (normalized < threshold)
      continue

    const scaled = normalized / threshold
    const rounded = scaled >= 10 ? scaled.toFixed(1) : scaled.toFixed(2)
    return `${rounded.replace(/\.?0+$/, '')}${unit}`
  }

  return formatInteger(normalized)
}
