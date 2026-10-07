<script setup lang="ts">
import type { UsageDisplayRecord } from '../utils/records'

import { computed } from 'vue'
import { usagePerformanceDetails } from '../utils/records'

const props = defineProps<{
  record: UsageDisplayRecord
}>()

const performance = computed(() => usagePerformanceDetails(props.record))
</script>

<template>
  <div class="grid justify-items-end gap-y-1.5 whitespace-nowrap font-mono text-cp-sm leading-none font-heavy tabular-nums">
    <span class="text-cp-text">{{ performance.throughputDisplay }}</span>
    <span class="text-cp-xs text-cp-text-secondary">
      <span title="首字时间" class="text-cp-text-quaternary">TTFT</span>
      {{ performance.firstTokenDisplay }}
    </span>
  </div>
</template>
