<script setup lang="ts">
import type { CodexDesktopWindowsDownloads } from '@/api'
import { MonitorUp, PackageOpen, TerminalSquare } from '@lucide/vue'
import { shallowRef } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { t } from '@/i18n'

import ClientCliInstallPanel from './ClientCliInstallPanel.vue'
import ClientDesktopInstallPanel from './ClientDesktopInstallPanel.vue'

defineOptions({ name: 'ClientVersionGuideModal' })

withDefaults(
  defineProps<{
    downloads?: CodexDesktopWindowsDownloads | null
    loading?: boolean
    error?: string
  }>(),
  {
    downloads: null,
    loading: false,
    error: '',
  },
)

const emit = defineEmits<{
  refresh: []
  retry: []
}>()

type ClientSection = 'desktop' | 'cli'

const open = defineModel<boolean>({ default: false })
const activeSection = shallowRef<ClientSection>('desktop')
const sectionOptions = [
  { label: 'Codex Desktop', value: 'desktop', icon: MonitorUp },
  { label: 'Codex CLI', value: 'cli', icon: TerminalSquare },
]
</script>

<template>
  <BaseModal
    v-model="open"
    :title="t('settings.clientVersion.codexClientUpgrade')"
    :description="t('settings.clientVersion.guideDescription')"
    tone="info"
    size="lg"
  >
    <template #icon>
      <PackageOpen class="size-4.5 text-cp-info" />
    </template>

    <div class="grid gap-4">
      <BaseSegmented
        v-model="activeSection"
        :label="t('settings.clientProfile.clientType')"
        :options="sectionOptions"
        size="lg"
        class="w-full"
      />

      <ClientDesktopInstallPanel
        v-if="activeSection === 'desktop'"
        :downloads="downloads"
        :loading="loading"
        :error="error"
        @retry="emit('retry')"
        @refresh="emit('refresh')"
      />
      <ClientCliInstallPanel v-else />
    </div>

    <template #footer>
      <BaseButton variant="primary" @click="open = false">
        {{ t('common.actions.close') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
