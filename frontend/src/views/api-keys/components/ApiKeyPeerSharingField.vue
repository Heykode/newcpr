<script setup lang="ts">
import type { ApiKeyPeerSharing } from '@/api'
import { computed } from 'vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import HelpPopover from '@/components/HelpPopover.vue'
import { t } from '@/i18n'

defineProps<{ disabled: boolean }>()
const sharing = defineModel<ApiKeyPeerSharing>({ required: true })
const permissions = computed(() => ({ sharePool: t('keys.editor.sharePool'), shareQuota: t('keys.editor.shareQuota'), sharePlans: t('keys.editor.sharePlans') }))
function update(field: keyof ApiKeyPeerSharing, value: boolean) {
  sharing.value = { ...sharing.value, [field]: value, ...(field === 'sharePool' && !value ? { sharePlans: false } : {}) }
}
</script>

<template>
  <BaseFormItem :label="t('keys.editor.peerSharing')">
    <template #label-extra>
      <HelpPopover :label="t('keys.editor.peerSharing')" :content="t('keys.editor.peerSharingHelp')" />
    </template>
    <div class="flex flex-wrap items-center gap-x-5 gap-y-3">
      <BaseSwitch :model-value="sharing.enabled" :aria-label="t('keys.editor.peerSharing')" :disabled="disabled" @update:model-value="update('enabled', $event)" />
      <template v-if="sharing.enabled">
        <BaseCheckbox v-for="(label, field) in permissions" :key="field" :model-value="sharing[field]" :label="label" show-label :disabled="disabled || (field === 'sharePlans' && !sharing.sharePool)" @update:model-value="update(field, $event)" />
      </template>
    </div>
  </BaseFormItem>
</template>
