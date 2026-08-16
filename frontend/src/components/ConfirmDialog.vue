<script setup>
import { ref, watch, nextTick } from 'vue'
import { state, resolveConfirm } from '../composables/useConfirm'

const inputEl = ref(null)

// Reset a local mirror so two-way editing stays smooth.
const localInput = ref('')

watch(
  () => state.open,
  async (open) => {
    if (open) {
      localInput.value = state.inputValue
      await nextTick()
      if (state.inputLabel) {
        inputEl.value?.focus()
        inputEl.value?.select()
      }
    }
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
function onKeydown(e) {
  if (e.key === 'Escape') {
    e.preventDefault()
    cancel()
  } else if (e.key === 'Enter') {
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
        class="dialog"
        role="dialog"
        aria-modal="true"
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
  z-index: 9000;
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
