<script setup lang="ts">
import type { EnrollmentEntry } from '../enrollment-plan'
import { ChevronRight, Layers, Monitor, MoreHorizontal } from '@lucide/vue'
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
import { canArrange } from '../enrollment-plan'
import { maintenanceBusy, maintenanceResult, queueButtonLabel, queueDisabledReason, recentOperation, recentOperationAt } from '../maintenance'
import { recoveryCountdown, reloginDisplayName, reloginPlanDisplay, retryProgress, shortWorkspace, workspaceId, workspaceLabel } from '../presentation'
import EnrollmentPlanStatus from './EnrollmentPlanStatus.vue'
import ReloginEgress from './ReloginEgress.vue'
import ReloginSessionStatus from './ReloginSessionStatus.vue'

defineProps<{ row: EnrollmentEntry, selected: boolean, busy: boolean, paused: boolean, canPush: boolean, now: number }>()
defineEmits<{
  arrange: []
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
        <AccountPlanBadge :plan-type="row.planType" :plan-type-display="reloginPlanDisplay(row)" size="sm" />
      </div>
    </div>
    <div class="mt-2 grid min-w-0 gap-1.5 rounded-cp bg-cp-fill-quaternary p-2.5">
      <div class="flex min-w-0 items-start gap-2">
        <Layers class="mt-0.5 size-3.5 shrink-0 text-cp-text-tertiary" />
        <span class="min-w-0 break-words text-cp-xs font-semibold text-cp-text-secondary [overflow-wrap:anywhere]">{{ workspaceLabel(row) === shortWorkspace(workspaceId(row)) ? '工作区' : workspaceLabel(row) }}</span>
      </div>
      <div class="flex min-w-0 items-baseline gap-2">
        <span class="shrink-0 text-[10px] font-medium text-cp-text-tertiary">ID</span>
        <span class="min-w-0 break-all font-mono text-cp-xs text-cp-text-secondary">{{ workspaceId(row) || '自动选择' }}</span>
      </div>
    </div>
    <div class="mt-3 grid min-w-0 gap-3 text-cp-sm">
      <ReloginSessionStatus :row="row" compact />
      <div class="flex min-w-0 items-start gap-3">
        <span class="shrink-0 pt-0.5 text-cp-xs text-cp-text-tertiary">出口</span>
        <ReloginEgress class="flex-1" :row="row" />
      </div>
      <EnrollmentPlanStatus v-if="row.enrollmentPlan" :row="row" />
      <div v-if="!row.enrollmentPlan || !['draft', 'scheduled'].includes(row.enrollmentPlan.state)">
        <button type="button" class="flex min-h-11 w-full items-center gap-2 rounded-cp text-left hover:bg-cp-fill-quaternary" aria-label="查看操作历史" @click="$emit('history')">
          <span class="min-w-0 flex-1">
            <span class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span class="break-words font-semibold text-cp-link">{{ recentOperation(row) }}</span>
              <span v-if="row.status !== 'awaiting_workspace'" class="rounded-full bg-cp-fill-tertiary px-2 py-0.5 text-cp-xs font-medium" :class="maintenanceResult(row).tone" :title="row.maintenance?.message || row.message">{{ maintenanceResult(row).label }}</span>
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
      <div class="flex min-w-0 flex-wrap items-center gap-1.5 text-cp-xs text-cp-text-secondary">
        <span>成功重登</span>
        <span class="inline-flex min-w-7 items-center justify-center rounded-full bg-cp-fill-tertiary px-2 py-0.5">
          <ReloginCountCell class="text-cp-sm font-semibold" :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" />
        </span>
        <span v-if="row.reloginCount != null">次</span>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <span class="text-cp-xs text-cp-text-secondary">自动恢复</span>
        <BaseSwitch class="min-h-11" :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="$emit('automatic', $event)" />
      </div>
    </div>
    <div class="flex min-w-0 items-center gap-1 border-t border-cp-border-secondary pt-2">
      <BaseButton v-if="canArrange(row)" class="min-h-11 min-w-0 flex-1" variant="soft" :disabled="busy || ['queued', 'running', 'pushing', 'uncertain'].includes(row.status)" @click="$emit('arrange')">
        {{ row.enrollmentPlan ? '查看安排' : '安排入池' }}
      </BaseButton>
      <BaseButton v-else-if="canPush" class="min-h-11 min-w-0 flex-1 whitespace-nowrap px-3" variant="soft" :disabled="busy" @click="$emit('push')">
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
            <BaseMenuItem v-if="canArrange(row) && canPush" :disabled="busy || row.enrollmentPlan?.state === 'scheduled'" @click="close(); $emit('push')">
              推送已有凭据
            </BaseMenuItem>
            <BaseMenuItem v-if="canArrange(row)" :disabled="!!queueDisabledReason(row, busy, paused) || row.enrollmentPlan?.state === 'scheduled'" @click="close(); $emit('queue')">
              仅重登，不推送
            </BaseMenuItem>
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
