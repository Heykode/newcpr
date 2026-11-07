<script setup lang="ts">
import type { Component } from 'vue'

import { useResizeObserver } from '@vueuse/core'
import { clamp } from 'es-toolkit'
import { computed, nextTick, shallowRef, useTemplateRef, watch } from 'vue'

type SegmentedSize = 'sm' | 'md' | 'lg'

export interface SegmentedOption {
  label: string
  value: string
  icon?: Component
  disabled?: boolean
}

const props = withDefaults(
  defineProps<{
    label: string
    options: SegmentedOption[]
    disabled?: boolean
    display?: 'label' | 'icon'
    size?: SegmentedSize
  }>(),
  {
    disabled: false,
    display: 'label',
    size: 'md',
  },
)

const model = defineModel<string>({ required: true })
const rootRef = useTemplateRef<HTMLElement>('root')
const optionRefs = useTemplateRef<HTMLButtonElement[]>('option')

const activeIndex = computed(() => props.options.findIndex(option => option.value === model.value))
const optionCount = computed(() => clamp(props.options.length, 1, Number.POSITIVE_INFINITY))
const gridStyle = computed(() => ({
  gridTemplateColumns: `repeat(${optionCount.value}, minmax(max-content, 1fr))`,
}))
const indicatorStyle = shallowRef<{ width: string, transform: string }>()

function updateIndicator() {
  const option = rootRef.value?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[activeIndex.value]
  indicatorStyle.value = option
    ? { width: `${option.offsetWidth}px`, transform: `translateX(${option.offsetLeft}px)` }
    : undefined
}

watch([activeIndex, () => props.options, rootRef], updateIndicator, { flush: 'post' })
useResizeObserver(() => [rootRef.value, ...(optionRefs.value ?? [])], updateIndicator)

const rootClasses: Record<SegmentedSize, string> = {
  sm: 'h-cp-control-sm min-h-cp-control-sm',
  md: 'h-cp-control min-h-cp-control',
  lg: 'h-cp-control-lg min-h-cp-control-lg',
}
const optionClasses: Record<SegmentedSize, string> = {
  sm: 'px-2 text-cp-xs',
  md: 'px-3 text-xs',
  lg: 'px-4 text-cp',
}

function enabledIndexes() {
  return props.options.flatMap((option, index) => (option.disabled ? [] : [index]))
}

function selectOption(index: number) {
  const option = props.options[index]
  if (props.disabled || !option || option.disabled || option.value === model.value)
    return
  model.value = option.value
}

async function handleKeydown(event: KeyboardEvent, index: number) {
  const indexes = enabledIndexes()
  if (indexes.length === 0)
    return

  const current = indexes.indexOf(index)
  let nextIndex: number | undefined

  if (event.key === 'ArrowRight' || event.key === 'ArrowDown')
    nextIndex = indexes[(current + 1 + indexes.length) % indexes.length]
  else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp')
    nextIndex = indexes[(current - 1 + indexes.length) % indexes.length]
  else if (event.key === 'Home')
    nextIndex = indexes[0]
  else if (event.key === 'End')
    nextIndex = indexes[indexes.length - 1]
  else return

  if (nextIndex === undefined)
    return

  event.preventDefault()
  const buttons = (event.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLButtonElement>('button')
  selectOption(nextIndex)
  await nextTick()
  buttons?.[nextIndex]?.focus()
}
</script>

<template>
  <div
    ref="root"
    class="relative inline-grid min-w-0 max-w-full items-stretch overflow-x-auto rounded-cp bg-cp-fill-tertiary p-0.75 scrollbar-none [&::-webkit-scrollbar]:hidden [&::-webkit-scrollbar]:size-0"
    :class="rootClasses[size]"
    :style="gridStyle"
    role="radiogroup"
    :aria-label="label"
    :aria-disabled="disabled || undefined"
  >
    <span
      v-if="indicatorStyle && activeIndex >= 0"
      class="pointer-events-none absolute inset-y-0.75 left-0 rounded-cp bg-cp-bg-container shadow-cp-tertiary transition-[transform,width] duration-200 ease-out motion-reduce:transition-none"
      :style="indicatorStyle"
    />
    <button
      v-for="(option, index) in options"
      :key="option.value"
      ref="option"
      class="relative z-10 inline-flex touch-manipulation items-center justify-center gap-1.5 rounded-cp border-0 bg-transparent leading-none font-emphasis outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-cp-control-outline motion-reduce:transition-none"
      :class="[
        optionClasses[size],
        model === option.value ? 'text-cp-text' : 'text-cp-text-secondary hover:text-cp-text',
        disabled || option.disabled ? 'cursor-not-allowed opacity-60 hover:text-cp-text-secondary' : undefined,
        display === 'icon' ? 'px-0' : undefined,
      ]"
      type="button"
      role="radio"
      :aria-checked="model === option.value"
      :aria-label="display === 'icon' ? option.label : undefined"
      :tabindex="model === option.value || (activeIndex < 0 && index === 0) ? 0 : -1"
      :disabled="disabled || option.disabled"
      @click="selectOption(index)"
      @keydown="handleKeydown($event, index)"
    >
      <component :is="option.icon" v-if="option.icon" class="size-3.5 shrink-0" />
      <span v-if="display === 'label'" class="whitespace-nowrap">{{ option.label }}</span>
    </button>
  </div>
</template>
