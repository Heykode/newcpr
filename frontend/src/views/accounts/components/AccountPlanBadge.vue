<script setup lang="ts">
import { computed } from 'vue'

import { stableVisualIndex } from '../utils/visualTone'

const props = withDefaults(
  defineProps<{
    planType?: string | null
    planTypeDisplay: string
    size?: 'xs' | 'sm' | 'md'
    wrap?: boolean
  }>(),
  {
    planType: null,
    size: 'md',
  },
)

const planPalettes: Record<string, string> = {
  free: 'bg-cp-cyan-container text-cp-cyan-on-container',
  pro: 'bg-cp-purple-container-strong text-cp-purple-on-container',
  prolite: 'bg-cp-purple-container text-cp-purple-on-container',
}

const fallbackPalettes = [
  'bg-cp-blue-container text-cp-blue-on-container',
  'bg-cp-green-container text-cp-green-on-container',
  'bg-cp-cyan-container text-cp-cyan-on-container',
  'bg-cp-orange-container text-cp-orange-on-container',
] as const

const rawPlanType = computed(() => props.planType?.trim() || '')

const sizes = {
  xs: { text: 'text-[10px] font-bold', singleLine: 'h-4.5 px-1.5' },
  sm: { text: 'text-cp-xs font-bold', singleLine: 'h-5 px-1.75' },
  md: { text: 'text-cp-xs font-heavy', singleLine: 'h-5.5 px-2' },
}

const paletteClass = computed(() => {
  const key = rawPlanType.value.toLowerCase()
  const planPalette = planPalettes[key]
  if (planPalette)
    return planPalette

  return fallbackPalettes[stableVisualIndex(key, fallbackPalettes.length)]
})
</script>

<template>
  <span
    class="inline-flex min-w-0 max-w-full items-center justify-center shadow-cp-tertiary"
    :class="[sizes[size].text, paletteClass, wrap ? 'min-h-5.5 rounded-lg px-1 py-0.75 text-center leading-3.5' : [sizes[size].singleLine, 'rounded-full whitespace-nowrap leading-none']]"
    :title="planTypeDisplay || undefined"
  >
    <span :class="wrap ? 'line-clamp-3 text-balance wrap-anywhere whitespace-normal' : 'truncate'">{{ planTypeDisplay }}</span>
  </span>
</template>
