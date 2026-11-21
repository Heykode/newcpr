export { formatCompactNumber, formatInteger } from '@/utils/number'
export function formatUsd(value: string | number | null) {
  if (value === null)
    return '—'
  return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 10 }).format(Number(value))
}
export function formatPercent(value: number | null) {
  return value === null ? '—' : `${(value * 100).toFixed(1)}%`
}
