<script setup lang="ts">
import type { AccountGroupFormValue } from '../composables/useAccountGroups'
import type { AccountGroup } from '@/api'
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseColorPicker from '@/components/base/BaseColorPicker/index.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { t } from '@/i18n'
import { ACCOUNT_GROUP_COLOR_PRESETS } from '../constants'

const props = defineProps<{
  group: AccountGroup | null
  saving: boolean
}>()
const emit = defineEmits<{
  save: []
}>()
const open = defineModel<boolean>({ required: true })
const form = defineModel<AccountGroupFormValue>('form', { required: true })
const title = computed(() => props.group ? '编辑分组' : '创建分组')
const description = computed(() => props.group
  ? '修改分组名称和用途说明。'
  : '创建后，可在账号管理中将账号加入这个分组。')
</script>

<template>
  <BaseModal
    v-model="open"
    :title="title"
    :description="description"
    size="md"
    :dismissible="!saving"
  >
    <BaseForm class="grid gap-5">
      <BaseFormItem :label="t('groups.editor.groupName')" required>
        <BaseInput
          v-model="form.name"
          :aria-label="t('groups.editor.groupName')"
          :placeholder="t('groups.editor.forExampleProductionAccount')"
          :disabled="saving"
        />
      </BaseFormItem>
      <BaseFormItem :label="t('groups.editor.groupColor')" required>
        <BaseColorPicker
          v-model="form.color"
          :label="t('groups.editor.selectGroupColor')"
          :presets="ACCOUNT_GROUP_COLOR_PRESETS"
          :disabled="saving"
        />
      </BaseFormItem>
      <BaseFormItem :label="t('groups.editor.descriptionOptional')">
        <BaseTextarea
          v-model="form.description"
          :aria-label="t('groups.editor.groupDescription')"
          :rows="4"
          :placeholder="t('groups.editor.describePurposeOfThisGroup')"
          :disabled="saving"
        />
      </BaseFormItem>
      <BaseFormItem :label="t('usage.billing.fastMode')">
        <BaseSelect
          v-model="form.fastMode"
          class="w-full"
          :aria-label="t('usage.billing.fastMode')"
          :options="[
            { value: 'default', label: '默认（跟随客户端）' },
            { value: 'enabled', label: '开启' },
            { value: 'disabled', label: '关闭' },
          ]"
          :disabled="saving"
        />
      </BaseFormItem>
      <BaseSwitch v-model="form.allowImageGeneration" label="允许生图" show-label :disabled="saving" />
    </BaseForm>

    <template #footer>
      <BaseButton variant="secondary" :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="!form.name.trim()"
        @click="emit('save')"
      >
        {{ t('groups.editor.saveGroup') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
