<script setup lang="ts">
import type { AccountRow } from '../constants'
import type { AccountReloginAction, CredentialSelection } from '@/api/modules/relogin'
import { ClipboardCheck, Download, KeyRound, MoreHorizontal, Pencil, RefreshCw, RotateCcw, Table2, Trash2, Wifi } from '@lucide/vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'

import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { t } from '@/i18n'

defineProps<{
  account: AccountRow
  comfortable?: boolean
  deleting: boolean
  recovering: boolean
  refreshing: boolean
  togglingExcel?: boolean
  togglingQuality?: boolean
  qualityDisabled?: boolean
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
  toggleExcel: [account: AccountRow, enabled: boolean]
  toggleQuality: [account: AccountRow]
  relogin: [account: AccountRow, selection: CredentialSelection]
  exportModelCatalog: [account: AccountRow]
}>()
</script>

<template>
  <div class="relative flex items-center justify-start gap-1">
    <BaseIconButton
      variant="ghost"
      :size="comfortable ? 'md' : 'sm'"
      :class="comfortable ? 'size-8!' : undefined"
      :label="t('accounts.editor.title')"
      @click.stop="emit('edit', account)"
    >
      <Pencil class="size-3.5 text-cp-link" />
    </BaseIconButton>

    <BaseIconButton
      variant="ghost"
      :size="comfortable ? 'md' : 'sm'"
      :class="comfortable ? 'size-8!' : undefined"
      :label="t('accounts.actions.deleteAccount')"
      :disabled="deleting"
      @click.stop="emit('delete', account)"
    >
      <Trash2 class="size-3.5 text-cp-error" />
    </BaseIconButton>

    <BasePopover placement="bottom-end">
      <template #trigger="{ open }">
        <BaseIconButton variant="ghost" :size="comfortable ? 'md' : 'sm'" :class="comfortable ? 'size-8!' : undefined" :label="t('accounts.actions.moreActions')" :pressed="open">
          <MoreHorizontal class="size-4" />
        </BaseIconButton>
      </template>

      <template #default="{ close }">
        <div role="group" :aria-label="t('appearance.preview.components.accountOperation')" class="w-40 p-1.5">
          <BaseMenuItem
            :loading="testing"
            :disabled="testing"
            @click.stop="(close(), emit('test', account))"
          >
            <template #icon>
              <Wifi class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ t('accounts.connectionTest.title') }}
          </BaseMenuItem>
          <BaseMenuItem :loading="togglingQuality" :disabled="togglingQuality || qualityDisabled" @click.stop="(close(), emit('toggleQuality', account))">
            <template #icon>
              <ClipboardCheck class="size-3.5 text-cp-text-quaternary" />
            </template>
            <span class="inline-flex w-full items-center justify-between gap-2">
              质量检测开关
              <span :class="account.qualityMonitoring?.enabled ? 'text-cp-success' : 'text-cp-text-secondary'" class="text-cp-xs">{{ account.qualityMonitoring?.enabled ? '开' : '关' }}</span>
            </span>
          </BaseMenuItem>
          <BaseMenuItem
            v-if="account.provider === 'openai' && ['oauth', 'none'].includes(account.authenticationKind)"
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
            {{ t('accounts.actions.refreshToken') }}
          </BaseMenuItem>
          <BaseMenuItem @click.stop="(close(), emit('reauthorize', account))">
            <template #icon>
              <KeyRound class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ t('accounts.actions.reauthorize') }}
          </BaseMenuItem>
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
            :loading="togglingExcel"
            :disabled="togglingExcel"
            @click.stop="(close(), emit('toggleExcel', account, account.responsesUpstream !== 'excel'))"
          >
            <template #icon>
              <Table2 class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ account.responsesUpstream === 'excel' ? '关闭 Excel 入口' : '开启 Excel 入口' }}
          </BaseMenuItem>
          <BaseMenuItem
            v-if="relogin"
            :loading="relogin.busy"
            :disabled="reloginUnavailable || relogin.busy || Boolean(relogin.blockedReason)"
            :title="reloginUnavailable ? '重登状态读取失败，正在重试' : relogin.blockedReason || relogin.message"
            @click.stop="(close(), emit('relogin', account, 'codex'))"
          >
            <template #icon>
              <RotateCcw class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ relogin.busy ? 'Codex 重登中' : '重登 Codex' }}
          </BaseMenuItem>
          <BaseMenuItem
            v-if="relogin?.excel"
            :loading="relogin.excel.busy"
            :disabled="reloginUnavailable || relogin.excel.busy || Boolean(relogin.excel.blockedReason)"
            :title="relogin.excel.blockedReason || relogin.excel.message"
            @click.stop="(close(), emit('relogin', account, 'excel'))"
          >
            <template #icon>
              <RotateCcw class="size-3.5 text-cp-text-quaternary" />
            </template>
            {{ relogin.excel.busy ? 'Excel 重登中' : '重登 Excel' }}
          </BaseMenuItem>
          <BaseMenuItem
            v-if="relogin?.excel"
            :disabled="reloginUnavailable || relogin.busy || relogin.excel.busy || Boolean(relogin.blockedReason || relogin.excel.blockedReason)"
            @click.stop="(close(), emit('relogin', account, 'all'))"
          >
            <template #icon>
              <RotateCcw class="size-3.5 text-cp-text-quaternary" />
            </template>
            重登两种凭据
          </BaseMenuItem>
          <p
            v-if="relogin && !relogin.busy && (relogin.status === 'failed' || relogin.status === 'uncertain')"
            class="px-2 py-1 text-cp-xs break-words text-cp-error"
          >
            {{ relogin.message }}
          </p>
          <p v-if="relogin?.excel && !relogin.excel.busy && ['failed', 'uncertain'].includes(relogin.excel.status)" class="px-2 py-1 text-cp-xs break-words text-cp-error">
            Excel：{{ relogin.excel.message }}
          </p>
        </div>
      </template>
    </BasePopover>
  </div>
</template>
