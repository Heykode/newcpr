<script setup lang="ts">
import type { AccountQuotaWindow } from '../../constants'
import type { CodexQuotaOverdraftState } from '@/api/modules/accounts'

import { computed } from 'vue'
import { useUiClock } from '@/composables/useUiClock'
import { t } from '@/i18n'
import { codexQuotaOverdraftWindow } from '../AccountQuotaOverdraft/presenter'
import AccountQuotaOverdraftWindowLine from '../AccountQuotaOverdraft/WindowLine.vue'
import { quotaWindowCode, quotaWindowPresentation, quotaWindowResetPresentation } from '../AccountUsageWindow/presenter'

const props = defineProps<{
  label: string
  windows: AccountQuotaWindow[]
  comfortable?: boolean
  overdraft?: CodexQuotaOverdraftState | null
}>()

const now = useUiClock()
const showLabel = computed(() => props.label !== t('accounts.quota.general'))
const windowItems = computed(() => props.windows.map(window => ({
  key: window.key,
  code: quotaWindowCode(window.windowSeconds, window.role),
  labelTooltip: window.labelDisplay,
  usedPercent: window.usedPercent,
  usedPercentDisplay: window.usedPercentDisplay,
  presentation: quotaWindowPresentation(window, '2px'),
  reset: quotaWindowResetPresentation(window, now.value.getTime()),
  resetAt: window.resetAt,
  overdraft: codexQuotaOverdraftWindow(props.overdraft, window.key),
})))
</script>

<template>
  <div class="grid min-w-0 gap-1.5">
    <span v-if="showLabel" class="min-w-0 truncate leading-3 font-bold text-cp-text-quaternary" :class="comfortable ? 'text-xs' : 'text-[10px]'" :title="label">
      {{ label }}
    </span>

    <div class="grid min-w-0 gap-1.5">
      <div
        v-for="item in windowItems"
        :key="item.key"
        class="grid min-w-0 items-center gap-x-1.5 leading-4"
        :class="comfortable ? 'min-h-5 grid-cols-[28px_minmax(0,1fr)_44px_64px] text-xs' : 'min-h-4 grid-cols-[24px_minmax(0,1fr)_40px_64px] text-[10px]'"
        role="group"
        :aria-label="`${item.labelTooltip}额度周期`"
      >
        <AccountQuotaOverdraftWindowLine
          v-if="item.overdraft?.active"
          class="col-span-full"
          :window="item.overdraft"
        />
        <span
          class="min-w-0 rounded-sm bg-cp-fill-quaternary px-0.5 text-center font-mono font-bold break-all text-cp-text-secondary"
          :title="item.labelTooltip"
        >
          {{ item.code }}
        </span>
        <div
          class="h-1.5 w-full overflow-hidden rounded-full bg-cp-border-secondary"
          role="progressbar"
          :aria-label="item.labelTooltip"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-valuenow="item.usedPercent ?? undefined"
          :aria-valuetext="item.usedPercentDisplay"
        >
          <div
            class="h-full rounded-full transition-[width,background-color] duration-200 motion-reduce:transition-none"
            :class="item.presentation.barClass"
            :style="item.presentation.barStyle"
          />
        </div>
        <strong
          class="min-w-0 text-right font-mono font-heavy break-all tabular-nums"
          :class="item.presentation.percentTextClass"
          :aria-label="`${item.labelTooltip}已使用${item.usedPercentDisplay}`"
          :title="`${item.labelTooltip}已使用：${item.usedPercentDisplay}`"
        >
          {{ item.usedPercentDisplay }}
        </strong>
        <time
          v-if="item.reset"
          class="min-w-0 text-right font-mono break-words tabular-nums"
          :class="item.reset.pending ? 'text-cp-warning-text' : 'text-cp-text-tertiary'"
          :datetime="item.resetAt ?? undefined"
          :title="item.reset.title"
          :aria-label="`${item.labelTooltip}：${item.reset.display}${item.reset.pending ? '' : '后重置'}`"
        >
          {{ item.reset.display }}
        </time>
        <span v-else class="text-right text-cp-text-tertiary" title="未提供重置时间">—</span>
      </div>
    </div>
  </div>
</template>
