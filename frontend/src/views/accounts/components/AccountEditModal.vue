<script setup lang="ts">
import type { AccountRow } from '../constants'
import type { AccountGroup, AccountModelAccess } from '@/api'

import type { AccountEgressReadState } from '@/utils/account-egress'
import { useTemplateRef } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseModal from '@/components/base/BaseModal/index.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { t } from '@/i18n'
import AccountIdentityCell from './AccountIdentityCell.vue'
import AccountPlanBadge from './AccountPlanBadge.vue'
import AccountPurchaseFields from './AccountPurchaseFields.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  account: AccountRow | null
  groups: AccountGroup[]
  groupsLoading: boolean
  saving: boolean
  egressReadState: AccountEgressReadState
}>()

const emit = defineEmits<{
  save: []
}>()

const open = defineModel<boolean>({ required: true })
const customName = defineModel<string>('customName', { required: true })
const purchaseAmount = defineModel<string>('purchaseAmount', { default: '' })
const purchaseCycleStart = defineModel<string>('purchaseCycleStart', { default: '' })
const enabled = defineModel<boolean>('enabled', { required: true })
const requestProxySource = defineModel<import('@/utils/request-proxy-source').RequestProxySource>('requestProxySource', { default: 'account' })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const credentialProxyId = defineModel<string | null>('credentialProxyId')
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const egressMode = defineModel<string>('egressMode', { required: true })
const settingsRef = useTemplateRef<InstanceType<typeof AccountSettingsFields>>('settingsFields')
async function requestSave() {
  if (await settingsRef.value?.validate())
    emit('save')
}
</script>

<template>
  <BaseModal
    v-model="open"
    :title="t('accounts.editor.title')"
    description="查看账号信息，并调整调度与所属分组。"
    size="lg"
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
        ref="settingsFields"
        v-model:custom-name="customName"
        v-model:enabled="enabled"
        v-model:request-proxy-source="requestProxySource"
        v-model:concurrency-limit="concurrencyLimit"
        v-model:weight="weight"
        v-model:model-access="modelAccess"
        v-model:selected-group-ids="selectedGroupIds"
        v-model:proxy-mode="proxyMode"
        v-model:proxy-id="proxyId"
        v-model:credential-proxy-id="credentialProxyId"
        v-model:egress-mode="egressMode"
        :provider="account.provider"
        :credential-egress="account.provider === 'openai'"
        name-available
        model-access-available
        :groups="groups"
        :groups-loading="groupsLoading"
        :request-proxy-available="account.provider === 'openai'"
        :preserve-proxy="false"
        :egress-read-state="egressReadState"
        :disabled="saving"
        :endpoint="account.outboundProxyEndpoint"
        :current-proxy-id="account.outboundProxyId"
        :account-id="account.id"
      />
      <AccountPurchaseFields v-model:amount="purchaseAmount" v-model:cycle-start="purchaseCycleStart" :disabled="saving || !account.purchaseCost" />
    </div>

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving" @click="open = false">
        取消未保存更改
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="!account || groupsLoading"
        @click="requestSave"
      >
        保存账号设置
      </BaseButton>
    </template>
  </BaseModal>
</template>
