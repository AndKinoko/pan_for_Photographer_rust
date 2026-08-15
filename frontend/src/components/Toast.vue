<script setup>
import { toasts, dismissToast } from '../composables/useToast'

const ICONS = {
  success: '✅',
  error: '⛔',
  warning: '⚠️',
  info: 'ℹ️',
}
</script>

<template>
  <div class="toast-wrap" aria-live="polite" aria-atomic="true">
    <TransitionGroup name="toast">
      <div
        v-for="t in toasts"
        :key="t.id"
        class="toast"
        :class="`t-${t.type}`"
        role="status"
      >
        <span class="icon">{{ ICONS[t.type] || 'ℹ️' }}</span>
        <span class="msg">{{ t.message }}</span>
        <button
          class="close"
          aria-label="关闭"
          @click="dismissToast(t.id)"
        >
          ✕
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-wrap {
  position: fixed;
  top: 16px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 9999;
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: min(92vw, 420px);
  pointer-events: none;
}
.toast {
  pointer-events: auto;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-left-width: 4px;
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-lg);
  color: var(--text-heading);
  font-size: 0.9rem;
}
.t-success {
  border-left-color: var(--success);
}
.t-error {
  border-left-color: var(--danger);
}
.t-warning {
  border-left-color: var(--warning);
}
.t-info {
  border-left-color: var(--info);
}
.icon {
  font-size: 1.05rem;
}
.msg {
  flex: 1 1 auto;
  word-break: break-word;
}
.close {
  flex: 0 0 auto;
  width: 26px;
  height: 26px;
  border-radius: 6px;
  color: var(--text-muted);
  font-size: 0.75rem;
}
.close:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
}

.toast-enter-active {
  transition: transform 0.25s cubic-bezier(0.18, 0.89, 0.32, 1.28),
    opacity 0.2s ease;
}
.toast-leave-active {
  transition: transform 0.2s ease, opacity 0.2s ease;
  position: absolute;
  width: 100%;
}
.toast-enter-from {
  transform: translateY(-16px);
  opacity: 0;
}
.toast-leave-to {
  transform: translateY(-10px);
  opacity: 0;
}
</style>
