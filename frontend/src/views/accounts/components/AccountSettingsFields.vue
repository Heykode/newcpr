<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'
import type { FormRules } from '@/components/base/BaseForm/types'
import type { AccountEgressReadState } from '@/utils/account-egress'
import { computed, useTemplateRef } from 'vue'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'

import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { t } from '@/i18n'
import { normalizeAccountName } from '@/utils/account-name'
import { accountModelAccessError, commonImageModels } from '../utils/modelAccess'
import AccountModelAccessField from './AccountModelAccessField.vue'
import AccountOutboundField from './AccountOutboundField.vue'
import CredentialEgressField from './CredentialEgressField.vue'

const props = withDefaults(defineProps<{
  groups: AccountGroup[]
  groupsLoading: boolean
  disabled: boolean
  batch?: boolean
  endpoint?: string | null
  currentProxyId?: string | null
  provider?: string
  accountId?: string
  modelAccountId?: string
  preserveProxy?: boolean
  egressReadState?: AccountEgressReadState
  requestProxyAvailable?: boolean
  proxyError?: string
  nameAvailable?: boolean
  knownModelCatalog?: boolean
  modelAccessAvailable?: boolean
  preserveModelAccess?: boolean
  credentialEgress?: boolean
  templateEgress?: boolean
}>(), { preserveProxy: true, batch: false, nameAvailable: false, modelAccessAvailable: false, preserveModelAccess: false })

const customName = defineModel<string>('customName', { default: '' })
const updateCustomName = defineModel<boolean>('updateCustomName', { default: false })
const enabled = defineModel<boolean>('enabled', { required: true })
const requestProxySource = defineModel<import('@/utils/request-proxy-source').RequestProxySource>('requestProxySource', { default: 'account' })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess')
const updateModelAccess = defineModel<boolean>('updateModelAccess', { default: false })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const credentialProxyId = defineModel<string | null>('credentialProxyId')
const egressMode = defineModel<string>('egressMode', { default: 'fixed_ipv6_reuse' })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const updateEnabled = defineModel<boolean>('updateEnabled', { default: false })
const updateConcurrencyLimit = defineModel<boolean>('updateConcurrencyLimit', { default: false })
const updateWeight = defineModel<boolean>('updateWeight', { default: false })
const updateGroups = defineModel<boolean>('updateGroups', { default: false })
const updateProxy = defineModel<boolean>('updateProxy', { default: false })
const formRef = useTemplateRef<InstanceType<typeof BaseForm>>('formElement')
const form = computed(() => ({ customName: customName.value, concurrencyLimit: concurrencyLimit.value, weight: weight.value, modelAccess: modelAccess.value, proxyId: proxyId.value }))
const rules = computed<FormRules>(() => props.batch
  ? {}
  : ({
      customName: { trigger: ['blur', 'change'], validator: () => {
        try {
          normalizeAccountName(customName.value)
          return true
        }
        catch (cause) { return cause instanceof Error ? cause : new Error('账号名称无效') }
      } },
      concurrencyLimit: { type: 'integer', transform: value => String(value ?? '').trim() ? Number(value) : undefined, min: 1, max: 4_294_967_295, trigger: ['blur', 'change'], message: '并发上限须为 1–4294967295 的整数，留空跟随全局' },
      weight: [
        { required: true, transform: value => String(value ?? '').trim() || undefined, message: '请填写权重', trigger: ['blur', 'change'] },
        { type: 'integer', transform: value => String(value ?? '').trim() ? Number(value) : undefined, min: 1, max: 100, message: '权重须为 1–100 的整数', trigger: ['blur', 'change'] },
      ],
      proxyId: { required: proxyMode.value === 'proxy', whitespace: true, message: '请选择指定代理', trigger: 'change' },
      modelAccess: { validator: () => {
        const error = accountModelAccessError(modelAccess.value)
        return error ? new Error(error) : true
      }, trigger: [] },
    }))
defineExpose({ validate: () => formRef.value?.validate().catch(() => false) })
</script>

