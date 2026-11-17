<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { ChevronDown, ChevronRight, Layers, Monitor, MoreHorizontal } from '@lucide/vue'
import { shallowRef, useId } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import ReloginCountCell from '@/components/ReloginCountCell.vue'
import { formatDateTime } from '@/utils/date'
import AccountPlanBadge from '../../accounts/components/AccountPlanBadge.vue'
import { maintenanceBusy, maintenanceResult, queueButtonLabel, queueDisabledReason, recentOperation, recentOperationAt } from '../maintenance'
import { credentialLabel, recoveryCountdown, reloginDisplayName, retryProgress, workspaceId, workspaceLabel } from '../presentation'
import ReloginEgress from './ReloginEgress.vue'
import ReloginSessionStatus from './ReloginSessionStatus.vue'

defineProps<{ row: ReloginEntry, selected: boolean, busy: boolean, paused: boolean, canPush: boolean, now: number }>()
defineEmits<{
  select: [value: boolean]
  automatic: [value: boolean]
  credentials: []
  history: []
  queue: []
  push: []
  workspace: []
  delete: []
  cancel: []
  maintain: [tab: string]
}>()
const detailsOpen = shallowRef(false)
const detailsId = useId()
</script>

<template>
  <BaseCard padding="compact" :aria-label="`重登账号 ${reloginDisplayName(row)}`">
    <div class="flex min-w-0 items-start gap-2">
      <BaseCheckbox class="min-h-11 min-w-11 justify-center" :label="`选择 ${row.email}`" :model-value="selected" :disabled="busy" @update:model-value="$emit('select', $event)" />
      <button type="button" class="min-h-11 min-w-0 flex-1 text-left" title="查看 2FA / JSON" @click="$emit('credentials')">
        <span class="block break-words font-semibold text-cp-link [overflow-wrap:anywhere]">{{ reloginDisplayName(row) }}</span>
        <span v-if="reloginDisplayName(row) !== row.email" class="mt-1 block break-all text-cp-xs text-cp-text-secondary">
          {{ row.email }}
        </span>
      </button>
      <div class="flex min-h-11 max-w-[38%] shrink-0 items-center">
        <AccountPlanBadge :plan-type="row.planType" :plan-type-display="row.planType?.toUpperCase() || '待识别'" size="sm" />
      </div>
    </div>
    <div class="mt-2 flex min-w-0 items-start gap-2 text-cp-xs text-cp-text-secondary" :title="workspaceId(row) || '自动选择工作区'">
      <Layers class="mt-0.5 size-3.5 shrink-0" />
      <span class="min-w-0 break-words [overflow-wrap:anywhere]">{{ workspaceLabel(row) }}</span>
    </div>
    <div class="mt-3 grid min-w-0 gap-3 text-cp-sm">
      <ReloginSessionStatus :row="row" compact />
      <div class="flex min-w-0 items-start gap-3">
        <span class="shrink-0 pt-0.5 text-cp-xs text-cp-text-tertiary">出口</span>
        <ReloginEgress class="flex-1" :row="row" />
      </div>
      <div>
        <button type="button" class="flex min-h-11 w-full items-center gap-2 rounded-cp text-left hover:bg-cp-fill-quaternary" aria-label="查看操作历史" @click="$emit('history')">
          <span class="min-w-0 flex-1">
            <span class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span class="break-words font-medium text-cp-link">{{ recentOperation(row) }}</span>
              <span v-if="row.status !== 'awaiting_workspace'" :class="maintenanceResult(row).tone" :title="row.maintenance?.message || row.message">{{ maintenanceResult(row).label }}</span>
            </span>
            <span v-if="recentOperationAt(row)" class="mt-1 block text-cp-xs tabular-nums text-cp-text-tertiary">{{ formatDateTime(recentOperationAt(row)) }}</span>
          </span>
          <ChevronRight class="size-4 shrink-0 text-cp-text-tertiary" />
        </button>
        <BaseButton v-if="row.status === 'awaiting_workspace'" class="mt-1 min-h-11 w-full" variant="soft" :disabled="busy || paused" @click="$emit('workspace')">
          选择工作区并继续
        </BaseButton>
        <span v-if="retryProgress(row)" class="mt-1 block text-cp-xs text-cp-text-tertiary">{{ retryProgress(row) }}</span>
        <span v-if="row.recovery?.retryAt" class="mt-1 block text-cp-xs text-cp-text-tertiary">{{ recoveryCountdown(row, now) }}</span>
      </div>
    </div>
    <div class="mt-2 flex items-center justify-between gap-2 border-t border-cp-border-secondary">
      <button type="button" class="flex min-h-11 items-center gap-1.5 text-cp-xs text-cp-text-secondary" :aria-expanded="detailsOpen" :aria-controls="detailsId" @click="detailsOpen = !detailsOpen">
        更多详情<ChevronDown class="size-3.5 transition-transform" :class="{ 'rotate-180': detailsOpen }" />
      </button>
      <div class="flex shrink-0 items-center gap-2">
        <span class="text-cp-xs text-cp-text-secondary">自动恢复</span>
        <BaseSwitch class="min-h-11" :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="$emit('automatic', $event)" />
      </div>
    </div>
    <div v-if="detailsOpen" :id="detailsId" class="pb-3 text-cp-xs">
      <dl class="m-0 grid gap-3 rounded-cp bg-cp-fill-quaternary p-3">
        <div>
          <dt class="text-cp-text-secondary">
            OAuth 凭据
          </dt><dd class="m-0 mt-1">
            {{ credentialLabel(row) }}
          </dd>
        </div>
        <div>
          <dt class="text-cp-text-secondary">
            工作区 ID
          </dt><dd class="m-0 mt-1 break-all font-mono">
            {{ workspaceId(row) || '自动选择' }}
          </dd>
        </div>
        <div>
          <dt class="text-cp-text-secondary">
            成功重登次数
          </dt><dd class="m-0 mt-1">
            <ReloginCountCell :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" />
          </dd>
        </div>
      </dl>
    </div>
    <div class="flex min-w-0 items-center gap-1 border-t border-cp-border-secondary pt-2">
      <BaseButton v-if="canPush" class="min-h-11 min-w-0 flex-1 whitespace-nowrap px-3" variant="soft" :disabled="busy" @click="$emit('push')">
        推送凭据
      </BaseButton>
      <BaseButton v-else class="min-h-11 min-w-0 flex-1 whitespace-nowrap px-3" :title="queueDisabledReason(row, busy, paused)" :disabled="!!queueDisabledReason(row, busy, paused)" @click="$emit('queue')">
        {{ queueButtonLabel(row, paused) }}
      </BaseButton>
      <BaseIconButton label="设备与会话" class="min-h-11 min-w-11" @click="$emit('maintain', 'devices')">
        <Monitor class="size-4" />
      </BaseIconButton>
      <BaseIconButton label="工作区" class="min-h-11 min-w-11" @click="$emit('maintain', 'workspaces')">
        <Layers class="size-4" />
      </BaseIconButton>
      <BasePopover trigger="click" placement="bottom-end">
        <template #trigger>
          <BaseIconButton label="更多账号操作" class="min-h-11 min-w-11">
            <MoreHorizontal class="size-4" />
          </BaseIconButton>
        </template>
        <template #default="{ close }">
          <div class="min-w-48 p-1 [&_button]:min-h-11">
            <BaseMenuItem @click="close(); $emit('maintain', 'session')">
              登录与授权
            </BaseMenuItem>
            <BaseMenuItem @click="close(); $emit('credentials')">
              账号资料与复制
            </BaseMenuItem>
            <BaseMenuItem @click="close(); $emit('maintain', 'security')">
              安全维护
            </BaseMenuItem>
            <BaseMenuItem @click="close(); $emit('history')">
              操作历史
            </BaseMenuItem>
            <BaseMenuItem :disabled="busy || maintenanceBusy(row)" @click="close(); $emit('workspace')">
              自动恢复工作区
            </BaseMenuItem>
            <BaseMenuItem v-if="maintenanceBusy(row)" @click="close(); $emit('cancel')">
              取消任务
            </BaseMenuItem>
            <BaseMenuItem tone="destructive" :disabled="busy" @click="close(); $emit('delete')">
              删除本地资料
            </BaseMenuItem>
          </div>
        </template>
      </BasePopover>
    </div>
  </BaseCard>
</template>
