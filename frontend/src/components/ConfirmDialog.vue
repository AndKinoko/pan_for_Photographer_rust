<script setup>
import { ref, watch, nextTick } from 'vue'
import { state, resolveConfirm } from '../composables/useConfirm'
import { useModal } from '../composables/useModal'

const inputEl = ref(null)
const dialogEl = ref(null)

// Reset a local mirror so two-way editing stays smooth.
const localInput = ref('')

/* 焦点陷阱 + Esc 统一交给 useModal。
   原先无输入框的确认弹窗（删除、清空回收站）收不到 Esc/Enter——
   容器 div 没有 tabindex，焦点进不来，@keydown 就永远不触发；
   只有带输入框的重命名弹窗因为输入框被自动聚焦才碰巧能用。 */
useModal(() => state.open, {
  container: dialogEl,
  onClose: cancel,
  initialFocus: () => {
    if (!state.inputLabel) return null
    inputEl.value?.select()
    return inputEl.value
  },
})

watch(
  () => state.open,
  (open) => {
    if (open) localInput.value = state.inputValue
  }
)

function cancel() {
  resolveConfirm(state.inputLabel ? null : false)
}
function confirm() {
  resolveConfirm(state.inputLabel ? localInput.value : true)
}
function onBackdrop() {
  cancel()
}
// 只处理 Enter；Esc 由 useModal 统一处理，避免同一次按键触发两次 cancel
function onKeydown(e) {
  if (e.key === 'Enter') {
    e.preventDefault()
    confirm()
  }
}
</script>

<template>
  <Transition name="fade">
    <div
      v-if="state.open"
      class="overlay"
      @mousedown.self="onBackdrop"
    >
      <div
        ref="dialogEl"
        class="dialog"
        role="dialog"
        aria-modal="true"
        tabindex="-1"
        @keydown="onKeydown"
      >
        <h3 class="title">{{ state.title }}</h3>
        <p v-if="state.message" class="message">{{ state.message }}</p>

        <div v-if="state.inputLabel" class="field" style="margin-top: 6px">
          <label>{{ state.inputLabel }}</label>
          <input
            ref="inputEl"
            v-model="localInput"
            class="input"
            :type="state.inputType"
            :placeholder="state.inputPlaceholder"
          />
        </div>

        <div class="actions">
          <button class="btn btn-ghost" @click="cancel">
            {{ state.cancelText }}
          </button>
          <button
            class="btn"
            :class="state.variant === 'danger' ? 'btn-danger' : 'btn-primary'"
            @click="confirm"
          >
            {{ state.confirmText }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.overlay {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
  backdrop-filter: blur(2px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  z-index: var(--z-confirm);
}
.dialog {
  width: min(92vw, 420px);
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 22px;
  outline: none;
}
.title {
  font-size: 1.1rem;
  color: var(--text-heading);
  margin-bottom: 8px;
}
.message {
  color: var(--text);
  font-size: 0.92rem;
  white-space: pre-wrap;
  word-break: break-word;
}
.actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 18px;
}
</style>
