import { reactive } from 'vue'

/**
 * Lightweight global toast notification system.
 * `toasts` is a module-level singleton shared across the app.
 * Toast.vue renders this list.
 */

let _id = 0

export const toasts = reactive([])

/**
 * Push a toast notification.
 * @param {string} message
 * @param {'success'|'error'|'warning'|'info'} type
 * @param {number} duration milliseconds (0 = sticky)
 */
export function toast(message, type = 'info', duration = 3200) {
  const id = ++_id
  toasts.push({ id, message, type })
  if (duration > 0) {
    setTimeout(() => dismissToast(id), duration)
  }
  return id
}

export function dismissToast(id) {
  const idx = toasts.findIndex((t) => t.id === id)
  if (idx !== -1) toasts.splice(idx, 1)
}

export function clearToasts() {
  toasts.splice(0, toasts.length)
}

/** Convenience helpers usable anywhere without instantiation. */
export const useToast = () => ({
  toast,
  dismiss: dismissToast,
  clear: clearToasts,
  success: (m, d) => toast(m, 'success', d),
  error: (m, d) => toast(m, 'error', d ?? 5000),
  warning: (m, d) => toast(m, 'warning', d),
  info: (m, d) => toast(m, 'info', d),
})

export default useToast
