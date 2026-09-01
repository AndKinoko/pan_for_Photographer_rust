<script setup>
import { ref, computed, onUnmounted } from 'vue'
import http, { formatSize } from '../api'
import { useToast } from '../composables/useToast'

const props = defineProps({
  folderId: { type: [Number, String, null], default: null },
  autoStart: { type: Boolean, default: true },
  compact: { type: Boolean, default: false },
})

const emit = defineEmits(['uploaded', 'all-done'])
const toast = useToast()

const dragOver = ref(false)
const inputEl = ref(null)
const queue = ref([]) // { id, name, size, status, progress, error }
let seq = 0
let activeController = null // 当前进行中上传的 AbortController
let unmounted = false // 组件已卸载，禁止继续启动上传

// 组件卸载时中止进行中的上传，避免静默占用网络与后端连接
onUnmounted(() => {
  unmounted = true
  activeController?.abort()
})

/** 失败自动重试：网络错误/5xx 退避重试（最多 2 次重试），4xx、超时与主动取消直接落败。 */
async function withRetry(fn, attempts = 3) {
  let lastErr
  for (let i = 1; i <= attempts; i++) {
    try {
      return await fn()
    } catch (err) {
      lastErr = err
      const status = err?.status
      const timedOut = err?.code === 'ECONNABORTED'
      const canceled = err?.code === 'ERR_CANCELED' // 主动 abort，不重试
      // 可重试：无状态码（网络中断）或 5xx；不重试 4xx、超时与取消
      const retriable = !canceled && (status ? status >= 500 : !timedOut)
      if (i === attempts || !retriable) throw err
      await new Promise((resolve) => setTimeout(resolve, 400 * i))
    }
  }
  throw lastErr
}

const hasActive = computed(() =>
  queue.value.some((q) => q.status === 'uploading' || q.status === 'pending')
)
const overall = computed(() => {
  if (!queue.value.length) return 0
  const total = queue.value.reduce((s, q) => s + q.size, 0)
  const loaded = queue.value.reduce(
    (s, q) => s + (q.size * q.progress) / 100,
    0
  )
  return total ? Math.round((loaded / total) * 100) : 0
})

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
function addFiles(fileList) {
  if (!fileList || !fileList.length) return
  const items = Array.from(fileList).map((f) => ({
    id: ++seq,
    name: f.name,
    size: f.size,
    status: 'pending',
    progress: 0,
    error: '',
    _file: f,
  }))
  queue.value.push(...items)
  if (props.autoStart) startNext()
}

async function uploadOne(item) {
  item.status = 'uploading'
  item.progress = 0
  const form = new FormData()
  if (props.folderId != null && props.folderId !== '') {
    form.append('folder_id', String(props.folderId))
  }
  form.append('file', item._file, item.name)
  const controller = new AbortController()
  activeController = controller
  try {
    // 网络错误/5xx 会自动退避重试；4xx 与超时直接落败并继续下一个
    const data = await withRetry(() =>
      http.post('/api/files/upload', form, {
        headers: { 'Content-Type': 'multipart/form-data' },
        signal: controller.signal,
        onUploadProgress: (e) => {
          if (e.total) item.progress = Math.round((e.loaded / e.total) * 100)
        },
      })
    )
    // data = { files: [FileInfo], errors: [String], count }
    const errs = data?.errors || []
    if (errs.length && (!data?.files || !data.files.length)) {
      item.status = 'error'
      item.error = errs[0]
      toast.error(`${item.name}: ${errs[0]}`)
    } else {
      item.status = 'done'
      item.progress = 100
      if (data?.files?.[0]) emit('uploaded', data.files[0])
      if (errs.length) toast.warning(`${item.name}: ${errs[0]}`)
    }
  } catch (err) {
    item.status = 'error'
    item.error = err?.status === 413 ? '文件超过大小限制' : err.message || '上传失败'
    toast.error(`${item.name}: ${item.error}`)
  } finally {
    activeController = null
    item._file = null
    startNext()
  }
}

function startNext() {
  if (unmounted) return // 组件已卸载，不再启动新上传
  const next = queue.value.find((q) => q.status === 'pending')
  if (next) {
    uploadOne(next)
  } else if (!hasActive.value) {
    emit('all-done')
  }
}

function removeItem(id) {
  const i = queue.value.findIndex((q) => q.id === id)
  if (i !== -1) queue.value.splice(i, 1)
}
function clearDone() {
  queue.value = queue.value.filter(
    (q) => q.status !== 'done' && q.status !== 'error'
  )
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
          <p class="muted">支持多文件同时上传</p>
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

    <Transition name="fade">
      <div v-if="queue.length" class="queue card">
        <div class="queue-head">
          <span>上传队列 ({{ queue.length }})</span>
          <div class="row">
            <span v-if="hasActive" class="muted small">总进度 {{ overall }}%</span>
            <button class="btn btn-sm btn-ghost" :disabled="hasActive" @click="clearDone">
              清除已完成
            </button>
          </div>
        </div>
        <ul>
          <li v-for="q in queue" :key="q.id" :class="q.status">
            <span class="q-icon">
              <span v-if="q.status === 'uploading'" class="spinner sm" />
              <span v-else-if="q.status === 'done'">✅</span>
              <span v-else-if="q.status === 'error'" :title="q.error">⚠️</span>
              <span v-else>•</span>
            </span>
            <div class="q-main">
              <div class="q-name truncate">
                {{ q.name }}
                <span class="muted small">· {{ formatSize(q.size) }}</span>
              </div>
              <div class="bar-track">
                <div
                  class="bar-fill"
                  :class="q.status"
                  :style="{ width: q.progress + '%' }"
                />
              </div>
              <div v-if="q.error" class="q-error truncate">{{ q.error }}</div>
            </div>
            <button
              v-if="!hasActive"
              class="q-remove"
              aria-label="移除"
              @click="removeItem(q.id)"
            >
              ✕
            </button>
          </li>
        </ul>
      </div>
    </Transition>
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
.small {
  font-size: 0.78rem;
}

.queue {
  padding: 12px 14px;
}
.queue-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 0.88rem;
  font-weight: 600;
  color: var(--text-heading);
  margin-bottom: 8px;
}
ul {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
li {
  display: flex;
  align-items: center;
  gap: 10px;
}
.q-icon {
  flex: 0 0 auto;
  width: 22px;
  display: flex;
  justify-content: center;
}
.q-main {
  flex: 1 1 auto;
  min-width: 0;
}
.q-name {
  font-size: 0.85rem;
  color: var(--text-heading);
}
.bar-track {
  height: 6px;
  background: var(--bg-hover);
  border-radius: 999px;
  overflow: hidden;
  margin-top: 4px;
}
.bar-fill {
  height: 100%;
  background: var(--primary);
  border-radius: 999px;
  transition: width 0.2s ease;
}
.bar-fill.done {
  background: var(--success);
}
.bar-fill.error {
  background: var(--danger);
}
.q-error {
  color: var(--danger);
  font-size: 0.75rem;
  margin-top: 2px;
}
.q-remove {
  flex: 0 0 auto;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  color: var(--text-muted);
}
.q-remove:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
}
.spinner.sm {
  width: 16px;
  height: 16px;
  border-width: 2px;
}
</style>
