<script setup lang="ts">
import type { EChartsOption } from 'echarts'

import type { getUsageRecordInsightsOverview } from '@/api'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'

import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseChart from '@/components/charts/BaseChart.vue'
import { requestActivityByBucket, zeroInactiveValues } from '@/components/charts/timeSeriesGap'
import { chartTooltipContent } from '@/components/charts/tooltip'
import { useChartPalette } from '@/composables/useChartPalette'
import { t } from '@/i18n'
import { formatLocalizedCompactNumber as formatCompactNumber } from '@/utils/number'
import {
  tooltipIndex,
  tooltipRows,
  usageCategoryAxis,
  usageGapAwareLineSeries,
  usageLegend,
  usageTooltip,
  usageTooltipItem,
  usageValueAxis,
} from '../utils/chart'

import { formatDuration, formatDurationAxis, formatPercent } from '../utils/format'

const props = withDefaults(
  defineProps<{
    performance: Performance
    activity: Activity
    loading?: boolean
    error?: string
  }>(),
  {
    loading: false,
    error: '',
  },
)

type Performance = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['performance']
type PerformancePoint = Performance['points'][number]
type PerformanceMetric = Exclude<keyof PerformancePoint, 'bucket' | 'label' | 'load'>

interface PerformanceSeries {
  field: PerformanceMetric | keyof PerformancePoint['load']
  value: (point: Omit<PerformancePoint, 'bucket' | 'label'>) => number | null
  label: string
  color: string
  yAxisIndex: number
  format: (value: number | null) => string
}

type Activity = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['health']['points']

const activeView = defineModel<string>('view', { default: 'total' })
const { palette } = useChartPalette()
const performancePoints = computed<PerformancePoint[]>(() => props.performance.points)
const requestActivity = computed(() => requestActivityByBucket(
  performancePoints.value.map(point => point.bucket),
  props.activity,
))

const viewOptions = computed(() => ([
  {
    label: t('usage.latency.totalLatency'),
    value: 'total',
  },
  {
    label: t('usage.ttft'),
    value: 'firstToken',
  },
  {
    label: t('usage.outputThroughput'),
    value: 'throughput',
  },
  {
    label: t('usage.performance.scheduling'),
    value: 'scheduling',
  },
  {
    label: t('usage.performance.load'),
    value: 'load',
  },
]))

const seriesDefinitions = computed<PerformanceSeries[]>(() => {
  const theme = palette.value
  if (activeView.value === 'load') {
    return [
      {
        field: 'averageConcurrency',
        value: point => point.load.averageConcurrency,
        label: t('usage.performance.averageConcurrency'),
        color: theme.info,
        yAxisIndex: 0,
        format: formatConcurrency,
      },
      {
        field: 'peakConcurrency',
        value: point => point.load.peakConcurrency,
        label: t('usage.performance.peakConcurrency'),
        color: theme.warning,
        yAxisIndex: 0,
        format: formatConcurrency,
      },
      {
        field: 'admissionDecisionP95Ms',
        value: point => point.load.admissionDecisionP95Ms,
        label: t('usage.performance.admissionP95'),
        color: theme.success,
        yAxisIndex: 1,
        format: formatDuration,
      },
      {
        field: 'accountSelectionWaitP95Ms',
        value: point => point.load.accountSelectionWaitP95Ms,
        label: t('usage.performance.accountSelectionP95'),
        color: theme.reasoning,
        yAxisIndex: 1,
        format: formatDuration,
      },
    ]
  }
  if (activeView.value === 'scheduling') {
    return [
      {
        field: 'admissionDecisionP95Ms',
        value: point => point.admissionDecisionP95Ms,
        label: t('usage.performance.admissionP95'),
        color: theme.info,
        yAxisIndex: 0,
        format: formatDuration,
      },
      {
        field: 'accountSelectionWaitP95Ms',
        value: point => point.accountSelectionWaitP95Ms,
        label: t('usage.performance.accountSelectionP95'),
        color: theme.warning,
        yAxisIndex: 0,
        format: formatDuration,
      },
      {
        field: 'capacityUtilizationP95',
        value: point => point.capacityUtilizationP95,
        label: t('usage.performance.capacityP95'),
        color: theme.success,
        yAxisIndex: 1,
        format: value => formatPercent(value),
      },
    ]
  }
  const throughput = activeView.value === 'throughput'
  const fields: PerformanceMetric[] = throughput
    ? ['outputThroughputP10', 'outputThroughputP50', 'outputThroughputP90']
    : activeView.value === 'firstToken'
      ? ['firstTokenP50Ms', 'firstTokenP95Ms', 'firstTokenP99Ms']
      : ['latencyP50Ms', 'latencyP95Ms', 'latencyP99Ms']
  const labels = throughput
    ? ['P10', 'P50', 'P90']
    : ['P50', 'P95', 'P99']
  const colors = [theme.info, theme.warning, theme.danger]
  return fields.map((field, index) => ({
    field,
    value: point => point[field],
    label: labels[index]!,
    color: colors[index]!,
    yAxisIndex: 0,
    format: throughput ? formatThroughput : formatDuration,
  }))
})

