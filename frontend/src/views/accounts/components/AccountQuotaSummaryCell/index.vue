<script setup lang="ts">
import type { AccountRow } from '../../constants'

import { computed } from 'vue'
import { useUiClock } from '@/composables/useUiClock'
import { groupedAccountQuotaWindows, visibleSummaryQuotaWindows } from '../../constants'
import AccountUsageWindow from '../AccountUsageWindow/index.vue'
import AccountQuotaSummaryEntry from './Entry.vue'
import { weeklyForecastPresentation } from './forecast'
import { recentlyUsedQuotaEntry } from './presenter'

const props = defineProps<{
  account: AccountRow
}>()
const emit = defineEmits<{
  forecastRequested: [account: AccountRow]
}>()

const quotaWindows = computed(() => props.account.quota.windows)
const now = useUiClock()
const weeklyForecast = computed(() => weeklyForecastPresentation(props.account.usage, now.value.getTime()))
const visibleQuotaWindows = computed(() => visibleSummaryQuotaWindows(quotaWindows.value))
const summaryEntries = computed(() => groupedAccountQuotaWindows(visibleQuotaWindows.value))
const hasUsage = computed(() => (props.account.usage.requestCount ?? 0) > 0)
const recentUsageEntry = computed(() => recentlyUsedQuotaEntry(
  summaryEntries.value,
  props.account.usage.models,
))
const additionalEntryCount = computed(() => Math.max(summaryEntries.value.length - 1, 0))
const usdCost = computed(() => props.account.usage.costs.find(cost => cost.currency.toUpperCase() === 'USD'))
const accountTypeLabel = computed(() => {
  if (props.account.authenticationKind === 'oauth')
    return 'OAuth'
  if (props.account.authenticationKind === 'api_key')
    return 'API Key'
  if (props.account.authenticationKind === 'setup_token')
    return 'Setup Token'
  return props.account.authenticationKind || '未知类型'
})
</script>

<template>
  <div class="box-border grid min-h-16.5 w-full min-w-0 content-center gap-1.5 py-1.5">
    <div class="flex min-h-3.5 min-w-0 items-baseline justify-between gap-2 leading-none">
      <span
        v-if="hasUsage && summaryEntries.length > 0"
        class="flex min-w-0 items-baseline gap-1 font-mono tabular-nums"
        :title="`${account.usage.windowLabelDisplay}总 Token`"
      >
        <strong class="truncate text-cp-xs font-heavy text-cp-text">
          {{ account.usage.totalTokensDisplay }}
        </strong>
        <span class="shrink-0 text-[9px] font-emphasis text-cp-text-quaternary">
          Tokens
        </span>
      </span>
      <span class="ml-auto min-w-0 truncate text-[10px] font-emphasis text-cp-text-tertiary" :title="accountTypeLabel">
        {{ accountTypeLabel }}
      </span>
    </div>
    <template v-if="summaryEntries.length > 0">
      <div v-if="recentUsageEntry" class="flex min-w-0 items-end gap-2">
        <div class="flex min-w-0 flex-1">
          <AccountQuotaSummaryEntry
            :label="recentUsageEntry.label"
            :windows="recentUsageEntry.windows"
          />
        </div>
        <span
          v-if="additionalEntryCount > 0"
          class="grid h-5 min-w-5 shrink-0 place-items-center rounded-cp bg-cp-fill-quaternary px-1.5 font-mono text-[9px] font-heavy tabular-nums text-cp-text-tertiary"
          :title="`另有 ${additionalEntryCount} 个额度组，可展开账号查看`"
        >
          +{{ additionalEntryCount }}
        </span>
      </div>
    </template>
    <AccountUsageWindow v-else variant="compact" />
    <div class="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-[10px] font-emphasis text-cp-text-tertiary">
      <span
        v-if="summaryEntries.length > 0"
        class="min-w-0 break-all text-cp-text-quaternary"
        :title="usdCost ? undefined : '暂无统计数据，不代表账号不可用'"
      >
        消费：{{ usdCost?.estimatedAmountDisplay ?? '—' }}
      </span>
      <button
        type="button"
        class="min-w-0 cursor-pointer text-left wrap-anywhere text-cp-primary-text underline decoration-dotted underline-offset-2"
        aria-haspopup="dialog"
        :title="weeklyForecast.title"
        @click.stop="emit('forecastRequested', account)"
      >
        预计周额度：{{ weeklyForecast.amount }}
      </button>
    </div>
  </div>
</template>
