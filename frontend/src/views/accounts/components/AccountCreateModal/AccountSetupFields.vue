<script setup lang="ts">
import type { AccountCreateForm } from './model'
import type { AccountGroup } from '@/api'
import AccountTemplatePicker from '@/components/account-templates/AccountTemplatePicker.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import AccountPurchaseFields from '../AccountPurchaseFields.vue'
import AccountSettingsFields from '../AccountSettingsFields.vue'
import AccountProviderChooser from './AccountProviderChooser.vue'
import { prefillAccountTemplate } from './template'

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
    <AccountTemplatePicker
      :model-value="form.importTemplate"
      label="导入账号模板"
      manage
      :summary="false"
      :disabled="disabled"
      @update:model-value="form = prefillAccountTemplate(form, $event)"
    />
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
      v-model:excel-ignore-encrypted-content="form.excelIgnoreEncryptedContent"
      v-model:excel-403-action="form.excel403Action"
      v-model:excel-recovery-enabled="form.excelRecoveryEnabled"
      v-model:excel-recovery-interval="form.excelRecoveryInterval"
      v-model:request-proxy-source="form.requestProxySource"
      v-model:concurrency-limit="form.concurrencyLimit"
      v-model:weight="form.weight"
      v-model:model-access="form.modelAccess"
      v-model:selected-group-ids="form.groupIds"
      v-model:proxy-mode="form.proxyMode"
      v-model:proxy-id="form.proxyId"
      v-model:egress-mode="form.egressMode"
      :excel-available="form.applyExcel && (form.provider === 'openai' || form.provider === 'batch')"
      encrypted-content-available
      :request-proxy-available="form.provider === 'openai' || form.provider === 'batch'"
      name-available
      model-access-available
      preserve-model-access
      :groups="groups"
      :groups-loading="groupsLoading"
      :preserve-proxy="form.importTemplate !== null || form.proxyMode === 'preserve'"
      :disabled="disabled"
      :proxy-error="proxyError"
    />
    <AccountPurchaseFields v-model:amount="form.purchaseAmount" v-model:cycle-start="form.purchaseCycleStart" importing :disabled="disabled" />
  </div>
</template>
