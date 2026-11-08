<script setup lang="ts">
import type { SidebarItem } from '@/components/base/BaseSidebar/types'
import {
  ArrowUpCircle,
  ChartNoAxesColumn,
  ClipboardCheck,
  FolderTree,
  Info,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Moon,
  Network,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  RefreshCw,
  Settings,
  Sun,
  Users,
} from '@lucide/vue'
import { storeToRefs } from 'pinia'
import { computed, useTemplateRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { isNavigationFailure, NavigationFailureType, useRoute, useRouter } from 'vue-router'
import AppBrandMark from '@/components/AppBrandMark.vue'
import AppLanguageMenu from '@/components/AppLanguageMenu.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseMotionIcon from '@/components/base/BaseMotionIcon.vue'
import BaseSidebar from '@/components/base/BaseSidebar/Sidebar.vue'
import { useAuthStore } from '@/stores/modules/auth'
import { useSystemUpdateStore } from '@/stores/modules/system-update'
import { useThemeStore } from '@/stores/modules/theme'

const props = withDefaults(defineProps<{
  collapsed?: boolean
  mobile?: boolean
}>(), {
  collapsed: false,
  mobile: false,
})
const emit = defineEmits<{
  close: []
  navigate: []
  openSystemUpdate: []
  openAbout: []
  toggle: []
}>()

const { t } = useI18n({ useScope: 'global' })
const sidebar = useTemplateRef<InstanceType<typeof BaseSidebar>>('sidebar')
const route = useRoute()
const router = useRouter()
const authStore = useAuthStore()
const systemUpdateStore = useSystemUpdateStore()
const themeStore = useThemeStore()
const { version, hasUpdate } = storeToRefs(systemUpdateStore)
const { effectiveTheme } = storeToRefs(themeStore)
const { toggleTheme } = themeStore
const isCollapsed = computed(() => !props.mobile && props.collapsed)

const navItems = computed<SidebarItem[]>(() => [
  { label: t('navigation.overview'), icon: LayoutDashboard, index: '/' },
  { label: t('navigation.accounts'), icon: Users, index: '/accounts' },
  { label: t('navigation.relogin'), icon: RefreshCw, index: '/relogin' },
  { label: t('navigation.proxies'), icon: Network, index: '/proxies' },
  { label: t('navigation.ipv6'), icon: Network, index: '/ipv6-egress' },
  { label: t('navigation.groups'), icon: FolderTree, index: '/account-groups' },
  { label: t('navigation.apiKeys'), icon: KeyRound, index: '/api-keys' },
  { label: t('navigation.usage'), icon: ChartNoAxesColumn, index: '/usage' },
  { label: t('navigation.quality'), icon: ClipboardCheck, index: '/quality-ops' },
  { label: t('navigation.appearance'), icon: Palette, index: '/theme' },
  { label: t('navigation.settings'), icon: Settings, index: '/settings' },
])

const activeIndex = computed(() => navItems.value.find(item => item.index !== '/' && route.path.startsWith(item.index))?.index ?? '/')

async function handleSelect(index: string) {
  // 路由守卫可能取消跳转，菜单保持当前页，直到实际路由改变
  sidebar.value?.updateActiveIndex(activeIndex.value)
  // 先移除手机侧栏的遮罩和焦点锁，让离页确认框可操作。
  if (props.mobile)
    emit('close')
  const failure = await router.push(index)
  if (!failure || isNavigationFailure(failure, NavigationFailureType.duplicated))
    emit('navigate')
}

function openSystemUpdate() {
  emit('openSystemUpdate')
}

async function handleLogout() {
  await authStore.logout()
  await router.push('/login')
  emit('navigate')
}

const themeToggleLabel = computed(() => effectiveTheme.value === 'dark'
  ? t('appearance.switchToLight')
  : t('appearance.switchToDark'))
const versionText = computed(() => version.value?.version.trim() ?? '')
const hasVersionLabel = computed(() => versionText.value.length > 0)
const versionLabel = computed(() => `v${versionText.value}`)
const updateButtonLabel = computed(() => hasUpdate.value
  ? t('navigation.update.available')
  : t('navigation.update.open'))
</script>

<template>
  <BaseSidebar
    ref="sidebar"
    class="z-20"
    :class="mobile ? 'flex' : 'hidden min-[961px]:flex'"
    :items="navItems"
    :default-active="activeIndex"
    :collapse="isCollapsed"
    :navigation-label="t('navigation.mainLabel')"
    @select="handleSelect"
  >
    <template #brand-icon>
      <BaseMotionIcon
        variant="brand"
        class="inline-flex size-11 items-center justify-center relative -top-0.5 rounded-cp"
      >
        <AppBrandMark class="block size-11 select-none" />
      </BaseMotionIcon>
    </template>
    <template #brand>
      <strong class="text-base leading-[1.1] font-heavy text-cp-text"> Codex Proxy </strong>
      <span class="mt-1.5 flex h-4.5 min-w-0 items-center gap-2">
        <span class="shrink-0 text-xs leading-none font-emphasis text-cp-text-secondary"> Rust build </span>
        <button
          v-if="hasVersionLabel"
          type="button"
          class="inline-flex h-4.5 min-w-0 cursor-pointer items-center gap-1 rounded-cp-sm border-0 px-1.5 font-mono text-[10px] leading-none font-bold transition-colors outline-none focus-visible:ring-2 focus-visible:ring-cp-control-outline focus-visible:ring-offset-2 focus-visible:ring-offset-cp-bg-container"
          :class="[
            hasUpdate
              ? 'bg-cp-success-container text-cp-success-on-container hover:bg-cp-success-container-hover'
              : 'bg-cp-fill-quaternary text-cp-text-quaternary hover:bg-cp-fill-tertiary hover:text-cp-text-secondary',
          ]"
          :aria-label="updateButtonLabel"
          @click="openSystemUpdate"
        >
          <span>{{ versionLabel }}</span>
          <ArrowUpCircle v-if="hasUpdate" class="size-3 shrink-0 text-cp-success" />
        </button>
      </span>
    </template>
    <template #footer>
      <div
        class="bg-cp-fill-quaternary"
        :class="isCollapsed ? 'grid gap-1 rounded-cp p-1' : 'flex min-h-11 flex-wrap items-center justify-between gap-1 rounded-cp-lg px-2'"
      >
        <span
          v-if="!isCollapsed"
          class="inline-flex h-7 items-center justify-self-start gap-1.5 whitespace-nowrap rounded-lg bg-cp-success-container px-2 text-xs leading-none font-emphasis text-cp-success-on-container"
        >
          <i class="size-1.5 rounded-full bg-cp-success" />
          {{ t('navigation.online') }}
        </span>

        <div :class="isCollapsed ? 'grid justify-items-center gap-1' : 'flex shrink-0 items-center gap-1 [&_button]:max-w-8'">
          <BaseIconButton
            v-if="isCollapsed && hasUpdate"
            variant="success"
            size="md"
            :label="updateButtonLabel"
            @click="openSystemUpdate"
          >
            <ArrowUpCircle :size="19" />
          </BaseIconButton>

          <BaseIconButton
            :size="isCollapsed ? 'md' : 'sm'"
            :label="t('auth.signOut')"
            variant="destructive"
            :disabled="authStore.loading || !authStore.isAuthenticated"
            @click="handleLogout"
          >
            <LogOut :size="isCollapsed ? 19 : 18" />
          </BaseIconButton>

          <BaseIconButton
            :size="isCollapsed ? 'md' : 'sm'"
            :label="themeToggleLabel"
            @click="toggleTheme($event)"
          >
            <Sun v-if="effectiveTheme === 'dark'" :size="isCollapsed ? 19 : 18" />
            <Moon v-else :size="isCollapsed ? 19 : 18" />
          </BaseIconButton>

          <AppLanguageMenu />
          <BaseIconButton :size="isCollapsed ? 'md' : 'sm'" label="关于" @click="emit('openAbout')">
            <Info :size="18" />
          </BaseIconButton>

          <BaseIconButton v-if="mobile" size="sm" :label="t('navigation.sidebar.close')" @click="emit('close')">
            <PanelLeftClose :size="18" />
          </BaseIconButton>

          <BaseIconButton
            v-else
            :size="isCollapsed ? 'md' : 'sm'"
            data-sidebar-toggle
            :label="isCollapsed ? t('navigation.sidebar.expand') : t('navigation.sidebar.collapse')"
            @click="emit('toggle')"
          >
            <PanelLeftOpen v-if="isCollapsed" :size="19" />
            <PanelLeftClose v-else :size="18" />
          </BaseIconButton>
        </div>
      </div>
    </template>
  </BaseSidebar>
</template>
