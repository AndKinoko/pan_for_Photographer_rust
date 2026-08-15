<script setup>
defineProps({
  selectedCount: { type: Number, default: 0 },
  fileSelectedCount: { type: Number, default: 0 },
  folderSelectedCount: { type: Number, default: 0 },
})
const emit = defineEmits(['move', 'copy', 'delete', 'share', 'clear'])
</script>

<template>
  <Transition name="slide-up">
    <div v-if="selectedCount > 0" class="batch-bar">
      <div class="left">
        <span class="count">已选 {{ selectedCount }} 项</span>
        <button class="btn btn-sm btn-ghost" @click="emit('clear')">
          取消选择
        </button>
      </div>
      <div class="actions">
        <button class="btn btn-sm" @click="emit('move')">
          📁 移动
        </button>
        <button class="btn btn-sm" @click="emit('copy')">
          📋 复制
        </button>
        <button
          v-if="fileSelectedCount > 0"
          class="btn btn-sm"
          @click="emit('share')"
        >
          🔗 分享
        </button>
        <button class="btn btn-sm btn-danger" @click="emit('delete')">
          🗑️ 删除
        </button>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.batch-bar {
  position: fixed;
  left: 50%;
  bottom: 20px;
  transform: translateX(-50%);
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  width: min(94vw, 720px);
  padding: 10px 14px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: var(--shadow-lg);
}
.left {
  display: flex;
  align-items: center;
  gap: 10px;
}
.count {
  font-weight: 600;
  color: var(--text-heading);
}
.actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.slide-up-enter-active {
  transition: transform 0.25s cubic-bezier(0.18, 0.89, 0.32, 1.28),
    opacity 0.2s ease;
}
.slide-up-leave-active {
  transition: transform 0.18s ease, opacity 0.18s ease;
}
.slide-up-enter-from {
  transform: translate(-50%, 30px);
  opacity: 0;
}
.slide-up-leave-to {
  transform: translate(-50%, 30px);
  opacity: 0;
}
@media (max-width: 560px) {
  .batch-bar {
    flex-direction: column;
    align-items: stretch;
  }
  .actions {
    justify-content: center;
  }
}
</style>
