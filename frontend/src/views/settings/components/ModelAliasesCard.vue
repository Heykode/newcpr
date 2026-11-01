<script setup lang="ts">
import type { FormRule } from '@/components/base/BaseForm/types'
import { Plus, Trash2 } from '@lucide/vue'
import { computed } from 'vue'
import ZButton from '@/components/base/BaseButton.vue'
import ZCard from '@/components/base/BaseCard.vue'
import ZFormItem from '@/components/base/BaseForm/FormItem.vue'

import ZIconButton from '@/components/base/BaseIconButton.vue'

import ZInput from '@/components/base/BaseInput.vue'
import { t } from '@/i18n'

const props = withDefaults(defineProps<{
  mappings: Array<{ requestedModel: string, upstreamModel: string }>
  loading?: boolean
  error?: string
}>(), {
  loading: false,
})

const emit = defineEmits<{
  addMapping: []
  updateMapping: [index: number, key: 'requestedModel' | 'upstreamModel', value: string]
  removeMapping: [index: number]
}>()

const requiredRule = computed<FormRule>(() => ({
  required: true,
  whitespace: true,
  trigger: ['blur', 'change'],
  message: t('settings.form.fillInModelMappingCompletely'),
}))
const uniqueModelRule = computed<FormRule>(() => ({
  trigger: ['blur', 'change'],
  validator: (_rule, value: string) => {
    const requested = value.trim()
    return !requested || props.mappings.filter(row => row.requestedModel.trim() === requested).length < 2
      || new Error(t('settings.form.duplicateClientModel', { requested }))
  },
}))
</script>

<template>
  <ZCard
    :title="t('settings.modelMapping')"
    :description="t('settings.routing.modelMapHelp')"
  >
    <div class="grid gap-4">
      <div class="flex flex-wrap items-center gap-3">
        <ZButton :disabled="loading" @click="emit('addMapping')">
          <template #icon>
            <Plus class="size-4" />
          </template>
          {{ t('settings.addMapping') }}
        </ZButton>
        <span v-if="error" class="text-xs font-emphasis text-cp-error-text">{{ error }}</span>
      </div>

      <div
        v-if="loading"
        class="rounded-cp bg-cp-fill-quaternary px-4 py-4 text-cp font-emphasis text-cp-text-quaternary"
      >
        {{ t('settings.loadingModelMapping') }}
      </div>
      <div v-else-if="mappings.length > 0" class="grid max-w-6xl gap-3">
        <div
          v-for="(row, index) in mappings"
          :key="index"
          class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-4.5 rounded-cp-card bg-cp-fill-quaternary p-3 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]"
        >
          <ZFormItem :prop="`modelMappings.${index}.requestedModel`" :rules="[requiredRule, uniqueModelRule]" :show-message="Boolean(row.requestedModel.trim())">
            <ZInput
              :model-value="row.requestedModel"
              :placeholder="t('settings.requestModel')"
              :aria-label="t('settings.requestModel')"
              @update:model-value="emit('updateMapping', index, 'requestedModel', $event)"
            />
          </ZFormItem>
          <span class="hidden text-cp-text-quaternary sm:block" aria-hidden="true">→</span>
          <ZFormItem :prop="`modelMappings.${index}.upstreamModel`" :rules="requiredRule" :show-message="false" class="col-start-1 row-start-2 sm:col-start-auto sm:row-start-auto">
            <ZInput
              :model-value="row.upstreamModel"
              :placeholder="t('settings.upstreamModel')"
              :aria-label="t('settings.upstreamModelName')"
              @update:model-value="emit('updateMapping', index, 'upstreamModel', $event)"
            />
          </ZFormItem>
          <ZIconButton
            class="col-start-2 row-span-2 row-start-1 sm:col-start-auto sm:row-span-1 sm:row-start-auto"
            :label="t('settings.deleteMapping')"
            @click="emit('removeMapping', index)"
          >
            <Trash2 class="size-4 text-cp-error" />
          </ZIconButton>
        </div>
      </div>
    </div>
  </ZCard>
</template>
