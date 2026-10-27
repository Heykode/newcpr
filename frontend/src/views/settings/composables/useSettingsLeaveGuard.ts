import type { Ref } from 'vue'
import { useConfirmDialog, useEventListener } from '@vueuse/core'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { onBeforeRouteLeave, onBeforeRouteUpdate } from 'vue-router'

export function useSettingsLeaveGuard(hasChanges: Readonly<Ref<boolean>>) {
  const showLeaveConfirm = shallowRef(false)
  const { reveal, confirm: confirmLeave, cancel } = useConfirmDialog(showLeaveConfirm)
  let pending: Promise<boolean> | undefined
  function mayLeave() {
    if (!hasChanges.value)
      return true
    pending ??= reveal().then(result => !result.isCanceled).finally(() => {
      pending = undefined
    })
    return pending
  }
  onBeforeRouteLeave(mayLeave)
  // 本地设置分区复用路由组件；切分区也可能卸载尚未保存的备份表单。
  onBeforeRouteUpdate((to, from) => to.path === from.path ? true : mayLeave())
  watch(showLeaveConfirm, (open) => {
    if (!open)
      cancel()
  })
  useEventListener(window, 'beforeunload', (event: BeforeUnloadEvent) => {
    if (hasChanges.value) {
      event.preventDefault()
      event.returnValue = ''
    }
  })
  onScopeDispose(cancel)
  return { showLeaveConfirm, confirmLeave }
}
