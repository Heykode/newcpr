<script setup lang="ts">
import type { AccountGroup, AccountModelAccess } from '@/api'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import ExcelModelFields from '@/components/ExcelModelFields.vue'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
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
  excelAvailable?: boolean
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
const excelAutoDisableOn403 = defineModel<boolean>('excelAutoDisableOn403', { default: false })
const updateExcelCacheCreationAsInput = defineModel<boolean>('updateExcelCacheCreationAsInput', { default: false })
const updateExcelAutoDisableOn403 = defineModel<boolean>('updateExcelAutoDisableOn403', { default: false })
const updateExcelModels = defineModel<boolean>('updateExcelModels', { default: false })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const weight = defineModel<string>('weight', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess')
const updateModelAccess = defineModel<boolean>('updateModelAccess', { default: false })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
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

    <BaseFormItem v-if="excelAvailable" label="Excel 遇到 HTTP 403 自动关闭">
      <template v-if="batch" #extra>
        <BaseCheckbox
          v-model="updateExcelAutoDisableOn403"
          label="应用 Excel 403 自动关闭更改"
          :disabled="disabled"
        />
      </template>
      <BaseSwitch
        v-model="excelAutoDisableOn403"
        label="Excel 遇到 HTTP 403 自动关闭"
        title="仅上游 HTTP 403 触发，模型权限变更除外；不重发当前请求"
        :disabled="disabled || (batch ? !updateExcelAutoDisableOn403 : !excelEnabled)"
      />
    </BaseFormItem>

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
