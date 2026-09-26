<script setup lang="ts">
import type { AccountCreateForm } from './model'
import type { AccountGroup } from '@/api'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import AccountSettingsFields from '../AccountSettingsFields.vue'
import AccountProviderChooser from './AccountProviderChooser.vue'

defineProps<{
  groups: AccountGroup[]
  groupsLoading: boolean
  disabled: boolean
  proxyError?: string
}>()
const form = defineModel<AccountCreateForm>({ required: true })
</script>

<template>
  <div class="grid gap-6">
    <fieldset class="m-0 min-w-0 border-0 p-0">
      <legend class="mb-3 p-0 text-cp font-medium text-cp-text-secondary">
        账号平台
      </legend>
      <AccountProviderChooser :selected="form.provider" :disabled="disabled" @select="form.provider = $event" />
    </fieldset>
    <BaseCheckbox
      v-if="form.provider === 'openai' || form.provider === 'batch'"
      v-model="form.applyExcel"
      label="指定 Excel 设置"
      show-label
      :disabled="disabled"
    />
    <AccountSettingsFields
      v-model:custom-name="form.customName"
      v-model:enabled="form.enabled"
      v-model:excel-enabled="form.excelEnabled"
      v-model:excel-models="form.excelModels"
      v-model:excel-models-follow-global="form.excelModelsFollowGlobal"
      v-model:excel-cache-creation-as-input="form.excelCacheCreationAsInput"
      v-model:excel-auto-disable-on-403="form.excelAutoDisableOn403"
      v-model:concurrency-limit="form.concurrencyLimit"
      v-model:weight="form.weight"
      v-model:model-access="form.modelAccess"
      v-model:selected-group-ids="form.groupIds"
      v-model:proxy-mode="form.proxyMode"
      v-model:proxy-id="form.proxyId"
      :excel-available="form.applyExcel && (form.provider === 'openai' || form.provider === 'batch')"
      name-available
      model-access-available
      preserve-model-access
      :groups="groups"
      :groups-loading="groupsLoading"
      :preserve-proxy="false"
      :disabled="disabled"
      :proxy-error="proxyError"
    />
  </div>
</template>
