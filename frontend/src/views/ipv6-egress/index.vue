<script setup lang="ts">
import type { Ipv6EgressAddress, Ipv6EgressConfig } from '@/api/modules/ipv6-egress'
import { computed, onMounted, ref, shallowRef, watch } from 'vue'
import { expandIpv6Egress, getIpv6Egress, ipv6EgressModes, updateIpv6Egress } from '@/api/modules/ipv6-egress'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'

const saved = shallowRef<Ipv6EgressConfig | null>(null)
const addresses = ref<Ipv6EgressAddress[]>([])
const defaultMode = shallowRef('unchanged')
const rangeStart = shallowRef('')
const rangeEnd = shallowRef('')
const currentPage = shallowRef(1)
const pageSize = shallowRef(20)
const visibleAddresses = computed(() => addresses.value.slice(
  (currentPage.value - 1) * pageSize.value,
  currentPage.value * pageSize.value,
))
const action = useAsyncAction()
const busy = action.loading
const columns = defineTableColumns<Ipv6EgressAddress>([
  { key: 'address', label: 'IPv6 地址', kind: 'custom', size: '3xl' },
  { key: 'enabled', label: '启用', kind: 'custom', size: 'sm', align: 'center' },
  { key: 'actions', label: '操作', kind: 'actions', size: 'sm', align: 'center' },
])

function hydrate(config: Ipv6EgressConfig) {
  saved.value = config
  addresses.value = config.addresses.map(address => ({ ...address }))
  defaultMode.value = config.defaultMode
}

watch([() => addresses.value.length, pageSize], () => {
  currentPage.value = Math.min(currentPage.value, Math.max(1, Math.ceil(addresses.value.length / pageSize.value)))
})

async function reload() {
  await action.run(async () => hydrate(await getIpv6Egress()), { errorText: 'IPv6 配置加载失败' })
}

async function addRange() {
  await action.run(async () => {
    const expanded = await expandIpv6Egress({
      start: rangeStart.value.trim(),
      end: rangeEnd.value.trim() || rangeStart.value.trim(),
    })
    const known = new Set(addresses.value.map(address => address.address))
    const additions = expanded.filter(address => !known.has(address.address))
    if (addresses.value.length + additions.length > 4096)
      throw new Error('地址池最多支持 4096 个地址')
    addresses.value.push(...additions)
    rangeStart.value = ''
    rangeEnd.value = ''
    toast.success(`已添加 ${additions.length} 个禁用地址；保存后生效`)
  }, { errorText: 'IPv6 范围不合法' })
}

async function save() {
  const current = saved.value
  if (!current)
    return
  await action.run(async () => {
    const result = await updateIpv6Egress({
      revision: current.revision,
      defaultMode: defaultMode.value,
      addresses: addresses.value,
    })
    hydrate(result.config)
    toast.success('IPv6 配置已保存；启用地址已通过本机绑定检查')
  }, { errorText: 'IPv6 配置保存失败；若版本冲突请刷新后重试' })
}

function toggleAddress(id: string, enabled: boolean) {
  addresses.value = addresses.value.map(address => address.id === id ? { ...address, enabled } : address)
}

onMounted(reload)
</script>

<template>
  <div class="grid gap-5">
    <BasePageHeader title="IPv6 出口" description="仅配置 CPR 的本地连接源地址，不修改操作系统网络接口。">
      <template #actions>
        <BaseButton :loading="busy" :disabled="!saved" @click="save">
          保存配置
        </BaseButton>
      </template>
    </BasePageHeader>
    <BaseCard>
      <div class="grid gap-4">
        <p class="m-0 text-cp-sm text-cp-text-secondary">
          默认不启用。只有已配置到本机的地址才能启用；本机绑定检查不代表已通过互联网连通性测试。
          本页管理本地 IPv6 源地址；侧栏“代理管理”维护转发代理，两者不是同一地址池。
          账号不能同时使用转发代理和 IPv6 策略；“不启用 IPv6 策略”保留原直连或代理设置。
        </p>
        <BaseFormItem label="全局默认策略">
          <BaseSelect v-model="defaultMode" :options="ipv6EgressModes" :disabled="busy || !saved" />
        </BaseFormItem>
        <p class="m-0 text-cp-xs text-cp-text-tertiary">
          账号可在“编辑账号”中覆盖。固定地址历史在删除、重新导入账号后保留；历史地址被禁用或移除时不会自动改绑。
          复用 / 新建是连接策略，不是 HTTP/2 开关；不改变 WebSocket 与 HTTP 的协议选择。
          每次独立连接只选择一次源地址，不逐帧轮换。连续会话优先原 WebSocket 连接，新建模式也不覆盖严格续接、不打断进行中的响应。
        </p>
      </div>
    </BaseCard>
    <BaseCard>
      <div class="grid gap-4">
        <div class="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
          <BaseFormItem label="地址 / 范围起点">
            <BaseInput v-model="rangeStart" :disabled="busy" placeholder="2001:db8::10" aria-label="IPv6 范围起点" />
          </BaseFormItem>
          <BaseFormItem label="范围终点（可选）">
            <BaseInput v-model="rangeEnd" :disabled="busy" placeholder="留空只添加一个地址" aria-label="IPv6 范围终点" />
          </BaseFormItem>
          <BaseButton :disabled="busy || !rangeStart.trim()" @click="addRange">
            添加禁用地址
          </BaseButton>
        </div>
        <p class="m-0 text-cp-xs text-cp-text-tertiary">
          共 {{ addresses.length }} / 4096 个地址。新账号优先分配给当前有效绑定数最少的地址；
          数量相同按表格顺序，全部有绑定时仍允许共享。
        </p>
        <BaseTable class="h-96!" :columns="columns" :rows="visibleAddresses" :loading="busy" empty-text="尚未配置 IPv6 地址">
          <template #address="{ row }">
            <span class="font-mono">{{ row.address }}</span>
          </template>
          <template #enabled="{ row }">
            <BaseSwitch :model-value="row.enabled" :disabled="busy" label="启用 IPv6 地址" @update:model-value="toggleAddress(row.id, $event)" />
          </template>
          <template #actions="{ row }">
            <BaseButton variant="secondary" :disabled="busy" @click="addresses = addresses.filter(address => address.id !== row.id)">
              移除
            </BaseButton>
          </template>
        </BaseTable>
        <BaseTablePagination
          :pagination="{ currentPage, pageSize, total: addresses.length }"
          :loading="busy"
          @page-change="currentPage = $event"
          @page-size-change="pageSize = $event; currentPage = 1"
        />
        <div class="flex justify-end gap-3">
          <BaseButton variant="secondary" :disabled="busy" @click="reload">
            重新加载
          </BaseButton>
        </div>
      </div>
    </BaseCard>
  </div>
</template>
