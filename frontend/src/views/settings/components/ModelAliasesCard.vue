<script setup lang="ts">
import { GitBranch, Plus, Trash2 } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { t } from '@/i18n'

withDefaults(defineProps<{
  mappings: Array<{ requestedModel: string, upstreamModel: string }>
  loading?: boolean
  error?: string
}>(), {
  loading: false,
  error: '',
})

const emit = defineEmits<{
  addMapping: []
  updateMapping: [index: number, key: 'requestedModel' | 'upstreamModel', value: string]
  removeMapping: [index: number]
}>()
</script>

<template>
  <BaseCard
    :title="t('settings.modelMapping')"
    description="配置请求模型与上游模型的映射关系"
  >
    <div class="grid gap-4">
      <div class="flex flex-wrap items-center gap-3">
        <BaseButton variant="secondary" :disabled="loading" @click="emit('addMapping')">
          <template #icon>
            <Plus class="size-4" />
          </template>
          {{ t('settings.addMapping') }}
        </BaseButton>
        <span v-if="error" class="text-xs font-emphasis text-cp-error-text">{{ error }}</span>
      </div>

      <div class="flex items-center gap-2 text-cp-sm font-emphasis text-cp-text-secondary">
        <GitBranch class="size-4 text-cp-primary-text" />
        全局模型映射
      </div>

      <div
        v-if="loading"
        class="rounded-cp bg-cp-fill-quaternary px-4 py-4 text-cp font-emphasis text-cp-text-quaternary"
      >
        正在加载模型映射...
      </div>
      <div
        v-else-if="mappings.length === 0"
        class="rounded-cp bg-cp-fill-quaternary px-4 py-4 text-cp font-emphasis text-cp-text-quaternary"
      >
        暂无模型映射
      </div>
      <div v-else class="grid gap-3">
        <div
          v-for="(row, index) in mappings"
          :key="index"
          class="grid grid-cols-[1fr_auto_1fr_auto] items-center gap-3 rounded-cp-card bg-cp-fill-quaternary p-3"
        >
          <BaseInput
            :model-value="row.requestedModel"
            :placeholder="t('settings.requestModel')"
            :aria-label="t('settings.requestModel')"
            @update:model-value="emit('updateMapping', index, 'requestedModel', $event)"
          />
          <span class="text-cp-text-quaternary">→</span>
          <BaseInput
            :model-value="row.upstreamModel"
            :placeholder="t('settings.upstreamModel')"
            :aria-label="t('settings.upstreamModelName')"
            @update:model-value="emit('updateMapping', index, 'upstreamModel', $event)"
          />
          <BaseIconButton
            variant="ghost"
            :label="t('settings.deleteMapping')"
            @click="emit('removeMapping', index)"
          >
            <Trash2 class="size-4 text-cp-error" />
          </BaseIconButton>
        </div>
      </div>
    </div>
  </BaseCard>
</template>
