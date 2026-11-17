<script setup lang="ts">
import type { getApiKeys } from '@/api'
import { Pencil, Power, RotateCcw, Terminal, Trash2, Upload } from '@lucide/vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'

import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import RowActionMenu from '@/components/RowActionMenu.vue'
import { t } from '@/i18n'

type ApiKeyRow = Awaited<ReturnType<typeof getApiKeys>>['items'][number]

defineProps<{
  apiKey: ApiKeyRow
  deleting: boolean
  updatingStatus: boolean
  revealing: boolean
  configurationLoading?: boolean
  importLoading?: boolean
}>()

const emit = defineEmits<{
  use: [apiKey: ApiKeyRow]
  importCcs: [apiKey: ApiKeyRow]
  toggle: [apiKey: ApiKeyRow]
  delete: [apiKey: ApiKeyRow]
  edit: [apiKey: ApiKeyRow]
  resetBudget: [apiKey: ApiKeyRow]
}>()
</script>

<template>
  <div class="flex items-center justify-start gap-0.5">
    <BaseIconButton
      variant="ghost"
      size="sm"
      :label="t('keys.actions.editKey')"
      @click.stop="emit('edit', apiKey)"
    >
      <Pencil class="size-3.5 text-cp-link" />
    </BaseIconButton>
    <BaseIconButton
      variant="ghost"
      size="sm"
      :label="t('keys.configuration.title')"
      :loading="configurationLoading"
      :disabled="revealing"
      @click.stop="emit('use', apiKey)"
    >
      <Terminal class="size-3.5 text-cp-primary-text" />
    </BaseIconButton>

    <RowActionMenu label="更多密钥操作" menu-class="w-44 p-1">
      <BaseMenuItem :disabled="revealing" :loading="importLoading" @click.stop="emit('importCcs', apiKey)">
        <template #icon>
          <Upload class="size-3.5" />
        </template>
        导入 CCSwitch
      </BaseMenuItem>
      <BaseMenuItem @click.stop="emit('resetBudget', apiKey)">
        <template #icon>
          <RotateCcw class="size-3.5" />
        </template>
        重置已用额度
      </BaseMenuItem>
      <BaseMenuItem :loading="updatingStatus" @click.stop="emit('toggle', apiKey)">
        <template #icon>
          <Power class="size-3.5" />
        </template>
        {{ apiKey.enabled ? '禁用密钥' : '启用密钥' }}
      </BaseMenuItem>
      <BaseMenuItem tone="destructive" :disabled="deleting" @click.stop="emit('delete', apiKey)">
        <template #icon>
          <Trash2 class="size-3.5" />
        </template>
        删除密钥
      </BaseMenuItem>
    </RowActionMenu>
  </div>
</template>
