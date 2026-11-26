<script setup lang="ts">
import type { PrivacyStep } from '@/api'

import { CircleHelp } from '@lucide/vue'
import { computed, useId, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { actions, fieldsByScope, scopes } from './presets'

const { t } = useI18n({ useScope: 'global' })

const step = defineModel<PrivacyStep>({ required: true })
const fixedValue = defineModel<string>('fixedValue', { required: true })
const fieldPresetId = useId()
const anonymizesProject = computed(() => step.value.action === 'anonymize_project')
const usesPattern = computed(() => ['regex_replace', 'rename_key'].includes(step.value.action) || (step.value.action === 'remove_field' && step.value.pattern !== null))
const pattern = computed({
  get: () => step.value.pattern ?? '',
  set: (value: string) => {
    step.value.pattern = value
  },
})
const conditional = computed({
  get: () => step.value.pattern !== null,
  set: (value: boolean) => {
    step.value.pattern = value ? '' : null
  },
})
const fields = computed(() => fieldsByScope.value[step.value.scope] ?? [])
const fieldChoice = computed({
  get: () => fields.value.some(field => field.value === step.value.selector)
    ? step.value.selector
    : '',
  set: (value: string) => {
    step.value.selector = value
  },
})
const selectorPlaceholder = computed(() => ({
  turn_metadata: t('settings.privacy.steps.pathPlaceholder'),
  desktop_git_context: t('settings.privacy.steps.forExampleRemotesFetchurl'),
  environment_text: t('settings.privacy.steps.environmentPathHelp'),
  request_body: t('settings.privacy.steps.forExampleMetadataWorkspace'),
  request_header: t('settings.privacy.steps.forExampleXClientName'),
})[step.value.scope])

watch(() => step.value.action, (action) => {
  if (action === 'anonymize_project') {
    step.value.scope = 'turn_metadata'
    step.value.selector = '$.workspaces'
    step.value.pattern = null
  }
  else if (action === 'set_value') {
    step.value.pattern = null
  }
  else if (action !== 'remove_field' && step.value.pattern === null) {
    step.value.pattern = ''
  }
})
</script>

<template>
  <BaseForm class="sm:grid-cols-2">
    <BaseFormItem class="md:[&>div:first-child]:min-h-8" :label="t('settings.privacy.rules.scope')">
      <BaseSelect v-model="step.scope" class="w-full" :options="scopes" :disabled="anonymizesProject" :aria-label="t('settings.privacy.rules.scope')" />
    </BaseFormItem>
    <BaseFormItem class="md:[&>div:first-child]:min-h-8" :label="t('settings.privacy.rules.processingMethod')">
      <BaseSelect v-model="step.action" class="w-full" :options="actions" :aria-label="t('settings.privacy.rules.processingMethod')" />
    </BaseFormItem>
    <BaseFormItem class="sm:col-span-2" :label="step.scope === 'request_header' ? t('settings.privacy.steps.requestHeaderName') : t('settings.privacy.steps.fieldPath')" required>
      <template #label-extra>
        <BasePopover trigger="hover-click">
          <template #trigger="{ open: helpOpen }">
            <BaseIconButton variant="link" size="sm" :label="t('settings.privacy.steps.fieldPathSyntax')" class="-my-2" :aria-expanded="helpOpen">
              <CircleHelp class="size-3.5" />
            </BaseIconButton>
          </template>
          <div class="max-w-72 p-3 text-cp-sm text-cp-text-secondary">
            {{ t('settings.privacy.steps.jsonPathHelp') }}<br>{{ t('settings.privacy.steps.targetHelp') }}
          </div>
        </BasePopover>
      </template>
      <div class="flex flex-wrap items-center gap-2 sm:flex-nowrap">
        <BaseInput v-model="step.selector" :disabled="anonymizesProject" :placeholder="selectorPlaceholder" :aria-label="t('settings.privacy.steps.fieldPath')" maxlength="1024" class="min-w-0 flex-1 font-mono" :class="fields.length ? 'basis-full sm:basis-auto' : undefined" spellcheck="false" />
        <BaseSelect v-if="fields.length" :id="fieldPresetId" v-model="fieldChoice" :disabled="anonymizesProject" class="min-w-0 flex-1 sm:w-40 sm:flex-none" :options="fields" :aria-label="t('settings.privacy.steps.commonFields')" />
      </div>
    </BaseFormItem>
    <BaseCheckbox v-if="step.action === 'remove_field'" v-model="conditional" show-label class="sm:col-span-2" :label="t('settings.privacy.steps.deleteMatchHelp')" />
    <BaseFormItem v-if="usesPattern" :class="step.action === 'remove_field' ? 'sm:col-span-2' : undefined" :label="t('settings.privacy.steps.matchExpression')">
      <BaseInput v-model="pattern" :placeholder="step.action === 'rename_key' ? t('settings.privacy.steps.forExampleHome') : t('settings.privacy.steps.forExamplePrivateOrg')" :aria-label="t('settings.privacy.steps.matchExpression')" maxlength="4096" spellcheck="false" class="font-mono" />
    </BaseFormItem>
    <BaseFormItem v-if="step.action === 'regex_replace' || step.action === 'rename_key'" :label="t('settings.privacy.steps.replaceContent')">
      <template #label-extra>
        <BasePopover trigger="hover-click">
          <template #trigger="{ open: helpOpen }">
            <BaseIconButton variant="link" size="sm" :label="t('settings.privacy.steps.regularSyntaxDescription')" class="-my-2" :aria-expanded="helpOpen">
              <CircleHelp class="size-3.5" />
            </BaseIconButton>
          </template>
          <div class="max-w-72 p-3 text-cp-sm text-cp-text-secondary">
            {{ t('settings.privacy.steps.useOrForCaptureGroupsAndFor') }}<br>{{ t('settings.privacy.steps.usesRustRegexSyntaxAndDoesNot') }}
          </div>
        </BasePopover>
      </template>
      <BaseInput v-model="step.replacement" :placeholder="step.action === 'rename_key' ? t('settings.privacy.steps.forExampleHomeUser') : t('settings.privacy.steps.forExampleTeamOrLeaveBlankTo')" :aria-label="t('settings.privacy.steps.replaceContent')" maxlength="4096" spellcheck="false" class="font-mono" />
    </BaseFormItem>
    <BaseFormItem v-if="step.action === 'set_value'" class="sm:col-span-2" :label="t('settings.privacy.steps.fixedValueJson')">
      <BaseTextarea v-model="fixedValue" :placeholder="t('settings.privacy.steps.forExampleAnonymousNullOr')" :rows="2" resize="none" maxlength="4096" :aria-label="t('settings.privacy.steps.fixedJsonLabel')" class="font-mono" />
    </BaseFormItem>
    <div v-if="usesPattern" class="flex flex-wrap gap-x-4 gap-y-2 sm:col-span-2">
      <BaseCheckbox v-if="step.action !== 'remove_field'" v-model="step.replaceAll" show-label :label="t('settings.privacy.steps.replaceAllMatches')" />
      <BaseCheckbox v-model="step.caseInsensitive" show-label :label="t('settings.privacy.steps.ignoreCase')" />
      <BaseCheckbox v-model="step.multiLine" show-label :label="t('settings.privacy.steps.matchMultipleLines')" />
    </div>
  </BaseForm>
</template>
