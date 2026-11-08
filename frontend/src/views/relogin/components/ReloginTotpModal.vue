<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { onBeforeUnmount, shallowRef, watch } from 'vue'
import { getReloginTotp } from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { useCopyText } from '@/composables/useCopyText'
import { errorMessage } from '@/utils/async'

const props = defineProps<{ entry: ReloginEntry | null }>()
const emit = defineEmits<{ afterLeave: [] }>()
const open = defineModel<boolean>({ default: false })
const secret = shallowRef('')
const loading = shallowRef(false)
const failure = shallowRef('')
const copyText = useCopyText()
let controller: AbortController | undefined

watch(() => [open.value, props.entry?.id] as const, async ([visible, id]) => {
  controller?.abort()
  if (!visible || !id)
    return
  secret.value = ''
  failure.value = ''
  loading.value = false
  const current = new AbortController()
  controller = current
  loading.value = true
  try {
    const result = await getReloginTotp(id, { signal: current.signal, silent: true })
    if (!current.signal.aborted)
      secret.value = result.secret
  }
  catch (error) {
    if (!current.signal.aborted)
      failure.value = errorMessage(error, '2FA 读取失败')
  }
  finally {
    if (!current.signal.aborted)
      loading.value = false
  }
}, { immediate: true, flush: 'sync' })
onBeforeUnmount(() => {
  controller?.abort()
  secret.value = ''
})
function afterLeave() {
  if (!open.value) {
    secret.value = ''
    failure.value = ''
    loading.value = false
  }
  emit('afterLeave')
}
</script>

<template>
  <BaseModal v-model="open" title="2FA 密钥" :description="entry?.email" size="sm" @after-leave="afterLeave">
    <p v-if="loading" role="status" class="text-cp-text-tertiary">
      正在读取
    </p>
    <p v-else-if="failure" role="alert" class="text-cp-error">
      {{ failure }}
    </p>
    <code v-else class="block select-all break-all rounded-cp-md bg-cp-fill-quaternary p-3 font-mono">{{ secret }}</code>
    <template #footer>
      <BaseButton :disabled="!secret || loading" @click="copyText(secret)">
        复制密钥
      </BaseButton>
    </template>
  </BaseModal>
</template>
