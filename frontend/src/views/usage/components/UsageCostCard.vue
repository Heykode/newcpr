<script setup lang="ts">
import type { EChartsOption } from 'echarts'
import type { getUsageRecordInsightsOverview } from '@/api'
import { computed } from 'vue'

import BaseCard from '@/components/base/BaseCard.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseChart from '@/components/charts/BaseChart.vue'
import {
  requestActivityByBucket,
  zeroInactiveValues,
} from '@/components/charts/timeSeriesGap'
import { chartTooltipContent } from '@/components/charts/tooltip'
import { useChartPalette } from '@/composables/useChartPalette'
import { t } from '@/i18n'

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
import {
  decimalDisplayNumber,
  formatPercent,
  formatUsd,
  formatUsdAxis,
} from '../utils/format'

type Cost = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['cost']
type Activity = Awaited<ReturnType<typeof getUsageRecordInsightsOverview>>['health']['points']
interface CostSeries {
  label: string
  color: string
  values: (number | null)[]
  area?: 'strong'
  tooltipValue: (index: number) => string | null
}

const props = withDefaults(
  defineProps<{
    cost: Cost
    activity: Activity
    loading?: boolean
    error?: string
  }>(),
  {
    loading: false,
    error: '',
  },
)

const activeView = defineModel<string>('view', { default: 'cost' })
const { palette } = useChartPalette()
const points = computed(() => props.cost.points)
const requestActivity = computed(() => requestActivityByBucket(
  points.value.map(point => point.bucket),
  props.activity,
))
const coverageRate = computed(() => {
  const { known, partial, unknown } = props.cost.coverage
  const total = known + partial + unknown
  return total > 0 ? (known + partial) / total : null
})

const viewOptions = [
  { label: '费用', value: 'cost' },
  { label: '节省', value: 'savings' },
  { label: '缓存', value: 'cache' },
]

function amountValues(amounts: (string | null)[]) {
  return zeroInactiveValues(amounts.map(amount => decimalDisplayNumber(amount)), requestActivity.value)
}

function primaryAmountSeries(label: string, amounts: (string | null)[]): CostSeries {
  return {
    label,
    color: palette.value.success,
    area: 'strong',
    values: amountValues(amounts),
    tooltipValue: index => formatUsd(amounts[index] ?? (requestActivity.value[index] === false ? '0' : null)),
  }
}

function rateSeries(label: string, color: string, values: (number | null)[]): CostSeries {
  return { label, color, values, tooltipValue: index => formatPercent(values[index] ?? null) }
}

const chart = computed<{ hasData: boolean, series: CostSeries[] }>(() => {
  const chartPoints = points.value
  if (props.loading || chartPoints.length === 0)
    return { hasData: false, series: [] }

  if (activeView.value === 'cost') {
    const actual = chartPoints.map(point => point.estimatedCost)
    const comparison = chartPoints.map(point => point.noCacheCost)
    const hasComparison = comparison.some(amount => amount != null)
    const series = [primaryAmountSeries(t('usage.cost.actualCost'), actual)]
    if (hasComparison) {
      series.push({
        label: t('usage.cost.withoutCacheEstimate'),
        color: palette.value.textMuted,
        values: amountValues(comparison),
        // 对比曲线允许补零，提示只展示该时间桶实际提供的估算
        tooltipValue: index => comparison[index] == null ? null : formatUsd(comparison[index]),
      })
    }
    return { hasData: hasComparison || actual.some(amount => amount != null), series }
  }
  if (activeView.value === 'savings') {
    const savings = chartPoints.map(point => point.cacheSavings)
    return {
      hasData: savings.some(amount => amount != null),
      series: [primaryAmountSeries(t('usage.cost.cacheSavings'), savings)],
    }
  }
  return {
    hasData: chartPoints.some(point => point.inputTokens > 0 || point.cachedTokenRate > 0 || (point.cacheHitRequestRate ?? 0) > 0),
    series: [
      rateSeries(t('usage.cost.cacheTokenProportion'), palette.value.normal, chartPoints.map(point => point.cachedTokenRate)),
      rateSeries(t('usage.cost.hitRequestRate'), palette.value.success, chartPoints.map(point => point.cacheHitRequestRate)),
    ],
  }
})

