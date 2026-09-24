<script setup lang="ts">
import type { AccountRow } from '../constants'
import type { AccountReloginAction } from '@/api/modules/relogin'
import { Download, KeyRound, MoreHorizontal, Pencil, RefreshCw, RotateCcw, ShieldCheck, ShieldOff, Trash2, Wifi } from '@lucide/vue'

import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'

defineProps<{
  account: AccountRow
  deleting: boolean
  recovering: boolean
  refreshing: boolean
  togglingTurnState: boolean
  testing: boolean
  exportingModelCatalog?: boolean
  relogin?: AccountReloginAction
  reloginUnavailable?: boolean
}>()

const emit = defineEmits<{
  edit: [account: AccountRow]
  delete: [account: AccountRow]
  recover: [accountId: string]
  test: [account: AccountRow]
  refresh: [accountId: string]
  reauthorize: [account: AccountRow]
  toggleTurnState: [account: AccountRow, enabled: boolean]
  relogin: [account: AccountRow]
  exportModelCatalog: [account: AccountRow]
}>()
</script>

<template>
  <div class="relative flex items-center justify-start gap-1">
    <BaseIconButton
      variant="ghost"
      size="sm"
      label="编辑账号"
      @click.stop="emit('edit', account)"
    >
      <Pencil class="size-3.5 text-cp-link" />
    </BaseIconButton>

    <BaseIconButton
      variant="ghost"
      size="sm"
      label="删除账号"
      :disabled="deleting"
      @click.stop="emit('delete', account)"
    >
      <Trash2 class="size-3.5 text-cp-error" />
    </BaseIconButton>

    <BasePopover placement="bottom-end">
      <template #trigger="{ open }">
        <BaseIconButton variant="ghost" size="sm" label="更多操作" :pressed="open">
          <MoreHorizontal class="size-4" />
        </BaseIconButton>
      </template>

      <template #default="{ close }">
        <div role="group" aria-label="账号操作" class="w-40 p-1.5">
          <BaseMenuItem
            :loading="testing"
            :disabled="testing"
            @click.stop="(close(), emit('test', account))"
          >
            <template #icon>
              <Wifi class="size-3.5 text-cp-text-quaternary" />
            </template>
            测试连接
          </BaseMenuItem>
          <BaseMenuItem
            v-if="account.provider === 'openai'"
            :loading="exportingModelCatalog"
            :disabled="exportingModelCatalog"
            @click.stop="(close(), emit('exportModelCatalog', account))"
          >
            <template #icon>
              <Download class="size-3.5 text-cp-text-quaternary" />
            </template>
            导出模型目录
          </BaseMenuItem>
          <BaseMenuItem
            :loading="refreshing"
            :disabled="refreshing"
            @click.stop="(close(), emit('refresh', account.id))"
          >
            <template #loading>
              <RefreshCw class="size-3.5 animate-spin text-cp-text-quaternary motion-reduce:animate-none" />
            </template>
            <template #icon>
              <RefreshCw class="size-3.5 text-cp-text-quaternary" />
            </template>
            刷新令牌
          </BaseMenuItem>
          <BaseMenuItem @click.stop="(close(), emit('reauthorize', account))">
            <template #icon>
              <KeyRound class="size-3.5 text-cp-text-quaternary" />
            </template>
            重新授权
          </BaseMenuItem>
          <BaseMenuItem
            v-if="relogin"
            :loading="relogin.busy"
            :disabled="reloginUnavailable || relogin.busy || Boolean(relogin.blockedReason)"
            :title="reloginUnavailable ? '重登状态读取失败，正在重试' : relogin.blockedReason || relogin.message"
            @click.stop="(close(), emit('relogin', account))"
          >
            <template #icon>
              <RotateCcw class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ relogin.busy ? '重登处理中' : '失效重登' }}
          </BaseMenuItem>
          <p
            v-if="relogin && !relogin.busy && (relogin.status === 'failed' || relogin.status === 'uncertain')"
            class="px-2 py-1 text-cp-xs break-words text-cp-error"
          >
            {{ relogin.message }}
          </p>
          <BaseMenuItem
            :loading="recovering"
            :disabled="recovering"
            @click.stop="(close(), emit('recover', account.id))"
          >
            <template #icon>
              <RotateCcw class="size-3.5 text-cp-text-quaternary" />
            </template>
            恢复状态
          </BaseMenuItem>
          <BaseMenuItem
            v-if="account.provider === 'openai'"
            :loading="togglingTurnState"
            :disabled="togglingTurnState"
            @click.stop="(close(), emit('toggleTurnState', account, !account.turnStateInjectionEnabled))"
          >
            <template #icon>
              <ShieldOff v-if="account.turnStateInjectionEnabled" class="size-3.5 text-cp-text-quaternary" />
              <ShieldCheck v-else class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ account.turnStateInjectionEnabled ? '关闭 State 注入' : '开启 State 注入' }}
          </BaseMenuItem>
        </div>
      </template>
    </BasePopover>
  </div>
</template>
