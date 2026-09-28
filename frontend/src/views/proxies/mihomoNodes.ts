import type { MihomoNode, NodeCheck } from '@/api/modules/mihomo'

export interface NodeFilters { source: string, state: string, search: string, dynamicOnly: boolean }
export type BatchOutcome = 'passed' | 'warning' | 'challenge' | 'failed' | 'skipped'

export function filterNodes(nodes: readonly MihomoNode[], filters: NodeFilters): MihomoNode[] {
  const query = filters.search.trim().toLowerCase()
  return nodes.filter(node =>
    (!filters.dynamicOnly || node.dynamic)
    && (filters.source === 'all'
      || (filters.source === 'dynamic' && node.dynamic)
      || (filters.source === 'subscription' && !node.dynamic)
      || (filters.source.startsWith('source:') && !node.dynamic && node.subscriptionIds.includes(filters.source.slice('source:'.length))))
    && (filters.state === 'all' || (filters.state === 'blocked' ? node.countryBlocked : node.state === filters.state))
    && `${node.displayName} ${node.name} ${node.countryCode ?? ''}`.toLowerCase().includes(query),
  )
}

export function batchTargets(nodes: readonly MihomoNode[], selected: ReadonlySet<string>): MihomoNode[] {
  return selected.size ? nodes.filter(node => selected.has(node.name)) : [...nodes]
}

export function checkOutcome(result: NodeCheck, quality: boolean): BatchOutcome {
  if (!quality)
    return result.success ? 'passed' : 'failed'
  const checks = result.quality?.checks ?? []
  if (checks.some(check => check.status === 'challenge'))
    return 'challenge'
  if (!result.success || !checks.length || checks.some(check => check.status === 'fail'))
    return 'failed'
  return checks.every(check => check.status === 'pass') ? 'passed' : 'warning'
}

export async function runNodeBatch(
  nodes: readonly MihomoNode[],
  concurrency: number,
  run: (node: MihomoNode) => Promise<BatchOutcome>,
  onResult: (outcome: BatchOutcome) => void,
  shouldStop: () => boolean,
): Promise<void> {
  let index = 0
  const worker = async () => {
    while (index < nodes.length && !shouldStop()) {
      const node = nodes[index++]!
      let outcome: BatchOutcome
      try {
        outcome = await run(node)
      }
      catch { outcome = 'failed' }
      if (!shouldStop())
        onResult(outcome)
    }
  }
  await Promise.all(Array.from({ length: Math.min(concurrency, nodes.length) }, worker))
}
