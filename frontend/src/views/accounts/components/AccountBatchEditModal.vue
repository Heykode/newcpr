<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  selectedCount: number
  groups: AccountGroup[]
  groupsLoading: boolean
  saving: boolean
  hasUpdates: boolean
  excelAvailable: boolean
  catalogAccountId?: string
}>()

const emit = defineEmits<{
  save: []
}>()

const open = defineModel<boolean>({ required: true })
const customName = defineModel<string>('customName', { required: true })
const updateCustomName = defineModel<boolean>('updateCustomName', { required: true })
const enabled = defineModel<boolean>('enabled', { required: true })
const excelEnabled = defineModel<boolean>('excelEnabled', { required: true })
const excelModels = defineModel<string>('excelModels', { required: true })
const excelModelsFollowGlobal = defineModel<boolean>('excelModelsFollowGlobal', { default: true })
const excelCacheCreationAsInput = defineModel<boolean>('excelCacheCreationAsInput', { default: false })
const excelAutoDisableOn403 = defineModel<boolean>('excelAutoDisableOn403', { default: false })
const updateExcelCacheCreationAsInput = defineModel<boolean>('updateExcelCacheCreationAsInput', { default: false })
const updateExcelAutoDisableOn403 = defineModel<boolean>('updateExcelAutoDisableOn403', { default: false })
const updateExcelModels = defineModel<boolean>('updateExcelModels', { required: true })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess', { required: true })
const updateModelAccess = defineModel<boolean>('updateModelAccess', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const updateEnabled = defineModel<boolean>('updateEnabled', { required: true })
const updateExcelEnabled = defineModel<boolean>('updateExcelEnabled', { required: true })
const updateConcurrencyLimit = defineModel<boolean>('updateConcurrencyLimit', { required: true })
const updateWeight = defineModel<boolean>('updateWeight', { required: true })
const updateGroups = defineModel<boolean>('updateGroups', { required: true })
const updateProxy = defineModel<boolean>('updateProxy', { required: true })
</script>

<template>
  <BaseModal
    v-model="open"
    title="批量编辑账号"
    :description="`已选择 ${selectedCount} 个账号`"
    size="md"
    :dismissible="!saving"
  >
    <AccountSettingsFields
      v-model:custom-name="customName"
      v-model:update-custom-name="updateCustomName"
      v-model:enabled="enabled"
      v-model:excel-enabled="excelEnabled"
      v-model:excel-models="excelModels"
      v-model:excel-models-follow-global="excelModelsFollowGlobal"
      v-model:excel-cache-creation-as-input="excelCacheCreationAsInput"
      v-model:excel-auto-disable-on-403="excelAutoDisableOn403"
      v-model:update-excel-cache-creation-as-input="updateExcelCacheCreationAsInput"
      v-model:update-excel-auto-disable-on-403="updateExcelAutoDisableOn403"
      v-model:update-excel-models="updateExcelModels"
      v-model:concurrency-limit="concurrencyLimit"
      v-model:weight="weight"
      v-model:model-access="modelAccess"
      v-model:update-model-access="updateModelAccess"
      v-model:selected-group-ids="selectedGroupIds"
      v-model:proxy-mode="proxyMode"
      v-model:proxy-id="proxyId"
      v-model:update-enabled="updateEnabled"
      v-model:update-excel-enabled="updateExcelEnabled"
      v-model:update-concurrency-limit="updateConcurrencyLimit"
      v-model:update-weight="updateWeight"
      v-model:update-groups="updateGroups"
      v-model:update-proxy="updateProxy"
      name-available
      model-access-available
      :account-id="catalogAccountId"
      :groups="groups"
      :groups-loading="groupsLoading"
      :excel-available="excelAvailable"
      :disabled="saving"
      batch
    />

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving" @click="open = false">
        取消
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
