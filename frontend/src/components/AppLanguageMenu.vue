<script setup lang="ts">
import type { Locale } from '@/i18n/locale'
import { Languages } from '@lucide/vue'
import { computed, shallowRef, useId } from 'vue'
import { toast } from '@/components/base/BaseToast'
import { locale, setLocale, t } from '@/i18n'
import { localeOptions } from '@/i18n/locale'

const pending = shallowRef<Locale>()
const controlId = useId()
const label = computed(() => t('navigation.switchLanguage', {
  value: localeOptions.find(option => option.value === locale.value)?.label ?? 'English',
}))
async function select(event: Event) {
  const value = (event.target as HTMLSelectElement).value as Locale
  pending.value = value
  try {
    await setLocale(value)
  }
  catch {
    toast.error(t('navigation.languageLoadFailed'))
  }
  finally {
    if (pending.value === value)
      pending.value = undefined
  }
}
</script>

<template>
  <label :for="controlId" class="relative flex h-9 w-9 shrink-0 items-center justify-center" :title="label">
    <Languages :size="18" aria-hidden="true" />
    <select
      :id="controlId"
      :value="pending ?? locale"
      :aria-label="label"
      class="absolute inset-0 h-full w-full cursor-pointer opacity-0"
      @change="select"
    >
      <option v-for="option in localeOptions" :key="option.value" :value="option.value">
        {{ option.label }}
      </option>
    </select>
  </label>
</template>
