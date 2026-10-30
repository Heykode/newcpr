<script setup lang="ts">
import type { UsageQueryState } from '../composables/useUsageInsights'
import type { getUsageCost, getUsageHealth, getUsagePerformance, getUsageRecordInsightsDiagnostics, UsageCostMetric, UsagePerformanceMetric } from '@/api'
import { computed } from 'vue'
import { t } from '@/i18n'
import { costView, emptyDiagnostics, emptyOverview, metricActivity, performanceView } from '../utils/insights'
import UsageCostCard from './UsageCostCard.vue'
import UsageDiagnosticCard from './UsageDiagnosticCard.vue'
import UsageHealthCard from './UsageHealthCard.vue'
import UsagePerformanceCard from './UsagePerformanceCard.vue'

const props = defineProps<{
  health: UsageQueryState<Awaited<ReturnType<typeof getUsageHealth>>>
  performance: UsageQueryState<Awaited<ReturnType<typeof getUsagePerformance>>>
  cost: UsageQueryState<Awaited<ReturnType<typeof getUsageCost>>>
  diagnostics: UsageQueryState<Awaited<ReturnType<typeof getUsageRecordInsightsDiagnostics>>>
}>()
defineEmits<{ diagnosticPageChange: [page: number] }>()
const diagnosticDimension = defineModel('diagnosticDimension', { type: String, default: 'model' })
const performanceMetric = defineModel<UsagePerformanceMetric>('performanceMetric', { default: 'latency' })
const costMetric = defineModel<UsageCostMetric>('costMetric', { default: 'cost' })
const performanceViewName = computed({
  get: () => performanceMetric.value === 'latency' ? 'total' : performanceMetric.value,
  set: (value: string) => { performanceMetric.value = value === 'total' ? 'latency' : value as UsagePerformanceMetric },
})
const costViewName = computed({
  get: () => costMetric.value,
  set: (value: string) => { costMetric.value = value as UsageCostMetric },
})
const shownPerformance = computed(() => performanceView(props.performance.data))
const shownCost = computed(() => costView(props.cost.data))
</script>

<template>
  <section
    class="mt-5 grid grid-cols-1 gap-3 xl:auto-rows-[minmax(24.25rem,auto)] xl:grid-cols-2"
    :aria-label="t('usage.insights.useStatisticalObservations')"
  >
    <UsageHealthCard
      :health="health.data?.data ?? emptyOverview().health"
      :granularity="health.data?.granularity ?? ''"
      :loading="health.loading"
      :error="health.error"
    />
    <UsageDiagnosticCard
      v-model:dimension="diagnosticDimension"
      :diagnostics="diagnostics.data ?? { ...emptyDiagnostics(), dimension: diagnosticDimension }"
      :loading="diagnostics.loading"
      :error="diagnostics.error"
      @page-change="$emit('diagnosticPageChange', $event)"
    />
    <UsagePerformanceCard
      v-model:view="performanceViewName"
      :performance="shownPerformance"
      :activity="metricActivity(performance.data)"
      :loading="performance.loading"
      :error="performance.error"
    />
    <UsageCostCard
      v-model:view="costViewName"
      :cost="shownCost"
      :activity="metricActivity(cost.data)"
      :loading="cost.loading"
      :error="cost.error"
    />
  </section>
</template>
