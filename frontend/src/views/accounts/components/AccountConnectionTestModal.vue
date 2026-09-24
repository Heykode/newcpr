<script setup lang="ts">
import type { useAccountConnectionTest } from '../composables/useAccountConnectionTest'

import { RefreshCw } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseScrollbar from '@/components/base/BaseScrollbar.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import AccountIdentityCell from './AccountIdentityCell.vue'
import AccountStatusBadge from './AccountStatusBadge/index.vue'

type ConnectionTest = ReturnType<typeof useAccountConnectionTest>

defineProps<{
  account: ConnectionTest['testingAccount']['value']
  status: ConnectionTest['connectionTestStatus']['value']
  model: string
  upstreamResponseModel?: string | null
  logs: ConnectionTest['connectionTestLogs']['value']
  error: string
  startedAt: string
  finishedAt: string
  durationMs: number | null
  loadingModels: boolean
  refreshingModels: boolean
  modelOptions: ConnectionTest['connectionTestModelOptions']['value']
  statusView: ConnectionTest['connectionTestStatusView']['value']
}>()

const emit = defineEmits<{
  test: []
  refreshModels: []
}>()
const open = defineModel<boolean>({ default: false })
const selectedModel = defineModel<string>('selectedModel', { required: true })
const stream = defineModel<boolean>('stream', { required: true })
const prompt = defineModel<string>('prompt', { required: true })

function connectionLogClass(tone: string) {
  if (tone === 'success')
    return 'text-cp-success-text'
  if (tone === 'danger')
    return 'text-cp-error-text'
  if (tone === 'info')
    return 'text-cp-info-text'
  return 'text-cp-text-secondary'
}
</script>

