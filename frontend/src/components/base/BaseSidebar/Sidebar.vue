<script setup lang="ts">
import type { SidebarItem } from './types'
import { useTemplateRef } from 'vue'
import SidebarNav from './components/SidebarNav.vue'

withDefaults(defineProps<{
  items: SidebarItem[]
  defaultActive?: string
  defaultOpeneds?: string[]
  collapse?: boolean
  width?: string
  collapseWidth?: string
  navigationLabel?: string
}>(), {
  defaultActive: '',
  defaultOpeneds: () => [],
  collapse: false,
  width: '251px',
  collapseWidth: '88px',
})

const emit = defineEmits<{
  select: [index: string, indexPath: string[]]
  open: [index: string, indexPath: string[]]
  close: [index: string, indexPath: string[]]
}>()
defineSlots<{
  'brand-icon'?: (props: { collapse: boolean }) => unknown
  'brand'?: (props: { collapse: boolean }) => unknown
  'footer'?: (props: { collapse: boolean }) => unknown
}>()

const navigation = useTemplateRef<InstanceType<typeof SidebarNav>>('navigation')

defineExpose({
  open: (index: string) => navigation.value?.open(index),
  close: (index: string) => navigation.value?.close(index),
  updateActiveIndex: (index: string) => navigation.value?.updateActiveIndex(index),
})
</script>

<template>
  <aside
    class="relative flex h-full min-h-0 shrink-0 flex-col overflow-hidden rounded-e-(--cp-layout-sider-border-radius) bg-(--cp-layout-sider-bg) bg-(image:--cp-layout-sider-bg-image) shadow-cp-layout-sider transition-[width,flex-basis] duration-340 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none"
    :style="{ width: collapse ? collapseWidth : width, flexBasis: collapse ? collapseWidth : width }"
  >
    <div
      v-if="$slots['brand-icon'] || $slots.brand"
      class="mx-4 mt-6 grid h-12 shrink-0 items-center"
      :class="[
        $slots['brand-icon'] ? 'grid-cols-[44px_minmax(0,1fr)]' : 'grid-cols-1',
        collapse ? 'w-11 self-center' : 'self-stretch gap-3',
      ]"
    >
      <slot name="brand-icon" :collapse="collapse" />
      <span
        v-if="$slots.brand"
        class="grid min-w-33 content-center overflow-hidden whitespace-nowrap transition-[opacity,translate] duration-200 motion-reduce:transition-none"
        :class="collapse ? 'pointer-events-none -translate-x-1.5 opacity-0' : 'translate-x-0 opacity-100'"
        :inert="collapse"
        :aria-hidden="collapse || undefined"
      >
        <slot name="brand" :collapse="collapse" />
      </span>
    </div>

    <SidebarNav
      ref="navigation"
      :items="items"
      :default-active="defaultActive"
      :default-openeds="defaultOpeneds"
      :collapse="collapse"
      :navigation-label="navigationLabel"
      @select="(index, path) => emit('select', index, path)"
      @open="(index, path) => emit('open', index, path)"
      @close="(index, path) => emit('close', index, path)"
    />

    <div v-if="$slots.footer" class="mx-4 mb-6 shrink-0" :class="collapse ? 'min-w-11 self-center' : 'self-stretch'">
      <slot name="footer" :collapse="collapse" />
    </div>
  </aside>
</template>
