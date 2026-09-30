<script setup lang="ts">
import { Save } from '@lucide/vue'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'

import AdminApiKeyCard from './components/AdminApiKeyCard.vue'
import AdminPasswordCard from './components/AdminPasswordCard.vue'
import SettingsBackupSection from './components/backup/SettingsBackupSection.vue'
import SettingsCleanupSection from './components/cleanup/SettingsCleanupSection.vue'
import ClientVersionSettings from './components/client-version/index.vue'
import ExcelSettingsCard from './components/ExcelSettingsCard.vue'
import ModelAliasesCard from './components/ModelAliasesCard.vue'
import NotificationChannelsCard from './components/NotificationChannelsCard.vue'
import OutboundUserAgentCard from './components/OutboundUserAgentCard.vue'
import RotationStrategyCard from './components/RotationStrategyCard.vue'
import RuntimeSettingsCard from './components/RuntimeSettingsCard.vue'
import { useAdminApiKey } from './composables/useAdminApiKey'
import { useSettingsForm } from './composables/useSettingsForm'
import { rotationOptions } from './constants'

const route = useRoute()
const router = useRouter()
const configurationScope = ref('common')

type SettingsSection = 'runtime' | 'backup' | 'cleanup'

const section = computed<SettingsSection>(() =>
  route.name === 'settings-cleanup' ? 'cleanup' : route.name === 'settings-backup' ? 'backup' : 'runtime',
)

function switchSection(value: string): void {
  const paths: Record<SettingsSection, string> = {
    runtime: '/settings',
    backup: '/settings/backup',
    cleanup: '/settings/cleanup',
  }
  const nextSection: SettingsSection = value === 'cleanup' ? 'cleanup' : value === 'backup' ? 'backup' : 'runtime'
  void router.push(paths[nextSection])
}

const {
  loading,
  saving,
  error,
  form,
  excelImages,
  mappings,
  addMapping,
  updateMapping,
  removeMapping,
  refreshMarginSecondsValue,
  refreshConcurrencyValue,
  maxConcurrentPerAccountValue,
  requestIntervalMsValue,
  responsesMaxDecompressedBodyBytesValue,
  minCodexDesktopVersionError,
  minCodexCliVersionError,
  saveSettings,
  loadSettings,
} = useSettingsForm()

const {
  loading: adminKeyLoading,
  regenerating: adminKeyRegenerating,
  deleting: adminKeyDeleting,
  showDeleteModal: showDeleteAdminKeyModal,
  generatedKey: generatedAdminApiKey,
  status: adminApiKeyStatus,
  regenerate: handleRegenerateAdminApiKey,
  remove: handleDeleteAdminApiKey,
  copyGeneratedKey: copyAdminApiKey,
  loadStatus: loadAdminApiKeyStatus,
} = useAdminApiKey()

// 只在切到对应分区时才加载各自接口，避免打开备份页也请求运行设置数据。
watch(
  section,
  (value) => {
    if (value === 'runtime') {
      void loadSettings()
      void loadAdminApiKeyStatus()
    }
  },
  { immediate: true },
)
</script>

<template>
  <div class="w-full">
    <BasePageHeader title="系统设置" description="管理运行参数、管理员凭据与备份配置" />

    <div class="mt-4 flex min-h-cp-control flex-wrap items-center justify-between gap-3">
      <BaseSegmented
        :model-value="section"
        label="设置分区"
        class="bg-(--cp-input-bg)!"
        :options="[
          { label: '运行设置', value: 'runtime' },
          { label: '备份', value: 'backup' },
          { label: '日志清理', value: 'cleanup' },
        ]"
        @update:model-value="switchSection"
      />
      <BaseButton
        v-if="section === 'runtime'"
        variant="primary"
        :loading="saving"
        :disabled="loading"
        @click="saveSettings"
      >
        <template #icon>
          <Save class="size-4" />
        </template>
        {{ saving ? '保存中...' : '保存全部设置' }}
      </BaseButton>
    </div>

    <div v-if="section === 'runtime'" class="mt-5 grid w-full gap-5">
      <BaseSegmented
        v-model="configurationScope"
        label="配置范围"
        :options="[
          { label: '通用与 Codex 配置', value: 'common' },
          { label: 'Excel 配置', value: 'excel' },
        ]"
      />
      <div v-show="configurationScope === 'common'" class="grid min-w-0 gap-5" role="region" aria-label="通用与 Codex 配置">
        <AdminPasswordCard />
        <AdminApiKeyCard
          :status="adminApiKeyStatus"
          :loading="adminKeyLoading"
          :regenerating="adminKeyRegenerating"
          :deleting="adminKeyDeleting"
          :generated-key="generatedAdminApiKey"
          @regenerate="handleRegenerateAdminApiKey"
          @request-delete="showDeleteAdminKeyModal = true"
          @copy="copyAdminApiKey"
        />

        <NotificationChannelsCard />

        <RuntimeSettingsCard
          v-model:max-concurrent-per-account="maxConcurrentPerAccountValue"
          v-model:refresh-margin-seconds="refreshMarginSecondsValue"
          v-model:refresh-concurrency="refreshConcurrencyValue"
          v-model:request-interval-ms="requestIntervalMsValue"
          v-model:responses-max-decompressed-body-bytes="responsesMaxDecompressedBodyBytesValue"
          v-model:disable-fast="form.disableFast"
          v-model:request-tuning="form.requestTuning"
          :disabled="loading || saving"
        />

        <ClientVersionSettings
          v-model:min-codex-desktop-version="form.minCodexDesktopVersion"
          v-model:min-codex-cli-version="form.minCodexCliVersion"
          :loading="loading"
          :desktop-error="minCodexDesktopVersionError"
          :cli-error="minCodexCliVersionError"
        />

        <OutboundUserAgentCard />

        <ModelAliasesCard
          :mappings="mappings"
          :loading="loading"
          :error="error"
          @add-mapping="addMapping"
          @update-mapping="updateMapping"
          @remove-mapping="removeMapping"
        />

        <RotationStrategyCard v-model="form.rotationStrategy" v-model:smart-scheduling="form.requestTuning.smartScheduling" :options="rotationOptions" :disabled="loading || saving" />

        <BaseConfirmModal
          v-model="showDeleteAdminKeyModal"
          title="删除管理员 API Key"
          description="删除后外部系统将无法继续使用该 Key 调用管理接口"
          destructive
          confirm-text="确认删除"
          :loading="adminKeyDeleting"
          @confirm="handleDeleteAdminApiKey"
        >
          <p class="m-0">
            确定要删除当前管理员 API Key 吗？此操作会立即生效
          </p>
        </BaseConfirmModal>
      </div>
      <div v-show="configurationScope === 'excel'" class="min-w-0" role="region" aria-label="Excel 配置">
        <ExcelSettingsCard
          v-model:models="form.excelDefaultModels"
          v-model:request-tuning="form.requestTuning"
          :images="excelImages"
          :disabled="loading || saving"
        />
      </div>
    </div>

    <div v-else-if="section === 'cleanup'" class="mt-5">
      <SettingsCleanupSection />
    </div>
    <div v-else class="mt-5">
      <SettingsBackupSection :active="section === 'backup'" />
    </div>
  </div>
</template>