<template>
  <BaseModal
    v-model="open"
    title="测试连接"
    description="验证账号凭据、身份绑定与上游模型端点是否可用"
    tone="info"
    size="lg"
  >
    <div v-if="account" class="flex flex-col gap-4">
      <section
        class="flex items-center justify-between gap-4 rounded-cp-card bg-cp-fill-quaternary px-4 py-3"
      >
        <AccountIdentityCell :account="account" size="lg" show-plan />
        <AccountStatusBadge
          :status="account.status"
          :error-reason="account.errorReason"
          :error-message="account.errorMessage"
          :next-refresh-at="account.nextRefreshAt"
          variant="pill"
        />
      </section>

      <section class="rounded-cp-card bg-cp-fill-quaternary px-4 py-3">
        <div class="grid gap-3">
          <div class="flex min-h-8 items-center justify-between gap-3">
            <span class="text-cp-sm font-heavy text-cp-text-quaternary">
              测试模型
            </span>
            <BaseIconButton
              variant="ghost"
              size="sm"
              label="刷新上游模型"
              :loading="refreshingModels"
              :disabled="status === 'running' || loadingModels"
              @click="emit('refreshModels')"
            >
              <template #loading>
                <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
              </template>
              <RefreshCw class="size-3.5" />
            </BaseIconButton>
          </div>
          <BaseSelect
            v-model="selectedModel"
            aria-label="测试模型"
            :options="modelOptions"
            :disabled="status === 'running' || loadingModels || refreshingModels"
            :placeholder="loadingModels ? '加载模型中...' : '选择上游模型'"
            empty-text="上游没有返回模型"
          />
          <div class="grid gap-3 sm:grid-cols-2">
            <BaseFormItem label="测试接口">
              <BaseInput
                model-value="Responses"
                aria-label="测试接口"
                readonly
                disabled
              />
            </BaseFormItem>
            <BaseFormItem label="返回方式">
              <BaseSelect
                :model-value="stream ? 'stream' : 'non-stream'"
                aria-label="返回方式"
                :options="[
                  { label: '流式返回', value: 'stream' },
                  { label: '完整返回', value: 'non-stream' },
                ]"
                :disabled="status === 'running'"
                @update:model-value="stream = $event === 'stream'"
              />
            </BaseFormItem>
          </div>
          <BaseFormItem label="测试提示词">
            <BaseTextarea
              v-model="prompt"
              aria-label="测试提示词"
              :rows="3"
              :maxlength="4096"
              :disabled="status === 'running'"
              placeholder="例如：Reply with exactly OK."
            />
          </BaseFormItem>
          <p v-if="error" role="alert" class="m-0 text-cp-sm wrap-break-word text-cp-error-text">
            {{ error }}
          </p>
        </div>
      </section>

      <section class="rounded-cp-card bg-cp-fill-quaternary p-4">
        <div class="flex items-start justify-between gap-4">
          <div class="flex min-w-0 items-start gap-3">
            <span
              class="inline-flex size-10 shrink-0 items-center justify-center rounded-lg"
              :class="statusView.badge"
            >
              <component
                :is="statusView.icon"
                class="size-5"
                :class="[statusView.iconClass, status === 'running' ? 'animate-pulse' : '']"
              />
            </span>
            <div class="min-w-0">
              <p class="m-0 text-[16px] font-heavy text-cp-text">
                {{ statusView.label }}
              </p>
              <p
                class="mt-1.5 mb-0 text-cp leading-normal font-emphasis text-cp-text-secondary"
              >
                {{ statusView.description }}
              </p>
            </div>
          </div>
          <span
            class="inline-flex h-7 shrink-0 items-center rounded-full px-2.5 text-cp-sm font-heavy"
            :class="statusView.badge"
          >
            {{ status === 'running' ? '检测中' : statusView.label }}
          </span>
        </div>

        <div class="mt-4 grid gap-3 sm:grid-cols-3">
          <div class="rounded-lg bg-cp-bg-container px-3 py-2.5">
            <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
              开始时间
            </p>
            <p class="mt-1.5 mb-0 font-mono text-cp-sm font-emphasis text-cp-text">
              {{ startedAt || '-' }}
            </p>
          </div>
          <div class="rounded-lg bg-cp-bg-container px-3 py-2.5">
            <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
              完成时间
            </p>
            <p class="mt-1.5 mb-0 font-mono text-cp-sm font-emphasis text-cp-text">
              {{ finishedAt || '-' }}
            </p>
          </div>
          <div class="rounded-lg bg-cp-bg-container px-3 py-2.5">
            <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
              响应耗时
            </p>
            <p class="mt-1.5 mb-0 font-mono text-cp-sm font-emphasis text-cp-text">
              {{ durationMs !== null ? `${durationMs}ms` : '-' }}
            </p>
          </div>
        </div>

        <div class="mt-3 rounded-lg bg-cp-bg-container px-3 py-2.5">
          <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
            测试模型
          </p>
          <p
            class="mt-1.5 mb-0 truncate font-mono text-cp-sm font-emphasis text-cp-text"
            :title="model || '-'"
          >
            {{ model || '-' }}
          </p>
          <p
            class="mt-3 mb-0 text-cp-xs font-heavy text-cp-text-quaternary"
            title="仅为上游响应声明，不代表独立型号鉴定"
          >
            返回模型（上游声明）
          </p>
          <p
            class="mt-1.5 mb-0 break-all font-mono text-cp-sm font-emphasis text-cp-text"
            :title="upstreamResponseModel || '未返回'"
          >
            {{ upstreamResponseModel || '未返回' }}
          </p>
        </div>

        <div class="mt-3 rounded-lg bg-cp-bg-container px-3 py-2.5">
          <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
            事件轨迹
          </p>
          <BaseScrollbar max-height="260px">
            <div class="pt-2">
              <div v-if="logs.length === 0" class="text-cp-sm font-emphasis text-cp-text-quaternary">
                -
              </div>
              <div v-else class="flex flex-col gap-1.5">
                <div
                  v-for="item in logs"
                  :key="item.key"
                  class="grid grid-cols-[54px_minmax(0,1fr)] gap-2 text-cp-sm leading-[1.45] font-emphasis"
                >
                  <span class="font-mono text-cp-text-quaternary">{{ item.time }}</span>
                  <div class="min-w-0">
                    <p
                      class="m-0 wrap-break-word"
                      :class="connectionLogClass(item.tone)"
                    >
                      {{ item.text }}
                    </p>
                    <div v-if="item.detail" class="mt-2 rounded-lg bg-cp-fill-quaternary px-3 py-2">
                      <p
                        v-if="item.tone === 'danger'"
                        class="mt-0 mb-2 text-cp-xs font-heavy text-cp-text-quaternary"
                      >
                        原始诊断
                      </p>
                      <BaseScrollbar max-height="138px">
                        <div>
                          <pre
                            class="m-0 whitespace-pre-wrap wrap-break-word font-mono text-cp-xs leading-[1.6] font-emphasis text-cp-text"
                            v-text="item.detail"
                          />
                        </div>
                      </BaseScrollbar>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </BaseScrollbar>
        </div>
      </section>
    </div>

    <template #footer>
      <BaseButton variant="secondary" @click="open = false">
        关闭
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="status === 'running'"
        :disabled="!account || loadingModels || refreshingModels || !selectedModel"
        @click="emit('test')"
      >
        {{ logs.length > 0 || error ? '重新测试' : '开始测试' }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
