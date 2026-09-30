<script setup>
import { toasts, dismissToast } from '../composables/useToast'
import AppIcon from './AppIcon.vue'

// 状态图标名（src/icons.js 注册表）。原先存的是 emoji——颜色由字体决定，
// 主题令牌与暗色模式都改不动它。
const ICONS = {
  success: 'Check',
  error: 'CircleX',
  warning: 'TriangleAlert',
  info: 'Info',
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
        <AppIcon class="icon" :name="ICONS[t.type] || 'Info'" size="sm" />
        <span class="msg">{{ t.message }}</span>
        <button
          class="close"
          aria-label="关闭"
          @click="dismissToast(t.id)"
        >
          <AppIcon name="X" size="sm" />
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
  z-index: var(--z-toast);
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
/* 图标跟随状态语义色。这是换成 SVG 之后才做得到的事——
   emoji 的颜色由字体固定，:root 令牌与暗色模式都影响不到它。 */
.icon {
  display: block;
  color: var(--text-muted);
}
.t-success .icon {
  color: var(--success);
}
.t-error .icon {
  color: var(--danger);
}
.t-warning .icon {
  color: var(--warning);
}
.t-info .icon {
  color: var(--info);
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
