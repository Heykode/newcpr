<script setup lang="ts">
import type { SidebarItem } from '../types'
import { usePreferredReducedMotion, useTimeoutFn } from '@vueuse/core'
import { gsap } from 'gsap'
import { computed, onBeforeUnmount, onMounted, shallowRef, useId, useTemplateRef, watch } from 'vue'
import Scrollbar from '../../BaseScrollbar.vue'
import SidebarEntry from './SidebarEntry.vue'

const props = defineProps<{
  items: SidebarItem[]
  defaultActive: string
  defaultOpeneds: string[]
  collapse: boolean
  navigationLabel?: string
}>()
const emit = defineEmits<{
  select: [index: string, indexPath: string[]]
  open: [index: string, indexPath: string[]]
  close: [index: string, indexPath: string[]]
}>()
const activeIndex = shallowRef(props.defaultActive)
const openedMenus = shallowRef([...props.defaultOpeneds])
const submenuId = useId()
const navigation = useTemplateRef<HTMLElement>('navigation')
const preferredMotion = usePreferredReducedMotion()
let labelMotion: gsap.core.Tween | undefined
const feedbackMuted = shallowRef(false)
const { start: restoreFeedback } = useTimeoutFn(() => {
  feedbackMuted.value = false
}, 300, { immediate: false })

function animateLabels(collapsed: boolean) {
  labelMotion?.kill()
  labelMotion = undefined
  const labels = navigation.value?.querySelectorAll<HTMLElement>('[data-sidebar-label]')
  if (!labels?.length)
    return

  const state = { opacity: collapsed ? 0 : 1, x: collapsed ? -6 : 0 }
  if (preferredMotion.value === 'reduce') {
    gsap.set(labels, state)
    return
  }

  // 逐帧更新目标值并保留文字的 CSS 过渡，形成错开显现的柔和拖尾
  labelMotion = gsap.to(labels, {
    ...state,
    duration: collapsed ? 0.16 : 0.2,
    ease: collapsed ? 'power2.in' : 'power3.out',
    stagger: collapsed ? 0 : 0.018,
    overwrite: true,
    onComplete: () => { labelMotion = undefined },
  })
}

onMounted(() => animateLabels(props.collapse))
watch([() => props.collapse, preferredMotion], ([collapsed]) => animateLabels(collapsed))
onBeforeUnmount(() => labelMotion?.kill())

function isExpanded(item: SidebarItem) {
  return !props.collapse && openedMenus.value.includes(item.index)
}

function isActive(item: SidebarItem) {
  return item.index === activeIndex.value || Boolean(item.children?.some(child => child.index === activeIndex.value))
}

watch(() => props.defaultActive, (index) => {
  activeIndex.value = index
})
watch(() => [...props.defaultOpeneds], (indexes, previous) => {
  // 等值数组不重置用户展开状态，仍响应父组件对数组的就地修改。
  if (indexes.length !== previous.length || indexes.some((index, position) => index !== previous[position]))
    openedMenus.value = indexes
})
watch([() => props.items, activeIndex, () => props.collapse], () => {
  if (props.collapse)
    return
  const parent = props.items.find(item => item.children?.some(child => child.index === activeIndex.value))
  // 外部选中项变化时展开其路径，不伪造用户的 open 事件。
  if (parent && !openedMenus.value.includes(parent.index))
    openedMenus.value = [...openedMenus.value, parent.index]
}, { immediate: true })

const indicatorStyle = computed(() => {
  const index = props.items.findIndex(isActive)
  const item = props.items[index]
  if (!item)
    return undefined
  const childIndex = item.children?.findIndex(child => child.index === activeIndex.value) ?? -1
  const nested = isExpanded(item) && childIndex >= 0
  // 按菜单布局的 spacing 单位定位，随主题密度同步缩放
  const expandedHeight = props.items.slice(0, index).reduce((height, entry) =>
    height + (isExpanded(entry) ? (entry.children?.length ?? 0) * 11 : 0), 0)
  const top = index * 14.5 + expandedHeight + (nested ? 12.5 + childIndex * 11 : 0)
  return {
    transform: `translate3d(${nested ? 'calc(var(--spacing) * 6)' : '0px'}, calc(var(--spacing) * ${top}), 0)`,
    width: nested ? 'calc(100% - var(--spacing) * 6)' : '100%',
    height: `calc(var(--spacing) * ${nested ? 10 : 11.5})`,
  }
})

