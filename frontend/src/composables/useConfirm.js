import { reactive } from 'vue'

/**
 * Promise-based confirm/prompt dialog system.
 * `confirm(options)` returns a Promise<boolean|string>:
 *   - without `inputLabel`: resolves true / false
 *   - with `inputLabel`: resolves to the entered string or null when cancelled
 *
 * ConfirmDialog.vue renders the shared state.
 */

export const state = reactive({
  open: false,
  title: '确认',
  message: '',
  confirmText: '确定',
  cancelText: '取消',
  variant: 'primary', // 'primary' | 'danger'
  inputLabel: null,
  inputValue: '',
  inputPlaceholder: '',
  inputType: 'text',
  loading: false,
})

let resolver = null

function reset() {
  state.open = false
  state.title = '确认'
  state.message = ''
  state.confirmText = '确定'
  state.cancelText = '取消'
  state.variant = 'primary'
  state.inputLabel = null
  state.inputValue = ''
  state.inputPlaceholder = ''
  state.inputType = 'text'
  state.loading = false
}

/**
 * Show a confirm dialog. Options:
 * { title, message, confirmText, cancelText, variant,
 *   inputLabel, inputValue, inputPlaceholder, inputType }
 */
export function confirm(options = {}) {
  reset()
  Object.assign(state, {
    title: options.title ?? '确认',
    message: options.message ?? '',
    confirmText: options.confirmText ?? '确定',
    cancelText: options.cancelText ?? '取消',
    variant: options.variant ?? 'primary',
    inputLabel: options.inputLabel ?? null,
    inputValue: options.inputValue ?? '',
    inputPlaceholder: options.inputPlaceholder ?? '',
    inputType: options.inputType ?? 'text',
  })
  state.open = true
  return new Promise((resolve) => {
    resolver = resolve
  })
}

/** Resolve the dialog from outside (used by ConfirmDialog.vue). */
export function resolveConfirm(value) {
  if (resolver) {
    resolver(value)
    resolver = null
  }
  reset()
}

export function useConfirm() {
  return { state, confirm, resolveConfirm }
}

export default useConfirm
