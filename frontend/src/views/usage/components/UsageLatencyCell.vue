<script setup lang="ts">
import type { UsageDisplayRecord } from '../utils/records'
import { computed } from 'vue'

import { t } from '@/i18n'
import { usageLatencyDetails } from '../utils/records'
import UsageDetailPopover from './UsageDetailPopover.vue'

const props = defineProps<{
  record: UsageDisplayRecord
}>()

const latencyDetails = computed(() => usageLatencyDetails(props.record))
const latencyTones = {
  good: 'text-cp-success-text',
  warn: 'text-cp-warning-text',
  slow: 'latency-slow',
  critical: 'text-cp-error-text',
}
</script>

<template>
  <div class="flex items-center justify-end gap-1.5">
    <div
      class="grid grid-cols-[auto_auto] items-center justify-end gap-x-2 gap-y-1.5 whitespace-nowrap font-mono text-cp-sm leading-none font-heavy tabular-nums"
    >
      <span class="text-cp-xs text-cp-text-quaternary">{{ latencyDetails.firstOutputLabel }}</span>
      <span :class="latencyDetails.firstOutputSeverity === null ? 'text-cp-text-quaternary' : latencyTones[latencyDetails.firstOutputSeverity]">{{ latencyDetails.firstOutputDisplay }}</span>
      <span class="text-cp-xs text-cp-text-quaternary">{{ t('usage.latency.totalLatency') }}</span>
      <span :class="latencyDetails.totalSeverity === null ? 'text-cp-text-quaternary' : latencyTones[latencyDetails.totalSeverity]">{{ latencyDetails.totalDisplay }}</span>
    </div>

    <UsageDetailPopover title="延迟明细" trigger-label="查看延迟明细">
      <div
        v-if="latencyDetails.breakdownItems.length"
        class="grid gap-1.5 text-cp-text-secondary"
      >
        <div
          v-for="item in latencyDetails.breakdownItems"
          :key="item.label"
          class="flex justify-between gap-4"
        >
          <span class="whitespace-nowrap">{{ item.label }}</span>
          <span class="whitespace-nowrap font-mono font-heavy text-cp-text">
            {{ item.value }}
          </span>
        </div>
      </div>
      <p v-else class="m-0 text-cp-text-quaternary">
        此记录未采集完整的阶段耗时。
      </p>

      <div class="flex justify-between border-t border-cp-split pt-2">
        <span class="whitespace-nowrap text-cp-text-secondary">{{ t('usage.latency.totalLatency') }}</span>
        <span class="whitespace-nowrap font-mono font-heavy text-cp-info-text">
          {{ latencyDetails.totalDisplay }}
        </span>
      </div>

      <div
        v-if="latencyDetails.transportItems.length"
        class="grid gap-1.5 border-t border-cp-split pt-2 text-cp-text-secondary"
      >
        <p class="m-0 font-heavy text-cp-text">
          {{ t('usage.status.transport') }}
        </p>
        <div
          v-for="item in latencyDetails.transportItems"
          :key="item.label"
          class="flex justify-between gap-4"
        >
          <span class="whitespace-nowrap">{{ item.label }}</span>
          <span class="whitespace-nowrap font-mono font-heavy text-cp-text">
            {{ item.value }}
          </span>
        </div>
        <p class="m-0 text-cp-xs leading-snug text-cp-text-quaternary">
          与阶段耗时可能重叠，不参与总耗时相加。
        </p>
      </div>
    </UsageDetailPopover>
  </div>
</template>

<style scoped>
.latency-slow {
  color: color-mix(in oklab, var(--cp-color-warning-text) 65%, var(--cp-color-error-text));
}
</style>
