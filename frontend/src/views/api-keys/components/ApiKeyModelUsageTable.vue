<script setup lang="ts">
import type { KeyModelRow, KeyStatistics, KeyStatisticsPeriod, KeyUsageMetric } from '@/api'
import { useElementVisibility } from '@vueuse/core'
import { computed, shallowRef, useTemplateRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getKeyModelTotals } from '@/api'
import ZButton from '@/components/base/BaseButton.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import ZTable from '@/components/base/BaseTable/index.vue'
import { useRequestState } from '@/composables/useRequestState'
import { formatInteger, formatPercent, formatUsd } from '../utils/statistics-format'

const props = defineProps<{ active: boolean, keyId: string, statistics: KeyStatistics | null, pending: boolean, period: KeyStatisticsPeriod, metric: KeyUsageMetric }>()
const { t } = useI18n({ useScope: 'global' })
const container = useTemplateRef<HTMLElement>('container')
const visible = useElementVisibility(container)
const request = useRequestState()
const { error, loading } = request
const result = shallowRef<{ asOf: number, period: KeyStatisticsPeriod, rows: KeyModelRow[], cost: number }>()
const rows = computed(() => [
  ...(result.value?.rows ?? []),
].sort((a, b) =>
  Number(b.totals[props.metric] ?? -1) - Number(a.totals[props.metric] ?? -1) || a.model.localeCompare(b.model)))

async function load() {
  if (!props.active || !props.statistics || loading.value)
    return
  const { asOf, modelCount, totals } = props.statistics
  const period = props.period
  if (result.value?.asOf === asOf && result.value.period === period)
    return
  if (modelCount && !visible.value)
    return
  const id = request.start()
  try {
    const rows = modelCount
      ? await getKeyModelTotals(props.keyId, period, asOf, { signal: request.signal, silent: true })
      : []
    if (request.isCurrent(id)) {
      result.value = {
        asOf,
        period,
        rows,
        cost: Number(totals.costUsd),
      }
    }
  }
  catch (cause) {
    request.fail(id, cause)
  }
  finally {
    request.finish(id)
  }
}
watch(() => props.active, (active) => {
  if (!active)
    request.invalidate()
}, { flush: 'sync' })
watch([
  () => props.statistics?.asOf,
  () => props.period,
], () => {
  request.invalidate()
  void load()
}, { immediate: true })
watch(visible, (shown) => {
  if (shown)
    void load()
})
const columns = computed(() => defineTableColumns<KeyModelRow>([
  {
    key: 'model',
    label: t('keys.statistics.model'),
    kind: 'text',
    size: 'xl',
    emptyText: t('keys.statistics.unknownModel'),
  },
  {
    key: 'requests',
    label: t('keys.statistics.requests'),
    kind: 'numeric',
    size: 'sm',
    format: (_, row) => formatInteger(row.totals.requests),
  },
  {
    key: 'tokens',
    label: 'Token',
    kind: 'numeric',
    format: (_, row) => row.totals.tokens === null
      ? '—'
      : formatInteger(row.totals.tokens),
  },
  {
    key: 'costUsd',
    label: t('keys.statistics.costUsd'),
    kind: 'numeric',
    size: 'sm',
    format: (_, row) => formatUsd(row.totals.costUsd),
  },
  {
    key: 'costShare',
    label: t('keys.statistics.costShare'),
    kind: 'numeric',
    size: 'lg',
    format: (_, row) => formatPercent(row.totals.costUsd !== null && result.value?.cost
      ? Number(row.totals.costUsd) / result.value.cost
      : null),
  },
  {
    key: 'peakRequests',
    label: t('keys.statistics.peakRequests'),
    kind: 'numeric',
    size: 'lg',
    format: (_, row) => formatInteger(row.peakRequests),
  },
  {
    key: 'activeDays',
    label: t('keys.statistics.activeDays'),
    kind: 'numeric',
    size: 'sm',
  },
]))
</script>

<template>
  <div ref="container" tabindex="-1" class="grid min-w-0 gap-3">
    <div v-if="error" class="flex items-center gap-2 text-cp-sm text-cp-error-text" role="alert">
      {{ error }} <ZButton size="sm" @click="container?.focus(); load()">
        {{ t('common.actions.refresh') }}
      </ZButton>
    </div>
    <ZTable class="h-80 min-w-0" :class="{ 'min-h-40!': !rows.length }" :columns="columns" :rows="rows" :loading="pending || loading" row-key="model" size="sm" show-header-when-empty>
      <template #empty>
        <span v-if="!error && result" class="text-cp-sm text-cp-text-secondary">{{ t('keys.statistics.empty') }}</span>
      </template>
      <template #costUsd="{ row, displayValue }">
        <span :title="[row.totals.hasEstimatedCost && t('keys.statistics.includesEstimate'), row.totals.hasMissingCost && t('keys.statistics.knownCost')].filter(Boolean).join(' · ') || undefined">{{ displayValue }}</span>
      </template>
    </ZTable>
  </div>
</template>
