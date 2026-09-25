<script setup lang="ts">
import type { AccountTemplate } from '@/api/modules/account-templates'
import { ChevronDown, LayoutTemplate, RefreshCw, Settings2 } from '@lucide/vue'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { applyAccountTemplate, getAccountTemplates } from '@/api/modules/account-templates'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import AccountTemplatesModal from './AccountTemplatesModal.vue'

const props = defineProps<{ accountIds: string[], disabled?: boolean }>()
const emit = defineEmits<{ applied: [accountIds: string[]], applying: [value: boolean] }>()
const applying = shallowRef(false)
const open = shallowRef(false)
const managing = shallowRef(false)
const loading = shallowRef(false)
const error = shallowRef('')
const templates = shallowRef<AccountTemplate[]>([])
let controller: AbortController | undefined
let disposed = false

async function load() {
  controller?.abort()
  const owner = new AbortController()
  controller = owner
  loading.value = true
  error.value = ''
  templates.value = []
  try {
    const result = await getAccountTemplates({ silent: true, signal: owner.signal })
    if (!owner.signal.aborted && !disposed)
      templates.value = result
  }
  catch (cause) {
    if (!owner.signal.aborted && !disposed)
      error.value = errorMessage(cause)
  }
  finally {
    if (controller === owner)
      loading.value = false
  }
}

async function apply(template: AccountTemplate) {
  if (applying.value || props.disabled || !props.accountIds.length || props.accountIds.length > 1000)
    return
  const ids = [...props.accountIds]
  const selection = { id: template.id, revision: template.revision }
  applying.value = true
  error.value = ''
  try {
    const result = await applyAccountTemplate(ids, selection)
    if (disposed)
      return
    open.value = false
    toast.success(`已为 ${result.accountIds.length} 个账号应用「${template.config.name}」`)
    emit('applied', result.accountIds)
  }
  catch (cause) {
    if (!disposed)
      error.value = errorMessage(cause)
  }
  finally { applying.value = false }
}

watch(applying, value => emit('applying', value), { flush: 'sync' })
watch(open, (value) => {
  if (value) {
    if (!applying.value) {
      error.value = ''
      void load()
    }
  }
  else {
    controller?.abort()
  }
})
onScopeDispose(() => {
  disposed = true
  controller?.abort()
})
</script>

<template>
  <BasePopover v-model="open" class="w-full xl:w-auto" :disabled="disabled || applying">
    <template #trigger>
      <BaseButton class="w-full whitespace-nowrap xl:w-auto" :disabled="disabled" :loading="applying" aria-label="账号模板">
        <LayoutTemplate class="size-4" />
        账号模板
        <ChevronDown class="size-3.5" />
      </BaseButton>
    </template>
    <div class="w-80 max-w-[calc(100vw-16px)] p-2" aria-label="账号模板菜单">
      <div class="flex items-center justify-between gap-2 px-3 py-2 text-cp-sm text-cp-text-secondary">
        <span>已选 {{ accountIds.length }} 个账号</span>
        <BaseIconButton label="刷新账号模板" :loading="loading" :disabled="applying" @click="load()">
          <RefreshCw class="size-4" />
        </BaseIconButton>
      </div>
      <p v-if="error" class="mx-3 my-2 break-words text-cp-sm text-cp-error" role="alert">
        {{ error }}
      </p>
      <p v-if="accountIds.length > 1000" class="mx-3 my-2 text-cp-sm text-cp-warning">
        每次最多应用 1000 个账号
      </p>
      <p v-if="loading || !templates.length" class="mx-3 my-2 text-cp-sm text-cp-text-secondary">
        {{ loading ? '加载中' : '暂无账号模板' }}
      </p>
      <div class="max-h-64 overflow-y-auto">
        <BaseMenuItem
          v-for="template in templates"
          :key="template.id"
          :disabled="loading || applying || !accountIds.length || accountIds.length > 1000"
          :title="template.config.name"
          :aria-label="`应用模板：${template.config.name}`"
          class="py-2"
          @click="apply(template)"
        >
          <span class="block truncate">{{ template.config.name }}</span>
          <span class="mt-1 block whitespace-normal break-words text-cp-xs leading-normal text-cp-text-secondary">
            {{ template.config.enabled ? '启用调度' : '暂停调度' }} · 并发 {{ template.config.concurrencyLimit ?? '默认' }} · 权重 {{ template.config.weight }}
          </span>
        </BaseMenuItem>
      </div>
      <div class="mt-2 border-t border-cp-border pt-2">
        <BaseMenuItem :disabled="applying" @click="open = false; managing = true">
          <template #icon>
            <Settings2 class="size-4" />
          </template>
          管理模板
        </BaseMenuItem>
      </div>
    </div>
  </BasePopover>
  <AccountTemplatesModal v-model="managing" />
</template>
