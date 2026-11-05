import { useClipboard } from '@vueuse/core'

import { toast } from '@/components/base/BaseToast'
import { t } from '@/i18n'

interface CopyTextOptions {
  successText?: string
  notifyWhenEmpty?: boolean
}

export function useCopyText() {
  const { copy } = useClipboard({ legacy: true })

  return async function copyText(value: string, options: CopyTextOptions = {}) {
    if (!value) {
      if (options.notifyWhenEmpty)
        toast.error(t('common.errors.nothingToCopy'))
      return
    }
    try {
      await copy(value)
      toast.success(options.successText ?? t('common.feedback.copied'))
    }
    catch {
      toast.error(t('common.errors.copy'))
    }
  }
}
