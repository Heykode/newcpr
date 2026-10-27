<script setup lang="ts">
import type { ApiKeyFormValue } from '../composables/useApiKeyMutations'
import type { AccountGroup } from '@/api'
import { Copy, DollarSign, KeyRound, Upload } from '@lucide/vue'
import { computed, reactive, ref, watch } from 'vue'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { t } from '@/i18n'
import { apiKeyFormErrors } from '../utils/formValidation'

const props = defineProps<{
  groups: AccountGroup[]
  groupLoading: boolean
  editing: boolean
  createdKey: string
  saving: boolean
}>()
const emit = defineEmits<{
  save: []
  copy: [text: string]
  importCcs: []
  afterLeave: []
  createdAfterLeave: []
}>()
const open = defineModel<boolean>({ default: false })
const createdOpen = defineModel<boolean>('createdOpen', { default: false })
const form = defineModel<ApiKeyFormValue>('form', { required: true })
const title = computed(() => props.editing ? '编辑 API Key' : '创建 API Key')

const touched = reactive<Partial<Record<keyof ApiKeyFormValue, boolean>>>({})
const submitted = ref(false)
const errors = computed(() => apiKeyFormErrors(form.value))
const fieldError = (key: keyof ApiKeyFormValue) => submitted.value || touched[key] ? errors.value[key] : undefined
function save() {
  submitted.value = true
  if (!Object.keys(errors.value).length)
    emit('save')
}
watch(open, () => {
  submitted.value = false
  for (const key of Object.keys(touched) as Array<keyof ApiKeyFormValue>)
    delete touched[key]
})
</script>

<template>
  <BaseModal
    v-model="open"
    :title="title"
    tone="info"
    size="md"
    :dismissible="!saving"
    @after-leave="emit('afterLeave')"
  >
    <template #icon>
      <KeyRound class="text-cp-text" :size="20" aria-hidden="true" />
    </template>

    <BaseForm class="grid gap-6">
      <BaseFormItem :error="fieldError('name')" :label="t('common.fields.name')" required>
        <BaseInput
          v-model="form.name"
          :aria-label="t('common.fields.name')"
          :placeholder="t('keys.editor.namePlaceholder')"
          :disabled="saving"
          @blur="touched.name = true"
        />
      </BaseFormItem>

      <BaseFormItem :label="t('keys.editor.labelOptional')">
        <BaseInput
          v-model="form.label"
          :aria-label="t('keys.editor.labelOptional')"
          :placeholder="t('keys.editor.forExampleBackendService')"
          :disabled="saving"
        />
      </BaseFormItem>

      <BaseFormItem :label="t('accounts.fields.accountGrouping')">
        <AccountGroupCheckboxGrid
          v-model="form.groupIds"
          :groups="groups"
          :loading="groupLoading"
          :disabled="saving"
        />
      </BaseFormItem>

      <div class="grid gap-6 sm:grid-cols-2">
        <BaseFormItem :error="fieldError('dailyLimitUsd')" :label="t('keys.editor.dailyLimit')">
          <BaseInput
            v-model="form.dailyLimitUsd"
            type="number"
            min="0"
            step="any"
            aria-label="日限额（美元）"
            placeholder="不限制"
            :disabled="saving"
            @blur="touched.dailyLimitUsd = true"
          >
            <template #prefix>
              <DollarSign class="size-4" aria-hidden="true" />
            </template>
          </BaseInput>
        </BaseFormItem>
        <BaseFormItem :error="fieldError('weeklyLimitUsd')" :label="t('keys.editor.weeklyLimit')">
          <BaseInput
            v-model="form.weeklyLimitUsd"
            type="number"
            min="0"
            step="any"
            aria-label="周限额（美元）"
            placeholder="不限制"
            :disabled="saving"
            @blur="touched.weeklyLimitUsd = true"
          >
            <template #prefix>
              <DollarSign class="size-4" aria-hidden="true" />
            </template>
          </BaseInput>
        </BaseFormItem>
      </div>

      <div class="grid gap-6 sm:grid-cols-2">
        <BaseFormItem :error="fieldError('maxConcurrency')" :label="t('keys.editor.maximumConcurrency')">
          <BaseInput
            v-model="form.maxConcurrency"
            type="number"
            :aria-label="t('keys.editor.maximumConcurrency')"
            min="0"
            step="1"
            placeholder="不限制"
            :disabled="saving"
            @blur="touched.maxConcurrency = true"
          />
        </BaseFormItem>
        <BaseFormItem :error="fieldError('requestsPerMinute')" :label="t('keys.editor.requestsPerMinuteRpm')">
          <BaseInput
            v-model="form.requestsPerMinute"
            type="number"
            :aria-label="t('keys.editor.requestsPerMinuteRpm')"
            min="0"
            step="1"
            placeholder="不限制"
            :disabled="saving"
            @blur="touched.requestsPerMinute = true"
          />
        </BaseFormItem>
      </div>
    </BaseForm>

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="!form.name.trim()"
        @click="save"
      >
        {{ editing ? '保存更改' : '创建' }}
      </BaseButton>
    </template>
  </BaseModal>

  <BaseModal
    v-model="createdOpen"
    :title="t('keys.created.apiKeyCreated')"
    description="复制密钥，或直接导入 CCSwitch"
    tone="success"
    size="md"
    @after-leave="emit('createdAfterLeave')"
  >
    <div class="flex flex-col gap-4">
      <div class="rounded-cp border border-cp-warning-border bg-cp-warning-container px-4 py-3">
        <p class="m-0 text-cp font-semibold text-cp-warning-on-container">
          {{ t('keys.created.accessNotice') }}
        </p>
      </div>
      <div>
        <p class="mb-2 text-cp font-medium text-cp-text-secondary">
          API Key
        </p>
        <div class="flex items-center gap-2">
          <code class="flex-1 rounded-cp bg-cp-fill-quaternary px-3 py-2.5 font-mono text-cp break-all text-cp-text">
            {{ createdKey }}
          </code>
          <BaseIconButton size="md" :label="t('common.actions.copy')" @click="emit('copy', createdKey)">
            <Copy class="size-4" />
          </BaseIconButton>
        </div>
      </div>
    </div>

    <template #footer>
      <BaseButton variant="secondary" @click="emit('copy', createdKey)">
        <template #icon>
          <Copy class="size-4" />
        </template>
        复制密钥
      </BaseButton>
      <BaseButton variant="secondary" @click="emit('importCcs')">
        <template #icon>
          <Upload class="size-4" />
        </template>
        导入 CCSwitch
      </BaseButton>
      <BaseButton variant="primary" @click="createdOpen = false">
        {{ t('keys.created.iHaveSaved') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
