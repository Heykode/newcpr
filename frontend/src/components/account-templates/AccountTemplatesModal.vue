<script setup lang="ts">
import type { AccountTemplate } from '@/api/modules/account-templates'
import { Pencil, Plus, RefreshCw, Trash2, X } from '@lucide/vue'
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { deleteAccountTemplate, getAccountTemplates, saveAccountTemplate } from '@/api/modules/account-templates'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { toast } from '@/components/base/BaseToast'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { errorMessage } from '@/utils/async'
import AccountSettingsFields from '@/views/accounts/components/AccountSettingsFields.vue'
import { templateConfig, templateForm } from './template-form'

const open = defineModel<boolean>({ required: true })
const templates = shallowRef<AccountTemplate[]>([])
const loading = shallowRef(false)
const busy = shallowRef(false)
const error = shallowRef('')
const editing = shallowRef(false)
const current = shallowRef<AccountTemplate>()
const deleting = shallowRef<AccountTemplate>()
const deleteOpen = shallowRef(false)
const form = ref(templateForm())
const { groups, loading: groupsLoading, loadGroups } = useAccountGroupCatalog({ immediate: false })
const missingGroups = computed(() => groupsLoading.value ? [] : form.value.groupIds.filter(id => !groups.value.some(group => group.id === id)))
let controller: AbortController | undefined
let disposed = false

async function load() {
  controller?.abort()
  const owner = new AbortController()
  controller = owner
  loading.value = true
  try {
    const rows = await getAccountTemplates({ silent: true, signal: owner.signal })
    if (!owner.signal.aborted && !disposed) {
      templates.value = rows
      error.value = ''
    }
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
function edit(row?: AccountTemplate) {
  current.value = row
  form.value = templateForm(row?.config)
  error.value = ''
  editing.value = true
  void loadGroups()
}
async function save() {
  if (busy.value)
    return
  busy.value = true
  error.value = ''
  try {
    const selection = current.value && { id: current.value.id, revision: current.value.revision }
    await saveAccountTemplate(templateConfig(form.value), selection)
    if (disposed)
      return
    editing.value = false
    toast.success('账号模板已保存')
    await load()
  }
  catch (cause) {
    if (!disposed)
      error.value = errorMessage(cause)
  }
  finally { busy.value = false }
}
function requestDelete(row: AccountTemplate) {
  deleting.value = row
  error.value = ''
  deleteOpen.value = true
}
async function remove() {
  if (busy.value || !deleting.value)
    return
  busy.value = true
  error.value = ''
  try {
    await deleteAccountTemplate({ id: deleting.value.id, revision: deleting.value.revision })
    if (disposed)
      return
    deleteOpen.value = false
    toast.success('模板已删除')
    await load()
  }
  catch (cause) {
    if (!disposed) {
      deleteOpen.value = false
      error.value = errorMessage(cause)
    }
  }
  finally { busy.value = false }
}
watch(open, (value) => {
  if (value) {
    editing.value = false
    error.value = ''
    void load()
  }
  else {
    controller?.abort()
    deleteOpen.value = false
  }
})
onScopeDispose(() => {
  disposed = true
  controller?.abort()
})
</script>

<template>
  <BaseModal v-model="open" :title="editing ? current ? '编辑账号模板' : '新建账号模板' : '账号模板'" :dismissible="!busy">
    <div v-if="error" class="mb-4 break-words text-cp-sm text-cp-error" role="alert">
      {{ error }}
    </div>
    <div v-if="editing" class="grid gap-5">
      <BaseFormItem label="模板名称">
        <BaseInput v-model="form.name" aria-label="模板名称" :disabled="busy" />
      </BaseFormItem>
      <AccountSettingsFields
        v-model:enabled="form.enabled"
        v-model:turn-state-injection-enabled="form.turnStateInjectionEnabled"
        v-model:concurrency-limit="form.concurrencyLimit"
        v-model:weight="form.weight"
        v-model:selected-group-ids="form.groupIds"
        v-model:proxy-mode="form.proxyMode"
        v-model:proxy-id="form.proxyId"
        :groups="groups"
        :groups-loading="groupsLoading"
        :preserve-proxy="false"
        turn-state-available
        :disabled="busy"
      />
      <div v-for="id in missingGroups" :key="id" class="flex min-w-0 items-center gap-2 text-cp-sm text-cp-warning">
        <span class="min-w-0 flex-1 break-all">未识别分组：{{ id }}</span>
        <BaseIconButton :label="`移除未识别分组 ${id}`" :disabled="busy" @click="form.groupIds = form.groupIds.filter(value => value !== id)">
          <X class="size-4" />
        </BaseIconButton>
      </div>
    </div>
    <template v-else>
      <div class="mb-3 flex items-center justify-between gap-2">
        <BaseButton :disabled="loading || busy" @click="edit()">
          <template #icon>
            <Plus class="size-4" />
          </template>新建模板
        </BaseButton>
        <BaseIconButton label="刷新账号模板" :loading="loading" :disabled="busy" @click="load()">
          <RefreshCw class="size-4" />
        </BaseIconButton>
      </div>
      <p v-if="!loading && !templates.length" class="text-cp-sm text-cp-text-secondary">
        暂无账号模板
      </p>
      <div v-for="row in templates" :key="row.id" class="flex min-w-0 items-center gap-3 border-b border-cp-border py-3">
        <div class="min-w-0 flex-1">
          <div class="break-all text-cp font-medium">
            {{ row.config.name }}
          </div>
          <div class="mt-1 text-cp-xs text-cp-text-secondary">
            {{ row.config.enabled ? '启用调度' : '暂停调度' }} · State {{ row.config.turnStateInjectionEnabled == null ? '未设置' : row.config.turnStateInjectionEnabled ? '开' : '关' }} · 并发 {{ row.config.concurrencyLimit ?? '默认' }} · 权重 {{ row.config.weight }} · {{ row.config.groupIds.length }} 个分组
          </div>
        </div>
        <BaseIconButton label="编辑模板" :disabled="busy" @click="edit(row)">
          <Pencil class="size-4" />
        </BaseIconButton>
        <BaseIconButton label="删除模板" :disabled="busy" @click="requestDelete(row)">
          <Trash2 class="size-4 text-cp-error" />
        </BaseIconButton>
      </div>
    </template>
    <template #footer>
      <BaseButton :disabled="busy" @click="editing ? (editing = false, error = '') : (open = false)">
        {{ editing ? '返回' : '关闭' }}
      </BaseButton>
      <BaseButton v-if="editing" variant="primary" :loading="busy" :disabled="groupsLoading" @click="save()">
        保存模板
      </BaseButton>
    </template>
  </BaseModal>
  <BaseConfirmModal v-model="deleteOpen" title="删除账号模板" destructive :loading="busy" @confirm="remove()">
    <p class="m-0 break-all">
      确认删除「{{ deleting?.config.name }}」？已入池账号保持不变。
    </p>
  </BaseConfirmModal>
</template>