function indexPath(index: string) {
  const parent = props.items.find(item => item.children?.some(child => child.index === index))
  return parent ? [parent.index, index] : [index]
}

function open(index: string) {
  if (openedMenus.value.includes(index) || !props.items.some(item => item.index === index && item.children?.length))
    return
  openedMenus.value = [...openedMenus.value, index]
  emit('open', index, [index])
}

function close(index: string) {
  if (!openedMenus.value.includes(index))
    return
  openedMenus.value = openedMenus.value.filter(opened => opened !== index)
  emit('close', index, [index])
}

function select(index: string) {
  activeIndex.value = index
  feedbackMuted.value = true
  restoreFeedback()
  emit('select', index, indexPath(index))
}

function activate(item: SidebarItem) {
  if (item.children?.length && !props.collapse) {
    if (isExpanded(item))
      close(item.index)
    else
      open(item.index)
    return
  }
  select(item.index)
}

function updateActiveIndex(index: string) {
  activeIndex.value = index
}

defineExpose({ open, close, updateActiveIndex })
</script>

<template>
  <Scrollbar class="my-6 w-full flex-1">
    <div class="px-4">
      <!-- 焦点环位于按钮内侧，避免被滚动区和折叠边界裁切 -->
      <nav ref="navigation" class="relative grid gap-3" :class="collapse ? 'mx-auto w-11.5' : 'w-full'" :aria-label="navigationLabel">
        <span
          v-if="indicatorStyle"
          class="pointer-events-none absolute left-0 top-0 overflow-hidden rounded-cp bg-cp-menu-item-selected-bg bg-(image:--cp-menu-item-selected-bg-image) shadow-cp-menu-item-selected transition-[transform,width,height] duration-260 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none"
          :style="indicatorStyle"
          aria-hidden="true"
        >
          <span :key="activeIndex" class="sidebar-signal absolute inset-y-0 left-0 w-2/3" />
        </span>
        <div v-for="(item, index) in items" :key="item.index" class="grid min-w-0">
          <SidebarEntry
            :item="item"
            :active="isActive(item)"
            :collapsed="collapse"
            :expandable="Boolean(item.children?.length)"
            :expanded="isExpanded(item)"
            :controls="`${submenuId}-${index}`"
            :feedback-muted="feedbackMuted"
            @activate="activate(item)"
          />
          <div
            v-if="item.children?.length"
            :id="`${submenuId}-${index}`"
            class="grid transition-[grid-template-rows] duration-260 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none"
            :class="isExpanded(item) ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'"
            :inert="!isExpanded(item)"
            :aria-hidden="!isExpanded(item) || undefined"
            role="group"
            :aria-label="item.label"
          >
            <div class="min-h-0 overflow-hidden">
              <div class="ml-3 grid gap-1 pt-1 pl-3">
                <SidebarEntry
                  v-for="child in item.children"
                  :key="child.index"
                  :item="child"
                  :disabled="item.disabled"
                  :active="child.index === activeIndex"
                  :collapsed="false"
                  nested
                  :feedback-muted="feedbackMuted"
                  @activate="select(child.index)"
                />
              </div>
            </div>
          </div>
        </div>
      </nav>
    </div>
  </Scrollbar>
</template>

<style scoped>
.sidebar-signal {
  background: linear-gradient(
    90deg,
    transparent,
    color-mix(in srgb, var(--cp-color-info) 9%, transparent),
    transparent
  );
  animation: sidebar-signal 520ms cubic-bezier(0.22, 1, 0.36, 1) both;
}

@keyframes sidebar-signal {
  from {
    opacity: 0.48;
    transform: translateX(-70%);
  }
  to {
    opacity: 0;
    transform: translateX(120%);
  }
}

@media (prefers-reduced-motion: reduce) {
  .sidebar-signal {
    animation: none;
    opacity: 0;
  }
}
</style>
