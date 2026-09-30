<script setup lang="ts">
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

defineProps<{ disabled?: boolean }>()
const enabled = defineModel<boolean>('enabled', { required: true })
const interval = defineModel<string>('interval', { required: true })
</script>

<template>
  <div class="grid gap-3">
    <BaseSwitch v-model="enabled" label="暂停后自动探测 Excel，成功后恢复调度" show-label :disabled="disabled" />
    <p class="m-0 text-cp-xs text-cp-text-secondary">
      仅 Excel 模式生效，包括手动暂停。长期停用请关闭此项；401、429、空回复或断流不会恢复调度。
    </p>
    <BaseFormItem v-if="enabled" label="恢复探测间隔（分钟）">
      <BaseInput v-model="interval" type="number" min="1" max="10080" step="1" aria-label="Excel 恢复探测间隔（分钟）" :disabled="disabled" />
    </BaseFormItem>
  </div>
</template>
