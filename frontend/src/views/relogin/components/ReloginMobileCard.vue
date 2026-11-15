<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { Settings2, Trash2 } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import ReloginCountCell from '@/components/ReloginCountCell.vue'
import { formatDateTime } from '@/utils/date'
import { credentialLabel, poolPresentation, processingStatus, recoveryCountdown, reloginDisplayName, retryProgress, shortWorkspace, workspaceId } from '../presentation'

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
        <p class="m-0 mt-1 break-words text-cp-xs text-cp-text-secondary">
          {{ row.message || '待处理' }}
        </p>
      </div>
      <span class="max-w-[38%] shrink-0 break-all rounded-cp-sm bg-cp-fill-tertiary px-2 py-1 font-mono text-cp-xs">{{ row.planType?.toUpperCase() ?? '待识别' }}</span>
    </div>
    <dl class="my-4 grid grid-cols-2 gap-x-4 gap-y-3 text-cp-sm">
      <div class="min-w-0">
        <dt class="mb-1 text-cp-xs text-cp-text-secondary">
          处理状态
        </dt><dd class="m-0 break-words">
          <button v-if="row.status === 'awaiting_workspace'" type="button" class="text-left text-cp-warning underline" :disabled="busy || paused" @click="$emit('workspace')">
            待选择工作区
          </button>
          <span v-else :class="processingStatus(row).tone" :title="processingStatus(row).detail">{{ processingStatus(row).label }}</span>
        </dd>
      </div>
      <div class="min-w-0">
        <dt class="mb-1 text-cp-xs text-cp-text-secondary">
          号池状态
        </dt><dd class="m-0 break-words" :class="poolPresentation(row).tone" :title="poolPresentation(row).detail">
          {{ poolPresentation(row).label }}<span class="block text-cp-xs text-cp-text-secondary">{{ poolPresentation(row).caption }}</span>
        </dd>
      </div>
      <div class="min-w-0">
        <dt class="mb-1 text-cp-xs text-cp-text-secondary">
          本次凭据
        </dt><dd class="m-0 break-words">
          {{ credentialLabel(row) }}<span v-if="row.verifiedAt" class="block text-cp-xs text-cp-text-secondary">{{ formatDateTime(row.verifiedAt) }}</span>
        </dd>
      </div>
      <div class="min-w-0">
        <dt class="mb-1 text-cp-xs text-cp-text-secondary">
          工作区
        </dt><dd class="m-0 break-all font-mono text-cp-xs" :title="workspaceId(row) || undefined">
          {{ shortWorkspace(workspaceId(row)) }}
        </dd>
      </div>
      <div class="min-w-0">
        <dt class="mb-1 text-cp-xs text-cp-text-secondary">
          上次重登
        </dt><dd class="m-0">
          <button v-if="row.lastAttemptAt || row.lastReloginAt" type="button" class="text-left text-cp-link hover:underline" title="查看重登详情" @click="$emit('history')">
            {{ formatDateTime(row.lastAttemptAt || row.lastReloginAt) }}
          </button><span v-else>—</span>
        </dd>
      </div>
      <div>
        <dt class="mb-1 text-cp-xs text-cp-text-secondary">
          成功重登次数
        </dt><dd class="m-0">
          <ReloginCountCell :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" />
        </dd>
      </div>
    </dl>
    <div v-if="row.recovery?.state !== 'idle' && (row.recovery?.message || retryProgress(row))" class="mb-3 rounded-cp bg-cp-fill-quaternary p-3 text-cp-xs text-cp-text-secondary">
      <p class="m-0 break-words">
        {{ row.recovery?.message }}
      </p>
      <p v-if="retryProgress(row) || row.recovery?.retryAt" class="m-0 mt-1 tabular-nums">
        {{ retryProgress(row) }} {{ recoveryCountdown(row, now) }}
      </p>
    </div>
    <div class="flex flex-wrap items-center justify-between gap-3 border-t border-cp-border-secondary pt-3">
      <div class="flex items-center gap-2">
        <span class="text-cp-xs text-cp-text-secondary">自动重登</span><BaseSwitch :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="$emit('automatic', $event)" />
      </div>
      <div class="flex items-center gap-1">
        <BaseButton size="sm" :disabled="busy || ['queued', 'running', 'pushing'].includes(row.status)" @click="$emit('queue')">
          重登
        </BaseButton>
        <BaseButton size="sm" variant="soft" :disabled="busy || !canPush" @click="$emit('push')">
          推送
        </BaseButton>
        <BaseIconButton label="选择工作区" size="sm" :disabled="busy || ['queued', 'running', 'pushing', 'uncertain'].includes(row.status)" @click="$emit('workspace')">
          <Settings2 :size="15" />
        </BaseIconButton>
        <BaseIconButton label="删除重登资料" size="sm" :disabled="busy" @click="$emit('delete')">
          <Trash2 :size="15" class="text-cp-error" />
        </BaseIconButton>
      </div>
    </div>
    <p class="m-0 mt-3 text-cp-xs text-cp-text-tertiary">
      导入 {{ row.importedAt ? formatDateTime(row.importedAt) : '时间未记录' }}
    </p>
  </BaseCard>
</template>
