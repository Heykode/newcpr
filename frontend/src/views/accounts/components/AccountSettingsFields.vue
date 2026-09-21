<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import AccountModelAccessField from './AccountModelAccessField.vue'
import AccountProxyField from './AccountProxyField.vue'

withDefaults(defineProps<{
  groups: AccountGroup[]
  groupsLoading: boolean
  disabled: boolean
  batch?: boolean
  endpoint?: string | null
  accountId?: string
  preserveProxy?: boolean
  proxyError?: string
  turnStateAvailable?: boolean
  nameAvailable?: boolean
  modelAccessAvailable?: boolean
  preserveModelAccess?: boolean
}>(), { preserveProxy: true, batch: false, turnStateAvailable: false, nameAvailable: false, modelAccessAvailable: false, preserveModelAccess: false })

const customName = defineModel<string>('customName', { default: '' })
const updateCustomName = defineModel<boolean>('updateCustomName', { default: false })
const enabled = defineModel<boolean>('enabled', { required: true })
const turnStateInjectionEnabled = defineModel<boolean>('turnStateInjectionEnabled', { default: false })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess')
const updateModelAccess = defineModel<boolean>('updateModelAccess', { default: false })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const updateEnabled = defineModel<boolean>('updateEnabled', { default: false })
const updateTurnStateInjectionEnabled = defineModel<boolean>('updateTurnStateInjectionEnabled', { default: false })
const updateConcurrencyLimit = defineModel<boolean>('updateConcurrencyLimit', { default: false })
const updateWeight = defineModel<boolean>('updateWeight', { default: false })
const updateGroups = defineModel<boolean>('updateGroups', { default: false })
const updateProxy = defineModel<boolean>('updateProxy', { default: false })
</script>

<template>
  <div class="grid gap-5">
    <BaseFormItem v-if="nameAvailable" label="自定义账号名称（选填）">
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
        <span class="text-cp leading-none font-medium text-cp-text-secondary">调度</span>
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
        label="切换账号调度"
        :disabled="disabled || (batch && !updateEnabled)"
      />
    </div>

    <div
      v-if="turnStateAvailable"
      :class="batch ? 'grid gap-2' : 'flex min-h-6 items-center justify-between gap-3'"
    >
      <div class="flex min-w-0 items-center justify-between gap-3">
        <div>
          <span class="text-cp leading-none font-medium text-cp-text-secondary">Turn State 注入</span>
          <p class="mt-1 mb-0 text-cp-xs text-cp-text-tertiary">
            仅在全局开启且模型命中维护名单时生效
          </p>
        </div>
        <BaseCheckbox
          v-if="batch"
          v-model="updateTurnStateInjectionEnabled"
          label="应用 Turn State 注入更改"
          title="应用 Turn State 注入更改"
          :disabled="disabled"
        />
      </div>
      <BaseSwitch
        v-model="turnStateInjectionEnabled"
        label="切换 Turn State 注入"
        :disabled="disabled || (batch && !updateTurnStateInjectionEnabled)"
      />
    </div>

    <div class="grid gap-4 sm:grid-cols-2">
      <BaseFormItem label="并发限制">
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
          aria-label="账号并发限制"
          type="number"
          min="1"
          max="4294967295"
          placeholder="留空使用默认值"
          :disabled="disabled || (batch && !updateConcurrencyLimit)"
        />
      </BaseFormItem>
      <BaseFormItem label="权重">
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
          aria-label="账号调度权重"
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
      :account-id="accountId"
      :disabled="disabled || (batch && !updateModelAccess)"
      :allow-preserve="preserveModelAccess"
    >
      <template v-if="batch" #extra>
        <BaseCheckbox v-model="updateModelAccess" label="应用模型限制更改" title="应用模型限制更改" :disabled="disabled" />
      </template>
    </AccountModelAccessField>

    <BaseFormItem label="所属分组">
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
    <AccountProxyField
      v-model:mode="proxyMode"
      v-model:proxy-id="proxyId"
      :preserve="preserveProxy"
      :error="proxyError"
      :endpoint="endpoint"
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
    </AccountProxyField>
  </div>
</template>
