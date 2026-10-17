<script setup lang="ts">
import { ref } from 'vue'
import { createProxyBatch } from '@/api/modules/proxies'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'

const emit = defineEmits<{ created: [] }>()
const open = defineModel<boolean>({ default: false })
const prefix = ref('代理')
const input = ref('')
const loading = ref(false)
const messages = ref<string[]>([])
const submitted = ref(false)
async function submit() {
  if (loading.value || submitted.value)
    return
  const lines = input.value.split(/\r?\n/).map(line => line.trim()).filter(Boolean)
  if (!prefix.value.trim() || !lines.length || lines.length > 100) {
    messages.value = ['请填写名称前缀及 1 至 100 条代理地址']
    return
  }
  loading.value = true
  submitted.value = true
  messages.value = []
  try {
    const result = await createProxyBatch(lines.map((proxyUrl, index) => ({ name: `${prefix.value.trim()} ${index + 1}`, proxyUrl })))
    messages.value = result.map((item, index) => `第 ${index + 1} 条：${item.status === 'succeeded' ? '已创建' : item.status === 'failed' ? item.message : '未执行'}`)
  }
  catch {
    messages.value = ['结果未确认，请刷新代理列表核对已创建项']
  }
  finally {
    // Credentials are not kept in the result view or retried automatically.
    input.value = ''
    loading.value = false
    emit('created')
  }
}
</script>

<template>
  <BaseModal v-model="open" title="批量新增代理" :dismissible="!loading">
    <div class="grid gap-3">
      <BaseInput v-model="prefix" aria-label="代理名称前缀" placeholder="代理名称前缀" :disabled="loading || submitted" />
      <BaseTextarea v-model="input" aria-label="代理地址，每行一条" placeholder="http://user:password@host:port" :rows="8" :disabled="loading || submitted" />
      <div v-if="messages.length" role="status" class="max-h-64 overflow-auto text-cp-sm">
        <p v-for="(message, index) in messages" :key="index">
          {{ message }}
        </p>
      </div>
      <BaseButton :loading="loading" :disabled="submitted" @click="submit">
        创建代理
      </BaseButton>
    </div>
  </BaseModal>
</template>
