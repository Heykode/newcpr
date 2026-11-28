<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'

import BaseButton from '@/components/base/BaseButton.vue'

import BaseModal from '@/components/base/BaseModal/index.vue'
import { t } from '@/i18n'
import AccountPurchaseFields from './AccountPurchaseFields.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  selectedCount: number
  groups: AccountGroup[]
  groupsLoading: boolean
  saving: boolean
  hasUpdates: boolean
  requestProxyAvailable: boolean
  provider?: string
  catalogAccountId?: string
}>()

const emit = defineEmits<{
  save: []
}>()

const open = defineModel<boolean>({ required: true })
const customName = defineModel<string>('customName', { required: true })
const purchaseAmount = defineModel<string>('purchaseAmount', { default: '' })
const purchaseCycleStart = defineModel<string>('purchaseCycleStart', { default: '' })
const updatePurchaseCost = defineModel<boolean>('updatePurchaseCost', { default: false })
const updateCustomName = defineModel<boolean>('updateCustomName', { required: true })
const enabled = defineModel<boolean>('enabled', { required: true })
const requestProxySource = defineModel<import('@/utils/request-proxy-source').RequestProxySource>('requestProxySource', { default: 'account' })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess', { required: true })
const updateModelAccess = defineModel<boolean>('updateModelAccess', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const updateEnabled = defineModel<boolean>('updateEnabled', { required: true })
const updateConcurrencyLimit = defineModel<boolean>('updateConcurrencyLimit', { required: true })
const updateWeight = defineModel<boolean>('updateWeight', { required: true })
const updateGroups = defineModel<boolean>('updateGroups', { required: true })
const updateProxy = defineModel<boolean>('updateProxy', { required: true })
const egressMode = defineModel<string>('egressMode', { required: true })
</script>

<template>
  <BaseModal
    v-model="open"
    :title="t('accounts.batchEdit.title')"
    :description="`已选择 ${selectedCount} 个账号`"
    size="md"
    :dismissible="!saving"
  >
    <AccountSettingsFields
      v-model:custom-name="customName"
      v-model:update-custom-name="updateCustomName"
      v-model:enabled="enabled"
      v-model:request-proxy-source="requestProxySource"
      v-model:concurrency-limit="concurrencyLimit"
      v-model:weight="weight"
      v-model:model-access="modelAccess"
      v-model:update-model-access="updateModelAccess"
      v-model:selected-group-ids="selectedGroupIds"
      v-model:proxy-mode="proxyMode"
      v-model:proxy-id="proxyId"
      v-model:egress-mode="egressMode"
      v-model:update-enabled="updateEnabled"
      v-model:update-concurrency-limit="updateConcurrencyLimit"
      v-model:update-weight="updateWeight"
      v-model:update-groups="updateGroups"
      v-model:update-proxy="updateProxy"
      :provider="provider"
      name-available
      model-access-available
      :model-account-id="catalogAccountId"
      :groups="groups"
      :groups-loading="groupsLoading"
      :request-proxy-available="requestProxyAvailable"
      :disabled="saving"
      batch
    />

    <AccountPurchaseFields v-model:amount="purchaseAmount" v-model:cycle-start="purchaseCycleStart" v-model:apply="updatePurchaseCost" class="mt-5" batch :disabled="saving" />

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="saving || selectedCount === 0 || !hasUpdates || (updateGroups && groupsLoading)"
        @click="emit('save')"
      >
        保存更改
      </BaseButton>
    </template>
  </BaseModal>
</template>
