<script setup>
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { useTransfer } from '../composables/useTransfer'

const props = defineProps({
  folderId: { type: [Number, String, null], default: null },
  compact: { type: Boolean, default: false },
})

const transfer = useTransfer()

const dragging = ref(false)
const inputEl = ref(null)
let dragDepth = 0

function hasFiles(e) {
  if (!e.dataTransfer) return false
  const types = Array.from(e.dataTransfer.types || [])
  if (!types.includes('Files')) return false
  // 页面内部拖拽（拖动缩略图/预览图）：Chromium 会把 <img> 当作虚拟文件
  // （types 带 Files），但必然同时携带 text/html 或 text/uri-list 标记；
  // 操作系统文件拖拽只带 Files。据此把内部拖拽排除在外。
  if (types.includes('text/html') || types.includes('text/uri-list')) {
    return false
  }
  return true
}

function addFiles(fileList) {
  if (!fileList || !fileList.length) return
  transfer.enqueueUpload(fileList, props.folderId)
}

/* 全屏隐形拖放层：监听 window 级拖拽事件，拖入时显示提示蒙层 */
function onDragEnter(e) {
  if (!hasFiles(e)) return
  dragDepth++
  dragging.value = true
}
function onDragOver(e) {
  if (!hasFiles(e)) return
  e.preventDefault()
}
function onDragLeave(e) {
  if (!hasFiles(e)) return
  dragDepth = Math.max(0, dragDepth - 1)
  if (dragDepth === 0) dragging.value = false
}
function onDrop(e) {
  if (!hasFiles(e)) return
  e.preventDefault()
  dragDepth = 0
  dragging.value = false
  addFiles(e.dataTransfer?.files)
}

function openPicker() {
  inputEl.value?.click()
}
function onPick(e) {
  addFiles(e.target.files)
  e.target.value = ''
}

onMounted(() => {
  window.addEventListener('dragenter', onDragEnter)
  window.addEventListener('dragover', onDragOver)
  window.addEventListener('dragleave', onDragLeave)
  window.addEventListener('drop', onDrop)
})
onBeforeUnmount(() => {
  window.removeEventListener('dragenter', onDragEnter)
  window.removeEventListener('dragover', onDragOver)
  window.removeEventListener('dragleave', onDragLeave)
  window.removeEventListener('drop', onDrop)
})

defineExpose({ addFiles, openPicker })
</script>

<template>
  <div class="upload-zone" :class="{ compact }">
    <Teleport to="body">
      <Transition name="dz-fade">
        <div v-if="dragging" class="drop-overlay">
          <div class="drop-hint">
            <span class="dz-icon">⬆️</span>
            <strong>松开鼠标，上传到当前文件夹</strong>
          </div>
        </div>
      </Transition>
    </Teleport>
    <input
      ref="inputEl"
      type="file"
      multiple
      hidden
      @change="onPick"
    />
  </div>
</template>

<style scoped>
.drop-overlay {
  position: fixed;
  inset: 0;
  z-index: 125;
  background: var(--primary-soft);
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
}
.drop-hint {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 28px 36px;
  border: 2px dashed var(--primary);
  border-radius: var(--radius);
  background: var(--bg-elevated);
  color: var(--text-heading);
  font-size: 1.05rem;
  box-shadow: var(--shadow-lg);
}
.dz-icon {
  font-size: 1.8rem;
}
.dz-fade-enter-active,
.dz-fade-leave-active {
  transition: opacity 0.15s ease;
}
.dz-fade-enter-from,
.dz-fade-leave-to {
  opacity: 0;
}
</style>
