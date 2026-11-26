<script setup lang="ts">
import type { RequestProfilePreview } from '@/api'

import { useI18n } from 'vue-i18n'
import BaseSkeleton from '@/components/base/BaseSkeleton.vue'

defineProps<{
  preview?: RequestProfilePreview
  previewing: boolean
  needsVersionInput: boolean
}>()

const { t } = useI18n({ useScope: 'global' })
</script>

<template>
  <p v-if="needsVersionInput" role="status" class="m-0 text-cp-sm text-cp-text-tertiary">
    {{ t('settings.clientProfile.previewAfterFillingInVersion') }}
  </p>
  <div
    v-else-if="previewing || preview"
    class="flex min-h-20 min-w-0 flex-col justify-between gap-2 rounded-cp bg-cp-fill-quaternary p-4"
    aria-live="polite"
    :aria-busy="previewing"
  >
    <template v-if="previewing && !preview">
      <div class="flex h-lh items-center text-cp-sm" role="status" :aria-label="t('settings.clientProfile.resolvingClientIdentity')">
        <BaseSkeleton shape="text" class="w-4/5" aria-hidden="true" />
      </div>
      <div class="flex h-lh items-center justify-end text-cp-xs" aria-hidden="true">
        <BaseSkeleton shape="text" class="w-52 max-w-full" />
      </div>
    </template>
    <template v-else-if="preview">
      <code class="break-all text-cp-sm text-cp-text">{{ preview.userAgent }}</code>
      <div class="flex flex-wrap items-end gap-x-3 gap-y-1">
        <p v-if="preview.error && preview.versionSource !== 'custom'" :title="preview.error" class="m-0 text-cp-sm text-cp-warning">
          {{ t('settings.clientProfile.updateFailedUseLastVersion') }}
        </p>
        <p class="m-0 ms-auto text-end text-cp-xs text-cp-text-tertiary">
          {{ preview.versionSource === 'custom' ? t('settings.clientProfile.fixedIdentity') : t('settings.clientProfile.automaticUpdates') }}
          <template v-if="preview.versionSource === 'official'">
            <template v-if="preview.versionLag">
              {{ t('settings.clientProfile.versionsBehind', { version: preview.versionLag }) }}
            </template>
            · {{ preview.checkedAtDisplay ? t('settings.clientProfile.checked', { checkedAtDisplay: preview.checkedAtDisplay }) : t('settings.clientProfile.toBeChecked') }}
          </template>
        </p>
      </div>
    </template>
  </div>
</template>
