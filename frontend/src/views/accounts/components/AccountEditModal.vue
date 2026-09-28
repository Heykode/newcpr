<script setup lang="ts">
import type { AccountRow } from '../constants'

import type { AccountGroup, AccountModelAccess } from '@/api'
import type { Excel403Action } from '@/utils/excel-settings'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import AccountIdentityCell from './AccountIdentityCell.vue'
import AccountPlanBadge from './AccountPlanBadge.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  account: AccountRow | null
  groups: AccountGroup[]
  groupsLoading: boolean
  saving: boolean
}>()

const emit = defineEmits<{
  save: []
}>()

const open = defineModel<boolean>({ required: true })
const customName = defineModel<string>('customName', { required: true })
const enabled = defineModel<boolean>('enabled', { required: true })
const excelEnabled = defineModel<boolean>('excelEnabled', { default: false })
const excelModels = defineModel<string>('excelModels', { default: DEFAULT_EXCEL_MODELS_INPUT })
const excelModelsFollowGlobal = defineModel<boolean>('excelModelsFollowGlobal', { default: true })
const excelCacheCreationAsInput = defineModel<boolean>('excelCacheCreationAsInput', { default: false })
const excelIgnoreEncryptedContent = defineModel<boolean>('excelIgnoreEncryptedContent', { default: false })
const requestProxySource = defineModel<import('@/utils/request-proxy-source').RequestProxySource>('requestProxySource', { default: 'account' })
const excel403Action = defineModel<Excel403Action>('excel403Action', { default: 'none' })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const egressMode = defineModel<string>('egressMode', { required: true })
</script>

<template>
  <BaseModal
    v-model="open"
    title="编辑账号"
    description="查看账号信息，并调整调度与所属分组。"
    size="md"
    :dismissible="!saving"
  >
    <div v-if="account" class="grid gap-5">
      <div
        class="flex flex-wrap items-center justify-between gap-4 rounded-cp bg-cp-fill-quaternary px-4 py-3.5"
      >
        <AccountIdentityCell
          class="min-w-0 flex-1"
          :account="account"
          size="lg"
        />
        <div class="flex shrink-0 items-center gap-3">
          <AccountPlanBadge :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" size="sm" />
          <ProviderIconGroup
            :provider="account.provider"
            :authentication-kind="account.authenticationKind"
          />
        </div>
      </div>

      <AccountSettingsFields
        v-model:custom-name="customName"
        v-model:enabled="enabled"
        v-model:excel-enabled="excelEnabled"
        v-model:excel-models="excelModels"
        v-model:excel-models-follow-global="excelModelsFollowGlobal"
        v-model:excel-cache-creation-as-input="excelCacheCreationAsInput"
        v-model:excel-ignore-encrypted-content="excelIgnoreEncryptedContent"
        v-model:request-proxy-source="requestProxySource"
        v-model:excel-403-action="excel403Action"
        v-model:concurrency-limit="concurrencyLimit"
        v-model:weight="weight"
        v-model:model-access="modelAccess"
        v-model:selected-group-ids="selectedGroupIds"
        v-model:proxy-mode="proxyMode"
        v-model:proxy-id="proxyId"
        v-model:egress-mode="egressMode"
        encrypted-content-available
        name-available
        model-access-available
        :groups="groups"
        :groups-loading="groupsLoading"
        :excel-available="account.provider === 'openai' && account.authenticationKind === 'oauth'"
        :request-proxy-available="account.provider === 'openai'"
        :disabled="saving"
        :endpoint="account.outboundProxyEndpoint"
        :account-id="account.id"
      />
    </div>

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving" @click="open = false">
        取消未保存更改
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="!account || groupsLoading"
        @click="emit('save')"
      >
        保存账号设置
      </BaseButton>
    </template>
  </BaseModal>
</template>
