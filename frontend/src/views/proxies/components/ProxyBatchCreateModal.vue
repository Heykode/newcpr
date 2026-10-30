<script setup lang="ts">
import { testProxyBatch } from '@/api/modules/proxies'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { useProxyBatchCreate } from '../composables/useProxyBatchCreate'
import { proxyBatchStates, proxyProtocolOptions } from '../utils/batch'

const emit = defineEmits<{ created: [] }>()
const open = defineModel<boolean>({ default: false })
const { text, prefix, protocol, parsed, inputError, items, busy, stopping, created, canEditFailures, tests, submit, stop, clear, editFailures, testCreated } = useProxyBatchCreate({
  test: (proxies, options, detectLocation) => testProxyBatch(proxies.map(({ id, revision }) => ({ id, revision, detectLocation })), options),
  reload: async () => emit('created'),
})
</script>

<template>
  <BaseModal v-model="open" title="批量新增代理" :dismissible="!busy">
    <div class="grid gap-3">
      <template v-if="!items.length">
        <BaseInput v-model="prefix" aria-label="代理名称前缀" placeholder="代理名称前缀" :disabled="busy" />
        <BaseSelect v-model="protocol" :options="proxyProtocolOptions" aria-label="代理协议" :disabled="busy" />
        <BaseTextarea v-model="text" aria-label="代理地址，每行一条" placeholder="host:port:username:password" :rows="8" :disabled="busy" />
        <p v-if="inputError" role="alert" class="m-0 text-cp-sm text-cp-error-text">
          {{ inputError }}
        </p>
        <div v-if="parsed.length" class="max-h-48 overflow-auto text-cp-sm" aria-label="代理预览">
          <p v-for="entry in parsed" :key="entry.line" class="break-all" :class="entry.error ? 'text-cp-error-text' : 'text-cp-text-secondary'">
            {{ entry.line }} · {{ entry.name }} · {{ entry.error || entry.endpoint }}
          </p>
        </div>
        <div class="flex flex-wrap gap-2">
          <BaseButton :disabled="busy || Boolean(inputError) || !parsed.length" @click="submit(false)">
            创建代理
          </BaseButton>
          <BaseButton :disabled="busy || Boolean(inputError) || !parsed.length" @click="submit(true)">
            创建并测试
          </BaseButton>
        </div>
      </template>
      <template v-else>
        <div role="status" class="max-h-64 overflow-auto text-cp-sm">
          <p v-for="item in items" :key="item.key" class="break-all" :class="proxyBatchStates[item.status].tone">
            {{ item.name }} · {{ proxyBatchStates[item.status].label }} · {{ item.message }}
          </p>
          <p v-for="item in tests.items.value" :key="`test-${item.key}`" class="break-all" :class="proxyBatchStates[item.status].tone">
            {{ item.name }} · {{ proxyBatchStates[item.status].label }} · {{ item.message }}
          </p>
        </div>
        <div class="flex flex-wrap gap-2">
          <BaseButton v-if="busy" :disabled="stopping" @click="stop">
            停止后续操作
          </BaseButton>
          <template v-else>
            <BaseButton v-if="created.length" @click="testCreated">
              测试已创建代理
            </BaseButton>
            <BaseButton v-if="canEditFailures" @click="editFailures">
              修改失败项
            </BaseButton>
            <BaseButton @click="clear">
              继续新增
            </BaseButton>
          </template>
        </div>
      </template>
    </div>
  </BaseModal>
</template>