<template>
  <BaseForm ref="formElement" :model="form" :rules="rules" :validate-on-rule-change="false" scroll-to-error focus-on-error novalidate class="grid gap-5">
    <BaseFormItem v-if="nameAvailable" prop="customName" label="自定义账号名称（选填）">
      <template v-if="batch" #extra>
        <BaseCheckbox v-model="updateCustomName" label="应用账号名称更改" title="应用账号名称更改" :disabled="disabled" />
      </template>
      <BaseInput
        v-model="customName"
        aria-label="自定义账号名称"
        placeholder="默认名称"
        :disabled="disabled || (batch && !updateCustomName)"
      />
    </BaseFormItem>
    <div :class="batch ? 'grid gap-2' : 'flex min-h-6 items-center justify-between gap-3'">
      <div class="flex min-w-0 items-center justify-between gap-3">
        <span class="text-cp leading-none font-medium text-cp-text-secondary">{{ t('accounts.scheduling.scheduling') }}</span>
        <BaseCheckbox
          v-if="batch"
          v-model="updateEnabled"
          label="应用启用状态更改"
          title="应用启用状态更改"
          :disabled="disabled"
        />
      </div>
      <BaseSwitch
        v-model="enabled"
        :label="t('accounts.scheduling.switchAccountSchedule')"
        :disabled="disabled || (batch && !updateEnabled)"
      />
    </div>

    <div class="grid gap-4 sm:grid-cols-2">
      <BaseFormItem prop="concurrencyLimit" :label="t('accounts.scheduling.concurrencyLimit')">
        <template v-if="batch" #extra>
          <BaseCheckbox
            v-model="updateConcurrencyLimit"
            label="应用并发限制更改"
            title="应用并发限制更改"
            :disabled="disabled"
          />
        </template>
        <BaseInput
          v-model="concurrencyLimit"
          :aria-label="t('accounts.scheduling.accountConcurrencyLimit')"
          type="number"
          min="1"
          max="4294967295"
          :placeholder="t('accounts.scheduling.defaultHint')"
          :disabled="disabled || (batch && !updateConcurrencyLimit)"
        />
      </BaseFormItem>
      <BaseFormItem prop="weight" :label="t('accounts.scheduling.weight')">
        <template v-if="batch" #extra>
          <BaseCheckbox
            v-model="updateWeight"
            label="应用调度权重更改"
            title="应用调度权重更改"
            :disabled="disabled"
          />
        </template>
        <BaseInput
          v-model="weight"
          :aria-label="t('accounts.scheduling.accountSchedulingWeight')"
          type="number"
          min="1"
          max="100"
          placeholder="越高越优先，最大 100"
          :disabled="disabled || (batch && !updateWeight)"
        />
      </BaseFormItem>
    </div>

    <AccountModelAccessField
      v-if="modelAccessAvailable"
      v-model="modelAccess"
      form-prop="modelAccess"
      :account-id="modelAccountId ?? accountId"
      :known-model-catalog="knownModelCatalog"
      :image-model-presets="provider === 'openai' ? commonImageModels : []"
      :disabled="disabled || (batch && !updateModelAccess)"
      :allow-preserve="preserveModelAccess"
    >
      <template v-if="batch" #extra>
        <BaseCheckbox v-model="updateModelAccess" label="应用模型限制更改" title="应用模型限制更改" :disabled="disabled" />
      </template>
    </AccountModelAccessField>

    <BaseFormItem :label="t('accounts.scheduling.groupLabel')">
      <template v-if="batch" #extra>
        <BaseCheckbox
          v-model="updateGroups"
          label="应用账号分组更改"
          title="应用账号分组更改"
          :disabled="disabled"
        />
      </template>
      <AccountGroupCheckboxGrid
        v-model="selectedGroupIds"
        :groups="groups"
        :loading="groupsLoading"
        :disabled="disabled || (batch && !updateGroups)"
      />
    </BaseFormItem>
    <AccountOutboundField
      v-model:mode="proxyMode"
      v-model:proxy-id="proxyId"
      v-model:egress-mode="egressMode"
      form-prop="proxyId"
      :openai="requestProxyAvailable"
      :current-source="requestProxySource"
      :preserve="preserveProxy"
      :egress-read-state="egressReadState"
      :error="proxyError"
      :endpoint="endpoint"
      :current-proxy-id="currentProxyId"
      :account-id="accountId"
      :disabled="disabled || (batch && !updateProxy)"
    >
      <template v-if="batch" #extra>
        <BaseCheckbox
          v-model="updateProxy"
          label="应用出站隧道更改"
          title="应用出站隧道更改"
          :disabled="disabled"
        />
      </template>
    </AccountOutboundField>
    <CredentialEgressField v-if="credentialEgress" v-model="credentialProxyId" :disabled="disabled" :template="templateEgress" />
  </BaseForm>
</template>
