<script setup lang="ts">
import type { AccountGroupRef } from '@/api'
import { FastColor } from '@ant-design/fast-color'
import { useResizeObserver } from '@vueuse/core'
import { computed, ref, shallowRef, watch } from 'vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { useThemeColor } from '@/composables/useThemeColor'
import { accountGroupLayout } from './account-group-layout'

const props = withDefaults(defineProps<{
  groups: AccountGroupRef[]
  layout?: 'inline' | 'stacked'
}>(), { layout: 'inline' })

const fullNames = computed(() =>
  props.groups.map(group => (group.enabled ? group.name : `${group.name}（已禁用）`)).join('、'),
)
const themeColor = useThemeColor()
const styledGroups = computed(() => {
  const surface = themeColor('--cp-color-bg-container', '#ffffff')
  return props.groups.map((group) => {
    const background = new FastColor(group.color).onBackground(surface)
    const luminance = background.getLuminance()
    return {
      ...group,
      style: {
        backgroundColor: background.toHexString(),
        color: (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) ? '#000000' : '#ffffff',
      },
    }
  })
})
const root = ref<HTMLElement | null>(null)
const measurements = ref<HTMLElement | null>(null)
const layoutSize = shallowRef<{ count: number, maxWidth: number | undefined }>({ count: 2, maxWidth: undefined })

function measureGroups() {
  if (props.layout !== 'stacked' || !root.value || !measurements.value)
    return
  const widths = Array.from(measurements.value.querySelectorAll<HTMLElement>('[data-group-measure]'), element => element.getBoundingClientRect().width)
  const overflowWidth = measurements.value.querySelector<HTMLElement>('[data-overflow-measure]')?.getBoundingClientRect().width
  if (!overflowWidth || widths.length !== props.groups.length)
    return
  const next = accountGroupLayout(widths, root.value.clientWidth, overflowWidth)
  if (next.count !== layoutSize.value.count || next.maxWidth !== layoutSize.value.maxWidth)
    layoutSize.value = next
}

useResizeObserver([root, measurements], measureGroups)
watch(() => [props.layout, props.groups], measureGroups, { deep: true, flush: 'post' })

const visibleGroups = computed(() => styledGroups.value.slice(0, props.layout === 'stacked' ? layoutSize.value.count : 2))
const hiddenCount = computed(() => Math.max(props.groups.length - visibleGroups.value.length, 0))
</script>

<template>
  <div
    v-if="groups.length > 0"
    ref="root"
    class="relative min-w-0 max-w-full"
    :class="layout === 'stacked' ? 'w-full' : undefined"
    :aria-label="fullNames"
  >
    <BasePopover
      class="min-w-0 max-w-full"
      :class="layout === 'stacked' ? 'w-full' : undefined"
      placement="top"
      trigger="hover-click"
      :hover-delay="100"
      arrow-surface-class="bg-cp-bg-elevated"
    >
      <template #trigger="{ open }">
        <button
          type="button"
          class="min-w-0 max-w-full cursor-help items-center overflow-hidden border-0 bg-transparent p-0 text-left"
          :class="layout === 'stacked' ? 'flex w-full flex-wrap justify-center gap-x-1.5 gap-y-1' : 'flex flex-nowrap gap-1.5'"
          :aria-label="`查看账号分组：${fullNames}`"
          :aria-expanded="open"
        >
          <span
            v-for="group in visibleGroups"
            :key="group.id"
            class="inline-flex min-w-0 items-center overflow-hidden px-2 py-1 leading-none font-heavy"
            :class="layout === 'stacked' ? 'max-w-full shrink-0 rounded-cp-sm text-[13px]' : 'max-w-28 shrink rounded-full text-[10px]'"
            :style="[group.style, layout === 'stacked' && layoutSize.maxWidth !== undefined ? { maxWidth: `${layoutSize.maxWidth}px` } : undefined]"
          >
            <span class="truncate">{{ group.name }}</span>
          </span>
          <span
            v-if="hiddenCount > 0"
            class="inline-flex h-5 min-w-5 shrink-0 items-center justify-center bg-cp-fill-tertiary px-1.5 font-mono text-cp-text-secondary"
            :class="layout === 'stacked' ? 'rounded-cp-sm text-[13px]' : 'rounded-full text-[10px]'"
          >
            +{{ hiddenCount }}
          </span>
        </button>
      </template>
      <div
        class="max-h-64 max-w-[min(360px,calc(100vw-16px))] overflow-y-auto p-2"
        role="list"
        :aria-label="`全部账号分组：${fullNames}`"
      >
        <div class="flex flex-wrap gap-1.5">
          <span
            v-for="group in styledGroups"
            :key="group.id"
            class="inline-flex min-w-0 max-w-full items-center rounded-cp px-2 py-1 text-xs leading-relaxed font-heavy"
            :style="group.style"
            role="listitem"
          >
            <span class="break-all whitespace-normal">
              {{ group.name }}<template v-if="!group.enabled">（已禁用）</template>
            </span>
          </span>
        </div>
      </div>
    </BasePopover>
    <div v-if="layout === 'stacked'" class="pointer-events-none invisible absolute inset-0 overflow-hidden" aria-hidden="true">
      <div ref="measurements" class="flex w-max gap-1.5">
        <span
          v-for="group in groups"
          :key="group.id"
          data-group-measure
          class="inline-flex shrink-0 px-2 py-1 text-[13px] leading-none font-heavy whitespace-nowrap"
        >
          {{ group.name }}
        </span>
        <span data-overflow-measure class="inline-flex h-5 min-w-5 shrink-0 items-center px-1.5 font-mono text-[13px]">+{{ groups.length }}</span>
      </div>
    </div>
  </div>
  <span v-else class="text-xs text-cp-text-quaternary">—</span>
</template>
