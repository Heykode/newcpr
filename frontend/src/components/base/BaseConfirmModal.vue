<script setup lang="ts">
import { t } from '@/i18n'
import BaseButton from './BaseButton.vue'
import BaseModal from './BaseModal/index.vue'

const props = withDefaults(
  defineProps<{
    title: string
    description?: string
    destructive?: boolean
    confirmText?: string
    cancelText?: string
    loading?: boolean
    confirmDisabled?: boolean
  }>(),
  {
    destructive: false,
    confirmText: undefined,
    cancelText: undefined,
    loading: false,
    confirmDisabled: false,
  },
)

const emit = defineEmits<{
  confirm: []
  cancel: []
}>()
const open = defineModel<boolean>({ default: false })
function handleCancel() {
  if (props.loading)
    return
  open.value = false
  emit('cancel')
}

function handleConfirm() {
  if (props.loading || props.confirmDisabled)
    return
  emit('confirm')
}
</script>

<template>
  <BaseModal
    v-model="open"
    :title="title"
    :description="description"
    size="sm"
    :tone="destructive ? 'danger' : 'warning'"
    role="alertdialog"
    :dismissible="!loading"
  >
    <div
      v-if="$slots.default"
      class="text-cp-lg leading-[1.55] font-emphasis text-cp-text-secondary"
    >
      <slot />
    </div>

    <template #footer>
      <BaseButton variant="secondary" :disabled="loading" @click="handleCancel">
        {{ cancelText ?? t('ui.cancel') }}
      </BaseButton>
      <BaseButton
        :variant="destructive ? 'destructive' : 'primary'"
        :loading="loading"
        :disabled="confirmDisabled"
        @click="handleConfirm"
      >
        {{ confirmText ?? t('ui.confirm') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