const selectedSeries = computed(() => seriesDefinitions.value.map((series) => {
  const values = performancePoints.value.map(series.value)
  return {
    ...series,
    // 独立调度查询未携带请求数；并发也可能来自前桶，不能把缺失样本补成零。
    values: activeView.value === 'scheduling' || series.field === 'averageConcurrency' || series.field === 'peakConcurrency'
      ? values
      : zeroInactiveValues(values, requestActivity.value),
  }
}))

const summaryMetrics = computed(() => {
  if (activeView.value === 'scheduling') {
    return [
      { label: t('usage.performance.decisionCoverage'), value: formatPercent(props.performance.admissionDecisionCoverage) },
      { label: t('usage.performance.accountSelectionCoverage'), value: formatPercent(props.performance.accountSelectionWaitCoverage) },
      { label: t('usage.performance.capacityCoverage'), value: formatPercent(props.performance.capacityCoverage) },
    ]
  }
  return seriesDefinitions.value.map(series => ({
    label: series.label,
    value: series.format(series.value(props.performance)),
  }))
})

const hasData = computed(() => !props.loading
  && seriesDefinitions.value.some(series => performancePoints.value.some(point => series.value(point) != null)))

const chartOption = computed<EChartsOption>(() => {
  const theme = palette.value
  const load = activeView.value === 'load'
  const series = selectedSeries.value.flatMap((item, index) => usageGapAwareLineSeries(
    item.label,
    item.values,
    item.color,
    {
      yAxisIndex: item.yAxisIndex,
      area: index === 0
        ? 'subtle'
        : false,
    },
  ))

  return {
    animationDuration: 240,
    grid: {
      left: 0,
      right: 0,
      top: load ? 60 : 40,
      bottom: 0,
      outerBoundsMode: 'same',
      outerBoundsContain: 'axisLabel',
    },
    legend: usageLegend(theme, selectedSeries.value.map(series => series.label)),
    tooltip: usageTooltip(theme, formatTooltip),
    xAxis: usageCategoryAxis(performancePoints.value.map(point => point.label), theme),
    yAxis: load
      ? [
          {
            ...usageValueAxis(theme, formatCompactNumber, { min: 0 }),
            name: t('usage.performance.concurrency'),
            nameTextStyle: { color: theme.textMuted, fontSize: 10, align: 'left' },
          },
          {
            ...usageValueAxis(theme, formatDurationAxis, { min: 0, splitLine: false }),
            name: t('usage.performance.waitTime'),
            nameTextStyle: { color: theme.textMuted, fontSize: 10, align: 'right' },
          },
        ]
      : activeView.value === 'scheduling'
        ? [
            usageValueAxis(theme, formatDurationAxis),
            usageValueAxis(theme, value => formatPercent(value), {
              min: 0,
              max: 1,
              splitLine: false,
            }),
          ]
        : usageValueAxis(
            theme,
            activeView.value === 'throughput'
              ? value => `${formatCompactNumber(value)}/s`
              : formatDurationAxis,
          ),
    series,
  }
})

function formatTooltip(params: unknown) {
  const rows = tooltipRows(params)
  const index = tooltipIndex(rows[0])
  const point = performancePoints.value[index]
  if (!point)
    return ''

  return chartTooltipContent(point.label, selectedSeries.value.map(series => usageTooltipItem(
    series.label,
    series.format(series.values[index] ?? null),
    series.color,
  )))
}

function formatConcurrency(value: number | null) {
  return value == null
    ? '—'
    : formatCompactNumber(value)
}

function formatThroughput(value: number | null) {
  return value == null
    ? '—'
    : `${formatCompactNumber(value)} tok/s`
}
</script>

<template>
  <BaseCard
    as="article"
    :title="t('usage.performance.title')"
    :description="t('usage.performance.description')"
    class="min-h-90 xl:h-full"
  >
    <template #actions>
      <div class="flex justify-end">
        <BaseSegmented v-model="activeView" :label="t('usage.performance.performanceIndicators')" :options="viewOptions" :disabled="loading" />
      </div>
    </template>

    <template #body>
      <div class="grid min-h-66 gap-3">
        <div
          v-if="hasData"
          class="grid gap-2 rounded-lg bg-cp-fill-quaternary/45 p-2"
          :class="activeView === 'load' ? 'grid-cols-2 sm:grid-cols-4' : 'grid-cols-3'"
        >
          <div v-for="metric in summaryMetrics" :key="metric.label" class="grid min-w-0 gap-1 px-2">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary" :title="metric.label">{{ metric.label }}</span>
            <strong class="truncate font-mono text-cp-base font-heavy tabular-nums text-cp-text" :title="metric.value">
              {{ metric.value }}
            </strong>
          </div>
        </div>
        <BaseChart v-if="hasData" :option="chartOption" :height="210" />
        <BaseEmpty
          v-else
          size="sm"
          surface="none"
          :title="
            error || (loading
              ? t('usage.performance.loading')
              : activeView === 'load'
                ? t('usage.performance.noLoad')
                : activeView === 'firstToken'
                  ? t('usage.performance.noTtftSamples')
                  : activeView === 'throughput'
                    ? t('usage.performance.noThroughput')
                    : activeView === 'scheduling'
                      ? t('usage.performance.noScheduling')
                      : t('usage.performance.noLatency'))
          "
          class="h-52.5 place-content-center"
        />
      </div>
    </template>
  </BaseCard>
</template>
