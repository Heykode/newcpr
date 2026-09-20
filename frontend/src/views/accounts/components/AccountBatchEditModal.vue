<script setup lang="ts">
import type { AccountGroup } from '@/api'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  selectedCount: number
  groups: AccountGroup[]
  groupsLoading: boolean
  saving: boolean
  hasUpdates: boolean
  turnStateAvailable: boolean
}>()

const emit = defineEmits<{
  save: []
}>()

const open = defineModel<boolean>({ required: true })
const customName = defineModel<string>('customName', { required: true })
const updateCustomName = defineModel<boolean>('updateCustomName', { required: true })
const enabled = defineModel<boolean>('enabled', { required: true })
const turnStateInjectionEnabled = defineModel<boolean>('turnStateInjectionEnabled', { required: true })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const updateEnabled = defineModel<boolean>('updateEnabled', { required: true })
const updateTurnStateInjectionEnabled = defineModel<boolean>('updateTurnStateInjectionEnabled', { required: true })
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
      v-model:turn-state-injection-enabled="turnStateInjectionEnabled"
      v-model:concurrency-limit="concurrencyLimit"
      v-model:weight="weight"
      v-model:selected-group-ids="selectedGroupIds"
      v-model:proxy-mode="proxyMode"
      v-model:proxy-id="proxyId"
      v-model:update-enabled="updateEnabled"
      v-model:update-turn-state-injection-enabled="updateTurnStateInjectionEnabled"
      v-model:update-concurrency-limit="updateConcurrencyLimit"
      v-model:update-weight="updateWeight"
      v-model:update-groups="updateGroups"
      v-model:update-proxy="updateProxy"
      name-available
      :groups="groups"
      :groups-loading="groupsLoading"
      :turn-state-available="turnStateAvailable"
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
