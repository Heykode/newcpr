<script setup lang="ts">
import type { MaintenanceMaterial, ReloginEntry } from '@/api/modules/relogin'
import { computed, onBeforeUnmount, shallowRef, watch } from 'vue'
import { exportRelogin, getReloginMaterial, updateReloginMaterial } from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { useCopyText } from '@/composables/useCopyText'
import { errorMessage } from '@/utils/async'
import { currentTotp } from '../totp'

const props = defineProps<{ entry: ReloginEntry | null }>()
const emit = defineEmits<{ afterLeave: [], changed: [] }>()
const open = defineModel<boolean>({ default: false })
const content = shallowRef('')
const format = shallowRef('two_fa')
const formats = [{ label: '2FA', value: 'two_fa' }, { label: 'JSON', value: 'json' }, { label: '编辑资料', value: 'edit' }, { label: '历史版本', value: 'history' }]
const loading = shallowRef(false)
const failure = shallowRef('')
const copyText = useCopyText()
const material = shallowRef<MaintenanceMaterial | null>(null)
const password = shallowRef('')
const secret = shallowRef('')
const code = shallowRef('')
const remaining = shallowRef(30)
const saving = shallowRef(false)
const editRevision = shallowRef(0)
const resolved = shallowRef(false)
const fields = computed(() => {
  const line = content.value.trimEnd()
  const first = line.indexOf('----')
  const last = line.lastIndexOf('----')
  return first < 0 || last <= first ? null : { email: line.slice(0, first), password: line.slice(first + 4, last), secret: line.slice(last + 4) }
})
let controller: AbortController | undefined
let interval: ReturnType<typeof setInterval> | undefined
async function tick() {
  const activeSecret = fields.value?.secret
  const activeController = controller
  if (!open.value || !activeSecret || format.value !== 'two_fa') {
    code.value = ''
    return
  }
  remaining.value = 30 - Math.floor(Date.now() / 1000) % 30
  try {
    const next = await currentTotp(activeSecret)
    if (open.value && activeController === controller && fields.value?.secret === activeSecret)
      code.value = next
  }
  catch {
    if (activeController === controller)
      code.value = ''
  }
}
async function saveMaterial() {
  if (!props.entry || saving.value)
    return
  const activeController = controller
  const id = props.entry.id
  saving.value = true
  failure.value = ''
  try {
    await updateReloginMaterial({ id, revision: editRevision.value, password: password.value, mfaSecret: secret.value, confirmMaintenanceResolution: resolved.value })
    emit('changed')
    if (open.value && controller === activeController)
      format.value = 'two_fa'
  }
  catch (error) {
    if (open.value && controller === activeController)
      failure.value = errorMessage(error)
  }
  finally { saving.value = false }
}

