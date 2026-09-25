<script setup lang="ts">
import type { AccountRow } from '../constants'
import type { AccountGroup, AccountModelAccess } from '@/api'
import { shallowRef } from 'vue'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import AccountEgressControl from './AccountEgressControl.vue'
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
const excelModels = defineModel<string>('excelModels', { default: 'gpt-5.6-sol, gpt-6-astra' })
const excelModelsFollowGlobal = defineModel<boolean>('excelModelsFollowGlobal', { default: true })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const egressSaving = shallowRef(false)
</script>

<template>
  <BaseModal
    v-model="open"
    title="编辑账号"
    description="查看账号信息，并调整调度与所属分组。"
    size="md"
    :dismissible="!saving && !egressSaving"
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
        v-model:concurrency-limit="concurrencyLimit"
        v-model:weight="weight"
        v-model:model-access="modelAccess"
        v-model:selected-group-ids="selectedGroupIds"
        v-model:proxy-mode="proxyMode"
        v-model:proxy-id="proxyId"
        name-available
        model-access-available
        :groups="groups"
        :groups-loading="groupsLoading"
        :excel-available="account.provider === 'openai' && account.authenticationKind === 'oauth'"
        :disabled="saving || egressSaving"
        :endpoint="account.outboundProxyEndpoint"
        :account-id="account.id"
      />
      <AccountEgressControl
        v-if="account.provider === 'openai'"
        :key="account.id"
        :account-id="account.id"
        :disabled="saving"
        @saving="egressSaving = $event"
      />
    </div>

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving || egressSaving" @click="open = false">
        取消未保存更改
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="!account || groupsLoading || egressSaving"
        @click="emit('save')"
      >
        保存账号设置
      </BaseButton>
    </template>
  </BaseModal>
</template>
