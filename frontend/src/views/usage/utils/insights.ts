import type { getUsageRecordInsightsDiagnostics, getUsageRecordInsightsOverview, getUsageRecordSummary, UsageCostMetric, UsageMetricResponse, UsageOverviewPerformanceMetrics, UsagePerformanceMetric } from '@/api'

export function emptySummary() {
  const summary: Awaited<ReturnType<typeof getUsageRecordSummary>> = {
    totalRequests: '0',
    inputTokens: '0',
    outputTokens: '0',
    cachedTokens: '0',
    cacheWriteTokens: '0',
    totalTokens: '0',
    averageLatencyMs: '0 ms',
  }
  return summary
}

export function emptyInsights() {
  return {
    overview: emptyOverview(),
    diagnostics: emptyDiagnostics(),
  }
}

export function emptyOverview() {
  const overview: Awaited<ReturnType<typeof getUsageRecordInsightsOverview>> = {
    granularity: '',
    health: {
      totalRequests: 0,
      successRequests: 0,
      failedRequests: 0,
      cancelledRequests: 0,
      incompleteRequests: 0,
      callerErrorRequests: 0,
      successRate: 0,
      completionRate: 0,
      requestChangeRate: null,
      successRateChange: null,
      points: [],
    },
    performance: {
      admissionDecisionP50Ms: null,
      admissionDecisionP95Ms: null,
      accountSelectionWaitP50Ms: null,
      accountSelectionWaitP95Ms: null,
      capacityUtilization: null,
      capacityUtilizationP95: null,
      latencyCoverage: 0,
      firstTokenCoverage: 0,
      admissionDecisionCoverage: 0,
      accountSelectionWaitCoverage: 0,
      capacityCoverage: 0,
      latencyP50Ms: null,
      latencyP95Ms: null,
      latencyP99Ms: null,
      firstTokenP50Ms: null,
      firstTokenP95Ms: null,
      firstTokenP99Ms: null,
      outputThroughputP10: null,
      outputThroughputP50: null,
      outputThroughputP90: null,
      load: {
        averageConcurrency: 0,
        peakConcurrency: 0,
        admissionDecisionP95Ms: null,
        accountSelectionWaitP95Ms: null,
      },
      points: [],
    },
    cost: {
      estimatedCost: null,
      standardCost: null,
      noCacheCost: null,
      cacheSavings: null,
      tierPremium: null,
      costPerRequest: null,
      costPerSuccessfulRequest: null,
      tokensPerRequest: 0,
      cachedTokenRate: 0,
      cacheHitRequestRate: 0,
      inputTokens: 0,
      outputTokens: 0,
      cachedTokens: 0,
      totalTokens: 0,
      points: [],
      coverage: { known: 0, partial: 0, unknown: 0, notBillable: 0 },
    },
  }
  return overview
}

export function emptyDiagnostics() {
  const diagnostics: Awaited<ReturnType<typeof getUsageRecordInsightsDiagnostics>> = {
    dimension: 'model',
    items: [],
    currentPage: 1,
    pageSize: 100,
    hasMore: false,
  }
  return diagnostics
}

export function performanceView(response: UsageMetricResponse<UsagePerformanceMetric> | null) {
  const value = emptyOverview().performance
  if (!response)
    return value
  if (response.metric === 'scheduling')
    return response.data
  function metrics(point: object): UsageOverviewPerformanceMetrics {
    const base = { ...emptyOverview().performance }
    if (response?.metric === 'load')
      return { ...base, load: point as UsageOverviewPerformanceMetrics['load'] }
    if (response?.metric === 'throughput') {
      const data = point as { p10: number | null, p50: number | null, p90: number | null }
      return { ...base, outputThroughputP10: data.p10, outputThroughputP50: data.p50, outputThroughputP90: data.p90 }
    }
    const data = point as { p50Ms: number | null, p95Ms: number | null, p99Ms: number | null }
    return response?.metric === 'firstToken'
      ? { ...base, firstTokenP50Ms: data.p50Ms, firstTokenP95Ms: data.p95Ms, firstTokenP99Ms: data.p99Ms }
      : { ...base, latencyP50Ms: data.p50Ms, latencyP95Ms: data.p95Ms, latencyP99Ms: data.p99Ms }
  }
  return {
    ...value,
    ...metrics(response.data.summary),
    points: response.data.points.map(point => ({ bucket: point.bucket, label: point.label, ...metrics(point.value) })),
  }
}

export function costView(response: UsageMetricResponse<UsageCostMetric> | null) {
  const value = emptyOverview().cost
  if (!response)
    return value
  const points = response.data.points.map((point) => {
    const base = { bucket: point.bucket, label: point.label, inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, estimatedCost: null, standardCost: null, noCacheCost: null, cacheSavings: null, cachedTokenRate: 0, cacheHitRequestRate: null }
    if (response.metric === 'savings')
      return { ...base, cacheSavings: point.value as string | null }
    return { ...base, ...(point.value as object) }
  })
  return { ...value, ...response.data.summary, points }
}

export function metricActivity(response: UsageMetricResponse<UsagePerformanceMetric | UsageCostMetric> | null) {
  if (!response || response.metric === 'scheduling')
    return []
  return response.data.points.map(point => ({
    bucket: point.bucket,
    label: point.label,
    totalRequests: point.requestCount,
    successRequests: 0,
    failedRequests: 0,
    cancelledRequests: 0,
    incompleteRequests: 0,
    callerErrorRequests: 0,
    errorRate: 0,
  }))
}