const chartOption = computed<EChartsOption>(() => {
  const theme = palette.value
  const series = chart.value.series

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
    legend: usageLegend(theme, series.map(item => item.label)),
    tooltip: usageTooltip(theme, formatTooltip),
    xAxis: usageCategoryAxis(
      points.value.map(point => point.label),
      theme,
    ),
    yAxis: activeView.value === 'cache'
      ? usageValueAxis(theme, formatPercent, { min: 0, max: 1 })
      : usageValueAxis(theme, formatUsdAxis),
    series: series.flatMap(item => usageGapAwareLineSeries(item.label, item.values, item.color, { area: item.area })),
  }
})

function formatTooltip(params: unknown) {
  const index = tooltipIndex(tooltipRows(params)[0])
  const point = points.value[index]
  if (!point)
    return ''
  const lines = chart.value.series.flatMap((series) => {
    const value = series.tooltipValue(index)
    return value == null ? [] : [usageTooltipItem(series.label, value, series.color)]
  })
  return chartTooltipContent(point.label, lines)
}
</script>

<template>
  <BaseCard
    as="article"
    :title="t('usage.cost.costEfficiency')"
    description="实际费用、缓存节省、服务层溢价与单位成本"
    class="min-h-90 xl:h-full"
  >
    <template #actions>
      <BaseSegmented v-model="activeView" :label="t('usage.cost.costView')" :options="viewOptions" :disabled="loading" class="w-50" />
    </template>

    <template #body>
      <div class="grid min-h-66" :class="chart.hasData ? 'gap-3' : 'flex-1'">
        <div v-if="chart.hasData" class="grid grid-cols-5 gap-1 rounded-xl bg-cp-fill-quaternary/45 p-2">
          <div class="grid min-w-0 gap-1 px-1.5">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary">实际费用</span>
            <strong class="truncate font-mono text-cp-sm font-heavy tabular-nums text-cp-text">
              {{ formatUsd(cost.estimatedCost) }}
            </strong>
          </div>
          <div class="grid min-w-0 gap-1 px-1.5">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary">{{ t('usage.cost.cacheSavings') }}</span>
            <strong class="truncate font-mono text-cp-sm font-heavy tabular-nums text-cp-green-text">
              {{ formatUsd(cost.cacheSavings) }}
            </strong>
          </div>
          <div class="grid min-w-0 gap-1 px-1.5">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary">{{ t('usage.cost.tierPremium') }}</span>
            <strong class="truncate font-mono text-cp-sm font-heavy tabular-nums text-cp-orange-text">
              {{ formatUsd(cost.tierPremium) }}
            </strong>
          </div>
          <div class="grid min-w-0 gap-1 px-1.5">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary">每成功请求</span>
            <strong class="truncate font-mono text-cp-sm font-heavy tabular-nums text-cp-text">
              {{ formatUsd(cost.costPerSuccessfulRequest, true) }}
            </strong>
          </div>
          <div class="grid min-w-0 gap-1 px-1.5">
            <span class="truncate text-[10px] font-bold text-cp-text-quaternary">{{ t('usage.cost.costCoverage') }}</span>
            <strong class="truncate font-mono text-cp-sm font-heavy tabular-nums text-cp-text">
              {{ formatPercent(coverageRate) }}
            </strong>
          </div>
        </div>
        <BaseChart v-if="chart.hasData" :option="chartOption" :height="210" />
        <BaseEmpty
          v-else
          size="sm"
          surface="none"
          :title="error || (loading ? '正在加载成本数据' : '暂无成本效率数据')"
          description="当前范围没有可绘制的费用或缓存收益数据"
          class="place-content-center"
        />
      </div>
    </template>
  </BaseCard>
</template>
