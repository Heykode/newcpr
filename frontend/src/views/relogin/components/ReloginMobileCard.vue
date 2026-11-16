<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { Layers, Monitor, MoreHorizontal } from '@lucide/vue'
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
import { maintenanceBusy, maintenanceResult, recentOperation, recentOperationAt } from '../maintenance'
import { credentialLabel, recoveryCountdown, reloginDisplayName, retryProgress, workspaceId } from '../presentation'
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
</script>

<template>
  <BaseCard padding="compact" :aria-label="`重登账号 ${reloginDisplayName(row)}`">
    <div class="flex min-w-0 items-start gap-3">
      <BaseCheckbox :label="`选择 ${row.email}`" :model-value="selected" :disabled="busy" @update:model-value="$emit('select', $event)" />
      <div class="min-w-0 flex-1">
        <button type="button" class="max-w-full break-all text-left font-semibold text-cp-link hover:underline" title="查看 2FA / JSON" @click="$emit('credentials')">
          {{ reloginDisplayName(row) }}
        </button>
        <p v-if="reloginDisplayName(row) !== row.email" class="m-0 mt-1 break-all text-cp-xs text-cp-text-secondary">
          {{ row.email }}
        </p>
      </div>
      <AccountPlanBadge class="max-w-[38%] shrink-0" :plan-type="row.planType" :plan-type-display="row.planType?.toUpperCase() || '待识别'" size="sm" />
    </div>
    <div class="my-4 grid min-w-0 grid-cols-2 gap-4 text-cp-sm">
      <ReloginSessionStatus :row="row" />
      <ReloginEgress :row="row" />
      <div class="col-span-2 flex flex-wrap items-start justify-between gap-2">
        <div>
          <button type="button" class="mb-1 block text-left font-medium text-cp-link hover:underline" @click="$emit('history')">
            {{ recentOperation(row) }}
          </button>
          <button v-if="row.status === 'awaiting_workspace'" type="button" class="text-left text-cp-warning underline" :disabled="busy || paused" @click="$emit('workspace')">
            待选择工作区
          </button>
          <span v-else :class="maintenanceResult(row).tone" :title="row.maintenance?.message || row.message">{{ maintenanceResult(row).label }}</span>
          <span v-if="retryProgress(row)" class="block text-cp-xs text-cp-text-tertiary">{{ retryProgress(row) }}</span>
          <span v-if="row.recovery?.retryAt" class="block text-cp-xs text-cp-text-tertiary">{{ recoveryCountdown(row, now) }}</span>
        </div>
        <button v-if="recentOperationAt(row)" type="button" class="whitespace-nowrap text-cp-xs tabular-nums text-cp-link hover:underline" @click="$emit('history')">
          {{ formatDateTime(recentOperationAt(row)) }}
        </button>
      </div>
    </div>
    <details class="mb-3 border-t border-cp-border-secondary pt-3 text-cp-xs">
      <summary class="cursor-pointer text-cp-text-secondary">
        凭据与工作区详情
      </summary>
      <dl class="my-3 grid gap-3">
        <div>
          <dt class="text-cp-text-secondary">
            OAuth 凭据
          </dt><dd class="m-0 mt-1">
            {{ credentialLabel(row) }}
          </dd>
        </div>
        <div>
          <dt class="text-cp-text-secondary">
            工作区
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
        <div>
          <dt class="text-cp-text-secondary">
            最近操作时间
          </dt><dd class="m-0 mt-1">
            <button type="button" class="text-cp-link" @click="$emit('history')">
              {{ formatDateTime(recentOperationAt(row)) }}
            </button>
          </dd>
        </div>
      </dl>
    </details>
    <div class="flex flex-wrap items-center justify-between gap-3 border-t border-cp-border-secondary pt-3">
      <div class="flex items-center gap-2">
        <span class="text-cp-xs text-cp-text-secondary">自动恢复</span><BaseSwitch :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="$emit('automatic', $event)" />
      </div>
      <div class="flex flex-wrap items-center gap-1">
        <BaseButton v-if="canPush" size="sm" variant="soft" :disabled="busy" @click="$emit('push')">
          推送凭据
        </BaseButton>
        <BaseButton v-else size="sm" :disabled="busy || paused || maintenanceBusy(row)" @click="$emit('queue')">
          立即重登
        </BaseButton>
        <BaseIconButton label="设备与会话" size="sm" @click="$emit('maintain', 'devices')">
          <Monitor class="size-4" />
        </BaseIconButton>
        <BaseIconButton label="工作区" size="sm" @click="$emit('maintain', 'workspaces')">
          <Layers class="size-4" />
        </BaseIconButton>
        <BasePopover trigger="click" placement="bottom-end">
          <template #trigger>
            <BaseIconButton label="更多账号操作" size="sm">
              <MoreHorizontal class="size-4" />
            </BaseIconButton>
          </template>
          <template #default="{ close }">
            <div class="min-w-40 p-1">
              <BaseMenuItem :disabled="busy || paused || maintenanceBusy(row)" @click="close(); $emit('queue')">
                立即重登
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
    </div>
  </BaseCard>
</template>
