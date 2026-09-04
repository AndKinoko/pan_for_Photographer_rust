<script setup>
import { ref } from 'vue'
import { useTransfer } from '../composables/useTransfer'

const props = defineProps({
  folderId: { type: [Number, String, null], default: null },
  compact: { type: Boolean, default: false },
})

const transfer = useTransfer()

const dragOver = ref(false)
const inputEl = ref(null)

/** 把选中的文件加入全局上传队列（抽屉内展示进度） */
function addFiles(fileList) {
  if (!fileList || !fileList.length) return
  transfer.enqueueUpload(fileList, props.folderId)
}

function openPicker() {
  inputEl.value?.click()
}
function onPick(e) {
  addFiles(e.target.files)
  e.target.value = ''
}
function onDrop(e) {
  e.stopPropagation()
  dragOver.value = false
  addFiles(e.dataTransfer?.files)
}

defineExpose({ addFiles })
</script>

<template>
  <div class="upload-zone" :class="{ compact }">
    <div
      class="dropzone"
      :class="{ over: dragOver }"
      role="button"
      tabindex="0"
      @click="openPicker"
      @keydown.enter.prevent="openPicker"
      @dragover.prevent="dragOver = true"
      @dragleave.prevent="dragOver = false"
      @drop.prevent="onDrop"
    >
      <div class="dz-inner">
        <span class="dz-icon">⬆️</span>
        <div>
          <strong>点击或拖拽文件到此处上传</strong>
          <p class="muted">上传进度请在「传输」查看</p>
        </div>
      </div>
      <input
        ref="inputEl"
        type="file"
        multiple
        hidden
        @change="onPick"
      />
    </div>
  </div>
</template>

<style scoped>
.upload-zone {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.dropzone {
  border: 2px dashed var(--border-strong);
  border-radius: var(--radius);
  background: var(--bg-elevated);
  padding: 22px 18px;
  text-align: center;
  cursor: pointer;
  transition: border-color 0.18s ease, background-color 0.18s ease;
  display: flex;
  align-items: center;
  justify-content: center;
}
.dropzone:hover,
.dropzone.over {
  border-color: var(--primary);
  background: var(--primary-soft);
}
.dz-inner {
  display: flex;
  align-items: center;
  gap: 12px;
  color: var(--text-heading);
}
.dz-icon {
  font-size: 1.8rem;
}
.upload-zone.compact .dropzone {
  padding: 14px;
}
.upload-zone.compact .dz-inner {
  font-size: 0.88rem;
}
.upload-zone.compact .dz-icon {
  font-size: 1.4rem;
}
</style>