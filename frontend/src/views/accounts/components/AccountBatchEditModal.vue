<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'

import type { Excel403Action } from '@/utils/excel-settings'

import { ipv6EgressModes } from '@/api/modules/ipv6-egress'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  selectedCount: number
  groups: AccountGroup[]
  groupsLoading: boolean
  saving: boolean
  hasUpdates: boolean
  excelAvailable: boolean
  egressAvailable: boolean
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
const excel403Action = defineModel<Excel403Action>('excel403Action', { default: 'none' })
const updateExcelCacheCreationAsInput = defineModel<boolean>('updateExcelCacheCreationAsInput', { default: false })
const updateExcel403Action = defineModel<boolean>('updateExcel403Action', { default: false })
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
const egressMode = defineModel<string>('egressMode', { required: true })
const updateEgressMode = defineModel<boolean>('updateEgressMode', { required: true })
const egressOptions = [{ value: 'inherit', label: '继承全局策略' }, ...ipv6EgressModes.map(({ value, label }) => ({ value, label }))]
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
      v-model:excel-403-action="excel403Action"
      v-model:update-excel-cache-creation-as-input="updateExcelCacheCreationAsInput"
      v-model:update-excel-403-action="updateExcel403Action"
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

    <div class="mt-5">
      <BaseFormItem v-if="egressAvailable" label="IPv6 出口策略">
        <template #extra>
          <BaseCheckbox v-model="updateEgressMode" label="更新 IPv6 出口策略" title="更新 IPv6 出口策略" :disabled="saving" />
        </template>
        <BaseSelect v-model="egressMode" class="w-full" :options="egressOptions" :disabled="saving || !updateEgressMode" aria-label="批量 IPv6 出口策略" />
      </BaseFormItem>
      <p v-else class="m-0 text-cp-xs text-cp-text-tertiary">
        IPv6 出口策略仅支持全部选中 OpenAI 账号时批量修改。
      </p>
    </div>

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
