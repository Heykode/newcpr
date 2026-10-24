<script setup lang="ts">
import { Github } from '@boxicons/vue'
import { AppWindow } from '@lucide/vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { t } from '@/i18n'

const emit = defineEmits<{ select: [], afterLeave: [] }>()
const open = defineModel<boolean>({ default: false })

function select() {
  emit('select')
  open.value = false
}
</script>

<template>
  <BaseModal v-model="open" :title="t('keys.configuration.importTo')" :description="t('keys.configuration.selectImportTool')" size="sm" @after-leave="emit('afterLeave')">
    <div class="flex items-center gap-2">
      <button type="button" class="flex min-w-0 flex-1 items-center gap-3 rounded-cp p-3 text-left hover:bg-cp-fill-quaternary" @click="select">
        <AppWindow class="size-5 shrink-0" aria-hidden="true" />
        <span class="min-w-0">
          <span class="block">CC Switch</span>
          <span class="block text-cp-sm text-cp-text-secondary">{{ t('keys.configuration.ccSwitchDescription') }}</span>
        </span>
      </button>
      <a href="https://github.com/farion1231/cc-switch" target="_blank" rel="noopener noreferrer" :aria-label="t('keys.configuration.viewOnGitHub', { name: 'CC Switch' })" :title="t('keys.configuration.viewOnGitHub', { name: 'CC Switch' })" class="shrink-0 p-2">
        <Github class="size-5" />
      </a>
    </div>
  </BaseModal>
</template>
