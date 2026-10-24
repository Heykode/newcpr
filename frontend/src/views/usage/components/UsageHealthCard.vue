<script setup lang="ts">
import type { BarSeriesOption, EChartsOption, LineSeriesOption } from 'echarts'
import type { getUsageRecordInsightsOverview } from '@/api'
import { BarChart } from 'echarts/charts'
import { use } from 'echarts/core'
import { computed } from 'vue'

import BaseCard from '@/components/base/BaseCard.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseChart from '@/components/charts/BaseChart.vue'
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
  usageTooltipContent,
  usageTooltipItem,
  usageValueAxis,
} from '../utils/chart'
import { formatPercent } from '../utils/format'

type Health = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['health']
type HealthPoint = Health['points'][number]

const props = withDefaults(
  defineProps<{
    health: Health
    granularity: string
    loading?: boolean
  }>(),
  {
    loading: false,
  },
)

use([BarChart])

const { palette } = useChartPalette()
const points = computed<HealthPoint[]>(() => props.health.points)

const hasData = computed(
  () => !props.loading && points.value.some(point => requestCount(point) > 0),
)

const granularityText = computed(() => {
  const labels: Record<string, string> = {
    '15m': t('usage.health.point.minutes'),
    '1h': t('usage.health.hours'),
    '1d': t('usage.health.day'),
    'day': t('usage.health.day'),
    'hour': t('usage.health.hours'),
  }
  return labels[props.granularity] ?? props.granularity
})

const chartOption = computed<EChartsOption>(() => {
  const theme = palette.value
  const chartPoints = points.value
  const activePointCount = chartPoints.filter(point => requestCount(point) > 0).length
  const successRateSeries = usageGapAwareLineSeries(
    t('usage.health.serviceSuccessRate'),
    chartPoints.map(serviceSuccessRate),
    theme.success,
    { yAxisIndex: 1 },
  )
  const series: Array<BarSeriesOption | LineSeriesOption> = [
    outcomeSeries(t('common.status.success'), chartPoints.map(point => point.successRequests), theme.success),
    outcomeSeries(t('usage.health.point.serviceFailed'), chartPoints.map(point => point.failedRequests), theme.danger),
    outcomeSeries(t('usage.status.cancelled'), chartPoints.map(point => point.cancelledRequests), theme.info),
    outcomeSeries(t('usage.health.notCompleted'), chartPoints.map(point => point.incompleteRequests), theme.warning),
    outcomeSeries(t('usage.health.point.callerError'), chartPoints.map(point => point.callerErrorRequests), theme.normal),
    ...successRateSeries.map((item, index) => index === 0
      ? {
          ...item,
          symbol: activePointCount <= 16 ? 'circle' : 'none',
          symbolSize: 4,
        }
      : item),
  ]

  return {
    animationDuration: 240,
    grid: {
      left: 0,
      right: 0,
      top: 40,
      bottom: 0,
      outerBoundsMode: 'same',
      outerBoundsContain: 'axisLabel',
    },
    legend: usageLegend(theme, [t('common.status.success'), t('usage.health.point.serviceFailed'), t('usage.status.cancelled'), t('usage.health.notCompleted'), t('usage.health.point.callerError'), t('usage.health.serviceSuccessRate')]),
    tooltip: usageTooltip(theme, formatTooltip),
    xAxis: usageCategoryAxis(
      chartPoints.map(point => point.label),
      theme,
    ),
    yAxis: [
      usageValueAxis(theme, value => formatCompactNumber(value)),
      usageValueAxis(theme, value => formatPercent(value), {
        min: 0,
        max: 1,
        splitLine: false,
      }),
    ],
    series,
  }
})

function requestCount(point: HealthPoint) {
  return Math.max(0, point.totalRequests ?? 0)
}

function serviceSuccessRate(point: HealthPoint) {
  const total = Math.max(0, point.successRequests) + Math.max(0, point.failedRequests) + Math.max(0, point.incompleteRequests ?? 0)
  if (total > 0)
    return Math.max(0, point.successRequests ?? 0) / total
  return requestCount(point) === 0 ? 0 : null
}

function outcomeSeries(name: string, data: number[], color: string) {
  return {
    name,
    type: 'bar' as const,
    stack: 'outcome',
    data,
    barMaxWidth: 24,
    itemStyle: { color, opacity: 0.72 },
  }
}

function formatTooltip(params: unknown) {
  const rows = tooltipRows(params)
  const point = points.value[tooltipIndex(rows[0])]
  if (!point)
    return ''

  const successRate = serviceSuccessRate(point)
  const theme = palette.value

  return usageTooltipContent(theme, point.label, [
    usageTooltipItem(t('usage.health.requestVolume'), formatCompactNumber(requestCount(point)), theme.textSecondary),
    usageTooltipItem(t('common.status.success'), formatCompactNumber(point.successRequests), theme.success),
    usageTooltipItem(t('usage.health.point.serviceFailed'), formatCompactNumber(point.failedRequests), theme.danger),
    usageTooltipItem(t('usage.status.cancelled'), formatCompactNumber(point.cancelledRequests), theme.info),
    usageTooltipItem(t('usage.health.notCompleted'), formatCompactNumber(point.incompleteRequests), theme.warning),
    usageTooltipItem(t('usage.health.point.callerError'), formatCompactNumber(point.callerErrorRequests), theme.normal),
    usageTooltipItem(
      t('usage.health.serviceSuccessRate'),
      successRate == null ? t('usage.health.noServiceResult') : formatPercent(successRate),
      theme.success,
    ),
  ])
}
</script>

<template>
  <BaseCard
    as="article"
    :title="t('usage.health.title')"
    :description="t('usage.health.description', { granularityText })"
    class="min-h-90 xl:h-full"
  >
    <template #body>
      <div class="grid min-h-66" :class="hasData ? 'gap-3' : 'h-full'">
        <div v-if="hasData" class="grid grid-cols-3 gap-2 rounded-xl bg-cp-fill-quaternary/45 p-2">
          <div class="grid gap-1 px-2">
            <span class="text-[10px] font-bold text-cp-text-quaternary">{{ t('usage.health.serviceSuccessRate') }}</span>
            <strong class="font-mono text-cp-base font-heavy tabular-nums text-cp-green-text">
              {{ formatPercent(health.successRate) }}
            </strong>
          </div>
          <div class="grid gap-1 px-2">
            <span class="text-[10px] font-bold text-cp-text-quaternary">{{ t('usage.health.completionRate') }}</span>
            <strong class="font-mono text-cp-base font-heavy tabular-nums text-cp-text">
              {{ formatPercent(health.completionRate) }}
            </strong>
          </div>
          <div class="grid gap-1 px-2">
            <span class="text-[10px] font-bold text-cp-text-quaternary">{{ t('usage.health.abnormalEnd') }}</span>
            <strong class="font-mono text-cp-base font-heavy tabular-nums text-cp-orange-text">
              {{ formatCompactNumber(health.cancelledRequests + health.incompleteRequests) }}
            </strong>
          </div>
        </div>
        <BaseChart v-if="hasData" :option="chartOption" :height="210" />
        <BaseEmpty
          v-else
          size="sm"
          surface="none"
          :title="loading ? '正在加载请求健康数据' : '暂无请求健康数据'"
          description="当前范围没有可绘制的请求记录"
          class="h-full place-content-center"
        />
      </div>
    </template>
  </BaseCard>
</template>