watch([() => open.value, () => props.entry?.id, () => format.value], async ([visible, id, selectedFormat]) => {
  controller?.abort()
  content.value = ''
  resolved.value = false
  editRevision.value = 0
  material.value = null
  password.value = ''
  secret.value = ''
  code.value = ''
  clearInterval(interval)
  failure.value = ''
  loading.value = false
  if (!visible || !id)
    return
  const current = new AbortController()
  controller = current
  loading.value = true
  try {
    if (selectedFormat === 'edit' || selectedFormat === 'history') {
      const data = await getReloginMaterial(id, { signal: current.signal, silent: true })
      if (!current.signal.aborted) {
        material.value = data
        password.value = data.password
        secret.value = data.mfaSecret
        editRevision.value = data.revision
      }
      return
    }
    const result = await exportRelogin([id], selectedFormat === 'json' ? 'json' : 'two_fa', { signal: current.signal, silent: true })
    if (!current.signal.aborted) {
      content.value = result.files.map(file => file.content).join('\n')
      await tick()
      if (!current.signal.aborted && selectedFormat === 'two_fa')
        interval = setInterval(tick, 1000)
    }
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
  clearInterval(interval)
  content.value = ''
  resolved.value = false
  editRevision.value = 0
  material.value = null
  password.value = ''
  secret.value = ''
  code.value = ''
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
    <BaseSegmented v-model="format" label="资料格式" :options="formats" class="mb-4 w-full" />
    <p v-if="format === 'two_fa' || format === 'json'" class="text-cp-xs text-cp-text-secondary">
      {{ format === 'two_fa' ? '邮箱----密码----2FA 密钥，与导出资料一致。' : '当前可导出的账号 JSON，与批量导出使用同一份资料。' }}
    </p>
    <p v-if="loading" role="status" class="text-cp-text-tertiary">
      正在读取
    </p>
    <p v-else-if="failure" role="alert" class="text-cp-error">
      {{ failure }}
    </p>
    <div v-else-if="format === 'edit'" class="grid gap-4">
      <p class="m-0 text-cp-xs text-cp-text-secondary">
        只更新本地保存的资料，不修改上游账号
      </p>
      <BaseCheckbox v-if="material?.needsResolution" v-model="resolved" label="已在官方页面核实填写的资料，解除未完成维护状态" show-label />
      <p v-if="material?.needsResolution && !resolved" class="m-0 text-cp-xs text-cp-warning">
        普通保存会保留待核实状态及候选密钥
      </p>
      <BaseFormItem label="密码">
        <BaseInput v-model="password" type="password" autocomplete="new-password" :disabled="saving" />
      </BaseFormItem>
      <BaseFormItem label="2FA 密钥">
        <BaseInput v-model="secret" autocomplete="off" :disabled="saving" />
      </BaseFormItem>
      <div v-if="material?.pendingTotpSecret" class="rounded-cp border border-cp-border-secondary p-3">
        <span class="text-cp-xs">待激活密钥</span><code class="my-2 block break-all">{{ material.pendingTotpSecret }}</code><BaseButton size="sm" @click="copyText(material.pendingTotpSecret)">
          复制候选密钥
        </BaseButton>
      </div>
    </div>
    <div v-else-if="format === 'history'" class="grid gap-3">
      <p v-if="!material?.history.length" class="text-cp-text-secondary">
        暂无保存的历史资料
      </p>
      <div v-for="(version, index) in material?.history ?? []" :key="index" class="rounded-cp border border-cp-border-secondary p-3">
        <span class="text-cp-xs text-cp-text-secondary">{{ version.saved_at }}</span><BaseButton class="ml-3" size="sm" @click="copyText(`${entry?.email}----${version.password}----${version.mfa_secret}`)">
          复制此版本
        </BaseButton>
        <BaseButton v-if="version.pending_totp" class="mt-2" size="sm" @click="copyText(version.pending_totp.secret)">
          复制此版本的待核实密钥
        </BaseButton>
      </div>
    </div>
    <pre v-else-if="content" class="m-0 max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-cp bg-cp-fill-quaternary p-4 text-cp-sm"><code class="select-all font-mono">{{ content }}</code></pre>
    <p v-else class="text-cp-text-secondary">
      暂无可展示的资料
    </p>
    <div v-if="format === 'two_fa' && fields" class="mt-4 grid gap-3">
      <div class="flex flex-wrap gap-2">
        <BaseButton size="sm" @click="copyText(fields.email)">
          复制邮箱
        </BaseButton><BaseButton size="sm" @click="copyText(fields.password)">
          复制密码
        </BaseButton><BaseButton size="sm" @click="copyText(fields.secret)">
          复制密钥
        </BaseButton>
      </div>
      <div v-if="code" class="flex items-center justify-between rounded-cp bg-cp-fill-quaternary p-4">
        <div><code class="text-2xl tracking-widest">{{ code }}</code><span class="ml-3 text-cp-xs text-cp-text-tertiary">{{ remaining }} 秒</span></div><BaseButton size="sm" @click="copyText(code)">
          复制验证码
        </BaseButton>
      </div>
    </div>
    <template #footer>
      <BaseButton v-if="format === 'edit'" variant="primary" :loading="saving" :disabled="!password || !secret || !editRevision" @click="saveMaterial">
        保存本地资料
      </BaseButton>
      <BaseButton v-else-if="format !== 'history'" :disabled="!content || loading" @click="copyText(content)">
        {{ format === 'two_fa' ? '复制完整 2FA 资料' : '复制 JSON' }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
