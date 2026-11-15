<script setup lang="ts">
import type { getUsageRecordSummary } from '@/api'
import { Activity, Database, FileText, Timer } from '@lucide/vue'
import { computed } from 'vue'

import BaseCard from '@/components/base/BaseCard.vue'
import { t } from '@/i18n'

const props = defineProps<{
  summary: Awaited<ReturnType<typeof getUsageRecordSummary>> | null
  loading?: boolean
  error?: string
}>()

function averageLatencyDisplay(value: string | undefined) {
  return !value || value === '—' || value === '-' ? '0 ms' : value
}

const items = computed(() => [
  {
    key: 'requests',
    label: '成功请求',
    icon: Activity,
    value: props.summary?.totalRequests ?? '0',
    detail: '筛选范围内',
    tone: 'bg-cp-blue-container text-cp-blue-on-container',
  },
  {
    key: 'tokens',
    label: '总 Token',
    icon: FileText,
    value: props.summary?.totalTokens ?? '0',
    detail: `输入 ${props.summary?.inputTokens ?? '0'} / 输出 ${props.summary?.outputTokens ?? '0'}`,
    tone: 'bg-cp-green-container text-cp-green-on-container',
  },
  {
    key: 'cached',
    label: '缓存 Token',
    icon: Database,
    value: props.summary?.cachedTokens ?? '0',
    detail: '缓存读取命中',
    tone: 'bg-cp-orange-container text-cp-orange-on-container',
  },
  {
    key: 'latency',
    label: '平均耗时',
    icon: Timer,
    value: averageLatencyDisplay(props.summary?.averageLatencyMs),
    detail: '成功请求平均值',
    tone: 'bg-cp-cyan-container text-cp-cyan-on-container',
  },
])
</script>

<template>
  <section class="grid shrink-0 grid-cols-2 gap-3 lg:grid-cols-4" :aria-label="t('usage.usageOverview')">
    <BaseCard
      v-for="item in items"
      :key="item.key"
      as="article"
      padding="compact"
      class="relative flex min-h-29 flex-col gap-3"
    >
      <component :is="item.icon" class="absolute right-4 top-4 size-4 text-cp-text-tertiary" aria-hidden="true" />
      <div class="flex min-w-0 flex-1 flex-col justify-between gap-2">
        <span class="block pr-6 text-cp-sm leading-snug text-cp-text-secondary">
          {{ item.label }}
        </span>
        <strong class="block truncate text-[27px] leading-tight font-semibold tracking-tight tabular-nums text-cp-text">
          {{ item.value }}
        </strong>
        <span class="block text-cp-xs leading-snug text-cp-text-secondary">
          {{ error || item.detail }}
        </span>
      </div>
    </BaseCard>
  </section>
</template>
