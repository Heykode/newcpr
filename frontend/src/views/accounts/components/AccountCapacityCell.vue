<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  inFlight?: number | null
  effectiveConcurrencyLimit?: number | null
  concurrencyLimit?: number | null
}>()

const limit = computed(() => props.effectiveConcurrencyLimit ?? props.concurrencyLimit)
const usedClass = computed(() => {
  const used = props.inFlight
  if (used === null || used === undefined || used <= 0)
    return 'text-cp-text-secondary'
  if (limit.value !== null && limit.value !== undefined) {
    if (used >= limit.value)
      return 'text-cp-error-text'
    if (used >= limit.value * 0.75)
      return 'text-cp-warning-text'
  }
  return 'text-cp-success-text'
})
</script>

<template>
  <div
    data-account-capacity
    class="inline-flex min-w-0 max-w-full flex-wrap items-baseline justify-center gap-x-1 rounded-cp-sm bg-cp-fill-quaternary px-1.5 py-1 font-mono text-cp-sm font-normal tabular-nums whitespace-normal text-cp-text-secondary"
    role="group"
    aria-label="账号并发容量"
  >
    <span
      data-capacity-used
      class="min-w-0 max-w-full font-emphasis wrap-anywhere"
      :class="usedClass"
      role="img"
      :aria-label="`实时并发 ${inFlight ?? '未知'}`"
    >
      {{ inFlight ?? '\u2014' }}
    </span>
    <span data-capacity-separator class="shrink-0 font-normal text-cp-text-secondary" aria-hidden="true">/</span>
    <span
      data-capacity-limit
      class="min-w-0 max-w-full font-normal wrap-anywhere text-cp-text-secondary"
      role="img"
      :aria-label="`并发上限 ${limit ?? '未知'}`"
    >
      {{ limit ?? '\u2014' }}
    </span>
  </div>
</template>
