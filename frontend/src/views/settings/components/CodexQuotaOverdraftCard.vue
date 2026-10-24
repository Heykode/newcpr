<script setup lang="ts">
import { Radar } from '@lucide/vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

defineProps<{
  disabled?: boolean
}>()

const enabled = defineModel<boolean>('enabled', { required: true })
const probeModel = defineModel<string>('probeModel', { required: true })
</script>

<template>
  <BaseCard title="Codex 额度透支">
    <div class="grid max-w-6xl min-w-0 gap-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
      <BaseFormItem label="透支探测模型" :required="enabled">
        <BaseInput
          v-model="probeModel"
          :disabled="disabled"
          aria-label="透支探测模型"
          maxlength="128"
          placeholder="gpt-6-astra"
        >
          <template #prefix>
            <Radar class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseSwitch
        v-model="enabled"
        class="min-h-cp-control justify-self-start sm:mb-0.5"
        :disabled="disabled"
        label="启用额度透支"
        show-label
      />
    </div>
  </BaseCard>
</template>
