<script setup>
import { computed, watch, onBeforeUnmount } from 'vue'
import AppIcon from './AppIcon.vue'

/* 批量条是 fixed 定位，会浮在内容最后一行的上面——用户选中若干文件后，
   最后一行恰好被挡住，而且卡片本身可点，容易误触。

   这里把「批量条占多高」按需写进 --batch-bar-offset，由页面消费：
   内容区写 padding-bottom: var(--batch-bar-offset) 即可。
   「占多高」放在这里维护而不是各页面写死，是为了避免实现重复后漂移
   （本项目已有四套对话框、三份扩展名白名单、两份 .err 字号的先例）。 */
function syncOffset(count) {
  if (typeof document === 'undefined') return
  document.documentElement.style.setProperty(
    '--batch-bar-offset',
    count > 0 ? 'var(--batch-bar-space)' : '0px'
  )
}

const props = defineProps({
  selectedCount: { type: Number, default: 0 },
  fileSelectedCount: { type: Number, default: 0 },
  folderSelectedCount: { type: Number, default: 0 },
  // 当前文件夹下的可选总数（文件 + 文件夹），用于全选状态判定
  selectableCount: { type: Number, default: 0 },
  // 是否已全部选中
  isAllSelected: { type: Boolean, default: false },
  // 还有多少项未加载（分页后「全选」覆盖不到的部分）。
  // 全选的真实作用域是「已加载的项」，把差额告诉用户，
  // 比让按钮名不副实地暗示「整个目录」要好。
  unloadedCount: { type: Number, default: 0 },
})

watch(
  () => props.selectedCount,
  (n) => syncOffset(n),
  { immediate: true }
)
// 卸载时必须归零，否则离开页面后内容底部会永久留着一段空白
onBeforeUnmount(() => syncOffset(0))

/* 全选按钮的提示文案。分页之后「全选」只能覆盖已加载的项，
   所以把未加载的数量说出来，并给出下一步动作。 */
const selectAllHint = computed(() => {
  if (props.isAllSelected) return '取消全选'
  if (props.unloadedCount > 0) {
    return `全选已显示的 ${props.selectableCount} 项（还有 ${props.unloadedCount} 项未加载，先点「加载更多」）`
  }
  return '全选当前文件夹下的所有文件和文件夹'
})

const emit = defineEmits([
  'move',
  'copy',
  'delete',
  'share',
  'download',
  'clear',
  'select-all',
  'invert',
])
</script>

<template>
  <Transition name="slide-up">
    <div v-if="selectedCount > 0" class="batch-bar">
      <div class="left">
        <span class="count">已选 {{ selectedCount }} 项</span>
        <button
          class="btn btn-sm btn-ghost"
          :title="selectAllHint"
          @click="emit('select-all')"
        >
          <AppIcon :name="isAllSelected ? 'CheckSquare' : 'Square'" size="sm" />
          {{ isAllSelected ? '已全选' : '全选' }}
        </button>
        <button
          v-if="selectableCount > 0 && selectedCount > 0 && !isAllSelected"
          class="btn btn-sm btn-ghost"
          title="反选（已选中的取消，未选中的选中）"
          @click="emit('invert')"
        >
          <AppIcon name="Repeat" size="sm" />
          反选
        </button>
        <button class="btn btn-sm btn-ghost" @click="emit('clear')">
          取消选择
        </button>
      </div>
      <div class="actions">
        <button class="btn btn-sm" @click="emit('move')">
          <AppIcon name="FolderInput" size="sm" />
          移动
        </button>
        <button class="btn btn-sm" @click="emit('copy')">
          <AppIcon name="Copy" size="sm" />
          复制
        </button>
        <button
          v-if="fileSelectedCount > 0"
          class="btn btn-sm"
          @click="emit('share')"
        >
          <AppIcon name="Link" size="sm" />
          分享
        </button>
        <button
          v-if="fileSelectedCount > 0"
          class="btn btn-sm"
          @click="emit('download')"
        >
          <AppIcon name="Download" size="sm" />
          下载
        </button>
        <button class="btn btn-sm btn-danger" @click="emit('delete')">
          <AppIcon name="Trash2" size="sm" />
          删除
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
  z-index: var(--z-batchbar);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  width: min(94vw, 800px);
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
