<script setup lang="ts">
import { computed } from 'vue'

import { stableVisualIndex } from '../utils/visualTone'

const props = withDefaults(
  defineProps<{
    planType?: string | null
    planTypeDisplay: string
    count?: number
    size?: 'xs' | 'sm' | 'md'
    wrap?: boolean
  }>(),
  {
    planType: null,
    size: 'md',
  },
)

const palettes = {
  blue: 'bg-cp-blue-container text-cp-blue-on-container',
  green: 'bg-cp-green-container text-cp-green-on-container',
  cyan: 'bg-cp-cyan-container text-cp-cyan-on-container',
  orange: 'bg-cp-orange-container text-cp-orange-on-container',
  cyanStrong: 'bg-cp-cyan-container-strong text-cp-cyan-on-container',
  greenStrong: 'bg-cp-green-container-strong text-cp-green-on-container',
}

const fallbackPalettes = [
  palettes.blue,
  palettes.green,
  palettes.cyan,
  palettes.orange,
]

// 已知套餐按展示名配色，Provider 的不同别名共享颜色
const planPalettes = new Map([
  ['api', palettes.cyanStrong],
  ['free', palettes.cyan],
  ['go', palettes.cyan],
  ['plus', 'bg-cp-blue-container-strong text-cp-blue-on-container'],
  ['prolite', 'bg-cp-purple-container text-cp-purple-on-container'],
  ['pro', 'plan-pro text-cp-purple-on-container'],
  ['promax', 'bg-cp-purple-container-strong text-cp-purple-on-container'],
  ['business', palettes.orange],
  ['business premium', 'bg-cp-orange-container-strong text-cp-orange-on-container'],
  ['enterprise', palettes.blue],
  ['enterprise (automation)', palettes.blue],
  ['edu', palettes.green],
  ['edu plus', palettes.greenStrong],
  ['edu pro', palettes.greenStrong],
  ['academic researchers', palettes.cyan],
  ['academic researchers plus', palettes.cyanStrong],
])

const displayText = computed(() => props.planTypeDisplay)

const sizes = {
  xs: { text: 'text-[10px] font-bold', singleLine: 'h-4.5 px-1.5' },
  sm: { text: 'text-cp-xs font-bold', singleLine: 'h-5 px-1.75' },
  md: { text: 'text-cp-xs font-heavy', singleLine: 'h-5.5 px-2' },
}

const paletteClass = computed(() => {
  const key = props.planType?.trim().toLowerCase() || ''
  return planPalettes.get(displayText.value?.trim().toLowerCase() || key)
    ?? fallbackPalettes[stableVisualIndex(key, fallbackPalettes.length)]
})
</script>

<template>
  <span
    v-if="wrap"
    class="line-clamp-3 max-w-full py-0.75 text-center leading-4 text-balance wrap-anywhere whitespace-normal"
    :class="sizes[size].text"
    :title="displayText || undefined"
  >
    <span class="box-decoration-clone rounded-md px-1.25 py-0.75" :class="paletteClass">
      {{ displayText }}
      <span v-if="count !== undefined" class="tabular-nums">{{ count }}</span>
    </span>
  </span>
  <span
    v-else
    class="inline-flex min-w-0 max-w-full items-center justify-center gap-1 rounded-full whitespace-nowrap leading-none shadow-cp-tertiary"
    :class="[sizes[size].text, sizes[size].singleLine, paletteClass]"
    :title="displayText || undefined"
  >
    <span class="truncate">{{ displayText }}</span>
    <span v-if="count !== undefined" class="shrink-0 tabular-nums">{{ count }}</span>
  </span>
</template>

<style scoped>
.plan-pro {
  background-color: color-mix(in srgb, var(--cp-color-purple-container), var(--cp-color-purple-container-strong));
}
</style>
