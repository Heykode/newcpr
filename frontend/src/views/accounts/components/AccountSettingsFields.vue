<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'
import type { AccountEgressReadState } from '@/utils/account-egress'

import type { Excel403Action } from '@/utils/excel-settings'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import Excel403ActionSelect from '@/components/Excel403ActionSelect.vue'
import ExcelModelFields from '@/components/ExcelModelFields.vue'
import ExcelRecoveryFields from '@/components/ExcelRecoveryFields.vue'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import AccountModelAccessField from './AccountModelAccessField.vue'
import AccountOutboundField from './AccountOutboundField.vue'

withDefaults(defineProps<{
  groups: AccountGroup[]
  groupsLoading: boolean
  disabled: boolean
  batch?: boolean
  endpoint?: string | null
  accountId?: string
  preserveProxy?: boolean
  egressReadState?: AccountEgressReadState
  requestProxyAvailable?: boolean
  proxyError?: string
  excelAvailable?: boolean
  encryptedContentAvailable?: boolean
  nameAvailable?: boolean
  modelAccessAvailable?: boolean
  preserveModelAccess?: boolean
}>(), { preserveProxy: true, batch: false, excelAvailable: false, nameAvailable: false, modelAccessAvailable: false, preserveModelAccess: false })

const customName = defineModel<string>('customName', { default: '' })
const updateCustomName = defineModel<boolean>('updateCustomName', { default: false })
const enabled = defineModel<boolean>('enabled', { required: true })
const excelEnabled = defineModel<boolean>('excelEnabled', { default: false })
const excelModels = defineModel<string>('excelModels', { default: DEFAULT_EXCEL_MODELS_INPUT })
const excelModelsFollowGlobal = defineModel<boolean>('excelModelsFollowGlobal', { default: true })
const excelCacheCreationAsInput = defineModel<boolean>('excelCacheCreationAsInput', { default: false })
const excelIgnoreEncryptedContent = defineModel<boolean>('excelIgnoreEncryptedContent', { default: false })
const requestProxySource = defineModel<import('@/utils/request-proxy-source').RequestProxySource>('requestProxySource', { default: 'account' })
const updateExcelIgnoreEncryptedContent = defineModel<boolean>('updateExcelIgnoreEncryptedContent', { default: false })
const excel403Action = defineModel<Excel403Action>('excel403Action', { default: 'none' })
const excelRecoveryEnabled = defineModel<boolean>('excelRecoveryEnabled', { default: false })
const excelRecoveryInterval = defineModel<string>('excelRecoveryInterval', { default: '60' })
const updateExcelRecovery = defineModel<boolean>('updateExcelRecovery', { default: false })
const updateExcelCacheCreationAsInput = defineModel<boolean>('updateExcelCacheCreationAsInput', { default: false })
const updateExcel403Action = defineModel<boolean>('updateExcel403Action', { default: false })
const updateExcelModels = defineModel<boolean>('updateExcelModels', { default: false })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess')
const updateModelAccess = defineModel<boolean>('updateModelAccess', { default: false })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const egressMode = defineModel<string>('egressMode', { default: 'fixed_ipv6_reuse' })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
const updateEnabled = defineModel<boolean>('updateEnabled', { default: false })
const updateExcelEnabled = defineModel<boolean>('updateExcelEnabled', { default: false })
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
      v-if="excelAvailable"
      :class="batch ? 'grid gap-2' : 'flex min-h-6 items-center justify-between gap-3'"
    >
      <div class="flex min-w-0 items-center justify-between gap-3">
        <span class="text-cp leading-none font-medium text-cp-text-secondary">Excel 入口</span>
        <BaseCheckbox
          v-if="batch"
          v-model="updateExcelEnabled"
          label="应用 Excel 入口更改"
          title="应用 Excel 入口更改"
          :disabled="disabled"
        />
      </div>
      <BaseSwitch
        v-model="excelEnabled"
        label="切换 Excel 入口"
        :disabled="disabled || (batch && !updateExcelEnabled)"
      />
    </div>

    <BaseFormItem v-if="excelAvailable" label="Excel 模型">
      <template v-if="batch" #extra>
        <BaseCheckbox v-model="updateExcelModels" label="应用 Excel 模型更改" title="应用 Excel 模型更改" :disabled="disabled" />
      </template>
      <ExcelModelFields
        v-model:models="excelModels"
        v-model:follow-global="excelModelsFollowGlobal"
        :disabled="disabled || (batch && !updateExcelModels)"
      />
    </BaseFormItem>

    <BaseFormItem v-if="excelAvailable" label="Excel 缓存写入按普通输入计费">
      <template v-if="batch" #extra>
        <BaseCheckbox
          v-model="updateExcelCacheCreationAsInput"
          label="应用缓存写入计费更改"
          :disabled="disabled"
        />
      </template>
      <BaseSwitch
        v-model="excelCacheCreationAsInput"
        label="缓存写入按普通输入计费"
        :disabled="disabled || (batch ? !updateExcelCacheCreationAsInput : !excelEnabled)"
      />
    </BaseFormItem>

    <BaseFormItem v-if="excelAvailable && encryptedContentAvailable" label="忽略历史中的加密消息内容">
      <template v-if="batch" #extra>
        <BaseCheckbox
          v-model="updateExcelIgnoreEncryptedContent"
          label="应用加密消息省略更改"
          :disabled="disabled"
        />
      </template>
      <BaseSwitch
        v-model="excelIgnoreEncryptedContent"
        label="忽略历史中的加密消息内容"
        :disabled="disabled || (batch ? !updateExcelIgnoreEncryptedContent || (updateExcelEnabled && !excelEnabled) : !excelEnabled)"
      />
      <p class="m-0 mt-2 text-cp-xs leading-relaxed text-cp-text-secondary">
        默认关闭。仅实际走 Excel 时，将无法转发的历史加密消息替换为省略提示；不会解密，模型将看不到这些内容。无此类内容时不修改，原生 Codex 请求不受影响。
      </p>
    </BaseFormItem>

    <BaseFormItem v-if="excelAvailable" label="Excel遇到HTTP 403">
      <template v-if="batch" #extra>
        <BaseCheckbox
          v-model="updateExcel403Action"
          label="应用 Excel 403 处理方式更改"
          :disabled="disabled"
        />
      </template>
      <Excel403ActionSelect
        v-model="excel403Action"
        :disabled="disabled || (batch ? !updateExcel403Action : !excelEnabled)"
      />
    </BaseFormItem>

    <div v-if="excelAvailable" class="grid gap-3">
      <BaseCheckbox v-if="batch" v-model="updateExcelRecovery" label="应用 Excel 恢复探测更改" :disabled="disabled" />
      <ExcelRecoveryFields
        v-model:enabled="excelRecoveryEnabled"
        v-model:interval="excelRecoveryInterval"
        :disabled="disabled || (batch && !updateExcelRecovery)"
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
    <AccountOutboundField
      v-model:mode="proxyMode"
      v-model:proxy-id="proxyId"
      v-model:egress-mode="egressMode"
      :openai="requestProxyAvailable"
      :current-source="requestProxySource"
      :preserve="preserveProxy"
      :egress-read-state="egressReadState"
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
    </AccountOutboundField>
  </div>
</template>
