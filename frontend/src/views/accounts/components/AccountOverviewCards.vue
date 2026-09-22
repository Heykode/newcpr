<script setup lang="ts">
import type { AccountGroup, getAccounts } from '@/api'
import { ChevronLeft, ChevronRight, Pin, PinOff, RefreshCw } from '@lucide/vue'
import { useElementSize } from '@vueuse/core'
import { computed, ref, toRef } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseScrollbar from '@/components/base/BaseScrollbar.vue'
import { formatInteger } from '@/utils/number'
import { useGroupMonitor } from '../composables/useGroupMonitor'
import AccountGroupMonitorCard from './AccountGroupMonitorCard.vue'
import GroupAlertSettingsModal from './GroupAlertSettingsModal.vue'

const props = defineProps<{
  summary: Awaited<ReturnType<typeof getAccounts>>['summary']
  groups: AccountGroup[]
  groupsLoading: boolean
}>()
const root = ref<HTMLElement>()
const alertGroup = ref<AccountGroup | null>(null)
const alertOpen = ref(false)
function openAlertSettings(group: AccountGroup) {
  alertGroup.value = group
  alertOpen.value = true
}
const { width } = useElementSize(root)
const pageSize = computed(() => width.value >= 960 ? 3 : width.value >= 650 ? 2 : 1)
const {
  page,
  pins,
  viewer,
  visible,
  displayedCount,
  totalPages,
  records,
  stale,
  loading,
  error,
  now,
  togglePin,
  refreshNow,
} = useGroupMonitor(toRef(props, 'groups'), pageSize)
</script>

<template>
  <div ref="root" class="monitor-overview mt-3 shrink-0" :style="{ '--monitor-columns': Math.max(1, visible.length) }">
    <BaseCard as="article" padding="none" class="monitor-summary" aria-label="账号总数和正常账号">
      <div>
        <div class="flex items-center justify-between gap-1">
          <p class="m-0 text-cp-xs font-emphasis text-cp-text-secondary">
            总账号
          </p>
          <BasePopover v-if="groups.length" trigger="click" placement="bottom-start">
            <template #trigger>
              <BaseIconButton label="置顶分组" size="sm" class="monitor-action" :disabled="!viewer">
                <Pin class="size-3" />
              </BaseIconButton>
            </template>
            <BaseScrollbar max-height="260px" class="w-[min(280px,calc(100vw-32px))]">
              <div class="p-2">
                <div v-for="group in groups" :key="group.id" class="flex min-w-0 items-center gap-2 py-1">
                  <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: group.color }" />
                  <span class="min-w-0 flex-1 truncate text-cp-xs" :title="group.name">{{ group.name }}</span>
                  <span class="shrink-0 text-cp-xs text-cp-text-tertiary">{{ formatInteger(group.memberCount) }}</span>
                  <BaseIconButton
                    :label="`${pins.includes(group.id) ? '取消置顶' : '置顶'} ${group.name}`"
                    size="sm"
                    class="monitor-action"
                    :pressed="pins.includes(group.id)"
                    @click="togglePin(group.id)"
                  >
                    <component :is="pins.includes(group.id) ? PinOff : Pin" class="size-3" />
                  </BaseIconButton>
                </div>
              </div>
            </BaseScrollbar>
          </BasePopover>
        </div>
        <strong class="block font-mono text-[22px] leading-7 font-heavy text-cp-text">{{ formatInteger(summary.total) }}</strong>
      </div>
      <p class="m-0 text-cp-xs text-cp-success-text">
        正常 <strong class="ml-1 font-mono">{{ formatInteger(summary.normal) }}</strong>
      </p>
      <div class="mt-auto flex flex-wrap items-center justify-between gap-1 border-t border-cp-border-secondary pt-1">
        <span class="text-cp-xs text-cp-text-tertiary">分组 {{ displayedCount ? page + 1 : 0 }}/{{ displayedCount ? totalPages : 0 }}</span>
        <div class="flex">
          <BaseIconButton label="上一页分组" size="sm" class="monitor-action" :disabled="page === 0" @click="page -= 1">
            <ChevronLeft class="size-3.5" />
          </BaseIconButton>
          <BaseIconButton label="下一页分组" size="sm" class="monitor-action" :disabled="page >= totalPages - 1" @click="page += 1">
            <ChevronRight class="size-3.5" />
          </BaseIconButton>
        </div>
      </div>
    </BaseCard>
    <AccountGroupMonitorCard
      v-for="(group, index) in visible"
      :key="group.id"
      :group="group"
      :snapshot="records.get(group.id)"
      :pinned="pins.includes(group.id)"
      :pin-disabled="!viewer"
      :stale="stale"
      :loading="loading"
      :now="now"
      @pin="togglePin(group.id)"
      @settings="openAlertSettings(group)"
    >
      <template v-if="index === visible.length - 1" #actions>
        <BaseIconButton label="立即刷新分组监控" size="sm" class="monitor-action" :loading="loading" @click="refreshNow">
          <RefreshCw class="size-3.5" />
        </BaseIconButton>
      </template>
    </AccountGroupMonitorCard>
    <BaseCard v-if="!visible.length" as="article" padding="compact" class="monitor-empty relative flex items-center justify-center text-cp-sm text-cp-text-tertiary">
      {{ groupsLoading || loading ? '读取分组中' : '暂无展示分组' }}
      <BaseIconButton label="立即刷新分组监控" size="sm" class="monitor-action absolute top-2 right-2" disabled>
        <RefreshCw class="size-3.5" />
      </BaseIconButton>
    </BaseCard>
    <div v-if="error" class="monitor-error flex items-center justify-end gap-1 text-cp-xs text-cp-warning-text" role="status">
      监控更新失败
      <BaseIconButton label="重试监控更新" size="sm" :loading="loading" @click="refreshNow">
        <RefreshCw class="size-3.5" />
      </BaseIconButton>
    </div>
    <GroupAlertSettingsModal v-model="alertOpen" :group="alertGroup" :snapshot="alertGroup ? records.get(alertGroup.id) : undefined" :stale="stale" :now="now" />
  </div>
</template>

<style scoped>
.monitor-overview {
  display: grid;
  grid-template-columns: 124px repeat(var(--monitor-columns), minmax(0, 1fr));
  gap: 10px;
  min-width: 0;
  container-type: inline-size;
}
.monitor-action {
  width: 24px;
  height: 24px;
}
.monitor-summary {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px 10px;
  min-height: 128px;
}
.monitor-empty {
  grid-column: 2 / -1;
  min-height: 128px;
}
.monitor-error {
  grid-column: 1 / -1;
}
@media (max-width: 479px) {
  .monitor-overview {
    grid-template-columns: minmax(0, 1fr);
  }
  .monitor-summary {
    min-height: 58px;
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    align-items: center;
    gap: 6px;
  }
  .monitor-summary > div:last-child {
    margin: 0;
    padding: 0;
    border: 0;
  }
  .monitor-summary > div:first-child {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }
  .monitor-summary > div:first-child strong {
    font-size: 20px;
  }
  .monitor-empty {
    grid-column: 1;
    min-height: 128px;
  }
}
</style>
