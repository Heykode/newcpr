<script setup lang="ts">
import type { AccountImportTask, AccountImportTaskDetail } from '@/api'
import { ListTodo } from '@lucide/vue'
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { taskLabel, taskTime } from './presenter'
import TaskDetail from './TaskDetail.vue'

const props = defineProps<{
  tasks: AccountImportTask[]
  selectedId: string
  detail: AccountImportTaskDetail | null
  loading: boolean
  stopping: boolean
  error: string
  stopError: string
}>()
const emit = defineEmits<{ select: [id: string], refresh: [], stop: [], viewAccounts: [] }>()
const open = defineModel<boolean>({ required: true })
const taskOptions = computed(() => props.tasks.map(task => ({
  value: task.taskId,
  label: taskTime(task.createdAt),
  description: `${task.total} 个条目 · ${taskLabel(task)}`,
})))
</script>

<template>
  <BaseModal v-model="open" title="导入任务" size="lg">
    <div v-if="error" role="alert" class="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-cp bg-cp-warning-container p-3 text-xs text-cp-warning-on-container">
      <span class="min-w-0 flex-1 wrap-anywhere">进度暂未更新：{{ error }}</span>
      <BaseButton size="sm" :loading="loading" @click="emit('refresh')">
        刷新进度
      </BaseButton>
    </div>
    <p v-if="stopError" role="alert" class="mb-4 text-xs wrap-anywhere text-cp-error-text">
      停止请求未确认：{{ stopError }}
    </p>
    <BaseEmpty v-if="!tasks.length" class="min-h-[min(20rem,50dvh)] content-center" :icon="ListTodo" :title="loading ? '正在读取导入任务' : error ? '导入任务读取失败' : '暂无导入任务'" />
    <div v-else class="min-h-[min(20rem,50dvh)] min-w-0">
      <div class="mb-5 grid gap-2">
        <span class="text-xs font-medium text-cp-text-secondary">近期批次（{{ tasks.length }}）</span>
        <BaseSelect
          :model-value="selectedId"
          :options="taskOptions"
          size="sm"
          aria-label="切换导入任务"
          class="w-full min-w-0"
          @update:model-value="emit('select', $event)"
        />
      </div>
      <TaskDetail v-if="detail" :task="detail" :stopping="stopping" @stop="emit('stop')" @view-accounts="emit('viewAccounts')" />
      <BaseEmpty v-else class="min-h-[min(16rem,40dvh)] content-center" :title="loading ? '正在读取条目结果' : '暂无条目结果'" surface="none" />
    </div>
    <p class="mt-4 mb-0 text-xs text-cp-text-tertiary">
      临时记录 · 完成后保留 1 小时 · 服务重启清空 · 不含 OAuth 授权
    </p>
    <template #footer>
      <BaseButton @click="open = false">
        关闭
      </BaseButton>
    </template>
  </BaseModal>
</template>
