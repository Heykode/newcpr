<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { onBeforeUnmount, shallowRef, watch } from 'vue'
import { exportRelogin } from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { useCopyText } from '@/composables/useCopyText'
import { errorMessage } from '@/utils/async'

const props = defineProps<{ entry: ReloginEntry | null }>()
const emit = defineEmits<{ afterLeave: [] }>()
const open = defineModel<boolean>({ default: false })
const content = shallowRef('')
const format = shallowRef('two_fa')
const formats = [{ label: '2FA', value: 'two_fa' }, { label: 'JSON', value: 'json' }]
const loading = shallowRef(false)
const failure = shallowRef('')
const copyText = useCopyText()
let controller: AbortController | undefined

watch(() => [open.value, props.entry?.id, format.value] as const, async ([visible, id, selectedFormat]) => {
  controller?.abort()
  content.value = ''
  failure.value = ''
  loading.value = false
  if (!visible || !id)
    return
  const current = new AbortController()
  controller = current
  loading.value = true
  try {
    const result = await exportRelogin([id], selectedFormat === 'json' ? 'json' : 'two_fa', { signal: current.signal, silent: true })
    if (!current.signal.aborted)
      content.value = result.files.map(file => file.content).join('\n')
  }
  catch (error) {
    if (!current.signal.aborted)
      failure.value = errorMessage(error, '账号资料读取失败')
  }
  finally {
    if (!current.signal.aborted)
      loading.value = false
  }
}, { immediate: true, flush: 'sync' })
onBeforeUnmount(() => {
  controller?.abort()
  content.value = ''
})
function afterLeave() {
  if (!open.value) {
    content.value = ''
    failure.value = ''
    loading.value = false
  }
  emit('afterLeave')
}
</script>

<template>
  <BaseModal v-model="open" title="账号资料" :description="entry?.displayName ? `${entry.displayName} · ${entry.email}` : entry?.email" size="lg" @after-leave="afterLeave">
    <BaseSegmented v-model="format" label="资料格式" :options="formats" class="mb-4 w-48" />
    <p class="text-cp-xs text-cp-text-secondary">
      {{ format === 'two_fa' ? '邮箱----密码----2FA 密钥，与导出资料一致。' : '当前可导出的账号 JSON，与批量导出使用同一份资料。' }}
    </p>
    <p v-if="loading" role="status" class="text-cp-text-tertiary">
      正在读取
    </p>
    <p v-else-if="failure" role="alert" class="text-cp-error">
      {{ failure }}
    </p>
    <pre v-else-if="content" class="m-0 max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-cp bg-cp-fill-quaternary p-4 text-cp-sm"><code class="select-all font-mono">{{ content }}</code></pre>
    <p v-else class="text-cp-text-secondary">
      暂无可展示的资料
    </p>
    <template #footer>
      <BaseButton :disabled="!content || loading" @click="copyText(content)">
        {{ format === 'two_fa' ? '复制完整 2FA 资料' : '复制 JSON' }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
