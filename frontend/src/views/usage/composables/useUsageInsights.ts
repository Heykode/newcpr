import type { UsageCostMetric, UsagePerformanceMetric } from '@/api'
import type { RequestOptions } from '@/api/request'
import { computed, shallowRef, watch } from 'vue'
import { getUsageCost, getUsageHealth, getUsagePerformance, getUsageRecordInsightsDiagnostics, getUsageRecordSummary } from '@/api'
import { useRequestState } from '@/composables/useRequestState'

type UsageRange = Parameters<typeof getUsageRecordSummary>[0]

export interface UsageQueryState<T> {
  data: T | null
  loading: boolean
  error: string
}

export function useUsageInsights(scopedParams: () => UsageRange) {
  const performanceMetric = shallowRef<UsagePerformanceMetric>('latency')
  const costMetric = shallowRef<UsageCostMetric>('cost')
  const diagnosticDimension = shallowRef('model')
  const diagnosticPage = shallowRef(1)
  const summary = useUsageQuery(getUsageRecordSummary)
  const health = useUsageQuery(getUsageHealth)
  const performance = useUsageQuery((params, options) => getUsagePerformance({ ...params, metric: performanceMetric.value }, options))
  const cost = useUsageQuery((params, options) => getUsageCost({ ...params, metric: costMetric.value }, options))
  const diagnostics = useUsageQuery((params, options) => getUsageRecordInsightsDiagnostics({ ...params, dimension: diagnosticDimension.value, ...(diagnosticDimension.value === 'keyModel' ? { currentPage: diagnosticPage.value, pageSize: 20 } : {}) }, options))

  watch(performanceMetric, () => void performance.load(scopedParams()))
  watch(costMetric, () => void cost.load(scopedParams()))
  watch(diagnosticDimension, () => {
    diagnosticPage.value = 1
    void diagnostics.load(scopedParams())
  })

  function handleDiagnosticPageChange(page: number) {
    const current = diagnostics.state.value.data
    if (diagnostics.state.value.loading || diagnosticDimension.value !== 'keyModel'
      || !current || !Number.isInteger(page) || page < 1
      || (page !== current.currentPage - 1 && !(page === current.currentPage + 1 && current.hasMore))) {
      return
    }
    diagnosticPage.value = page
    void diagnostics.load(scopedParams(), true)
  }

  function invalidate() {
    for (const query of [summary, health, performance, cost, diagnostics]) query.invalidate()
  }

  function loadUsageAnalytics(params: UsageRange = scopedParams()) {
    diagnosticPage.value = 1
    // 各查询收到结果即发布，慢指标不会阻塞其他卡片
    return Promise.all([
      summary,
      health,
      performance,
      cost,
      diagnostics,
    ].map(query => query.load(params)))
  }

  return {
    summary: summary.state,
    health: health.state,
    performance: performance.state,
    cost: cost.state,
    diagnostics: diagnostics.state,
    performanceMetric,
    costMetric,
    diagnosticDimension,
    loadUsageAnalytics,
    handleDiagnosticPageChange,
    invalidate,
  }
}

function useUsageQuery<T>(fetch: (params: UsageRange, options: RequestOptions) => Promise<T>) {
  const data = shallowRef<T | null>(null)
  const request = useRequestState()
  const state = computed<UsageQueryState<T>>(() => ({ data: data.value, loading: request.loading.value, error: request.error.value }))

  async function load(params: UsageRange, preserveData = false) {
    const id = request.start()
    if (!preserveData)
      data.value = null
    try {
      const result = await fetch(params, { signal: request.signal })
      if (request.isCurrent(id))
        data.value = result
    }
    catch (cause) {
      request.fail(id, cause)
    }
    finally {
      request.finish(id)
    }
  }

  function invalidate() {
    request.invalidate()
    data.value = null
  }

  return { state, load, invalidate }
}
