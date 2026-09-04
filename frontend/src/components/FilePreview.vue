<script setup>
import { computed, watch, ref, onMounted, onBeforeUnmount } from 'vue'
import { authUrl, fileIcon, formatSize, formatDate, isImageFile } from '../api'
import { useTransfer } from '../composables/useTransfer'

const transfer = useTransfer()

const props = defineProps({
  visible: { type: Boolean, default: false },
  /** Array of file objects to preview. */
  files: { type: Array, default: () => [] },
  /** Current index within files. */
  index: { type: Number, default: 0 },
})

const emit = defineEmits(['close', 'update:index'])

const current = computed(() => props.files[props.index] || null)
const hasPrev = computed(() => props.index > 0)
const hasNext = computed(() => props.index < props.files.length - 1)

const isImg = computed(
  () => current.value && isImageFile(current.value.file_type, current.value.name)
)
const isVideo = computed(() => {
  const ext = (current.value?.name || '').split('.').pop()?.toLowerCase()
  return ['mp4', 'mov', 'avi', 'mkv', 'webm', 'flv', 'wmv', 'm4v'].includes(ext)
})
const isAudio = computed(() => {
  const ext = (current.value?.name || '').split('.').pop()?.toLowerCase()
  return ['mp3', 'wav', 'flac', 'ogg', 'aac', 'm4a'].includes(ext)
})
const isPdf = computed(() => current.value?.file_type?.toLowerCase() === 'pdf')

const mediaSrc = computed(() => {
  if (!current.value) return ''
  const url = current.value.preview_url || current.value.media_url
  return authUrl(url)
})

function downloadCurrent() {
  if (!current.value) return
  // 下载进入全局下载队列（抽屉内实时进度）
  transfer.enqueueDownload({
    filename: current.value.name,
    url: current.value.download_url,
    authed: true,
  })
}

const imgLoaded = ref(false)
const imgError = ref(false)
const mediaError = ref(false)
watch(
  () => props.index,
  () => {
    imgLoaded.value = false
    imgError.value = false
    mediaError.value = false
  }
)

function prev() {
  if (hasPrev.value) emit('update:index', props.index - 1)
}
function next() {
  if (hasNext.value) emit('update:index', props.index + 1)
}
function onImgError() {
  imgError.value = true
  imgLoaded.value = true
}
function onKey(e) {
  if (!props.visible) return
  if (e.key === 'Escape') emit('close')
  else if (e.key === 'ArrowLeft') prev()
  else if (e.key === 'ArrowRight') next()
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))

watch(
  () => props.visible,
  (v) => {
    document.body.style.overflow = v ? 'hidden' : ''
    if (v) {
      imgLoaded.value = false
      imgError.value = false
      mediaError.value = false
    }
  }
)
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="visible && current" class="preview" role="dialog" aria-modal="true">
        <header class="bar">
          <div class="title truncate">
            {{ current.name }}
          </div>
          <div class="bar-actions">
            <span class="counter muted">{{ index + 1 }} / {{ files.length }}</span>
            <a
              class="btn btn-sm btn-ghost"
              href="#"
              @click.prevent="downloadCurrent"
            >
              ⬇️ 下载
            </a>
            <button class="btn-icon btn-ghost" aria-label="关闭" @click="$emit('close')">
              ✕
            </button>
          </div>
        </header>

        <button
          v-if="hasPrev"
          class="nav prev"
          aria-label="上一个"
          @click="prev"
        >
          ‹
        </button>
        <button
          v-if="hasNext"
          class="nav next"
          aria-label="下一个"
          @click="next"
        >
          ›
        </button>

        <div class="stage" @click.self="$emit('close')">
          <div class="viewer">
            <template v-if="isImg">
              <img
                v-show="imgLoaded && !imgError"
                :src="mediaSrc"
                :alt="current.name"
                @load="imgLoaded = true"
                @error="onImgError"
              />
              <span v-show="!imgLoaded && !imgError" class="spinner" />
              <div v-if="imgError" class="fallback">
                <span class="emoji">{{ fileIcon(current.file_type, current.name) }}</span>
                <p>预览加载失败</p>
                <a
                  class="btn btn-primary btn-sm"
                  href="#"
                  @click.prevent="downloadCurrent"
                >
                  ⬇️ 下载文件
                </a>
              </div>
            </template>
            <template v-else-if="isVideo">
              <video
                v-if="!mediaError"
                :src="mediaSrc"
                controls
                autoplay
                @error="mediaError = true"
              />
              <div v-else class="fallback">
                <span class="emoji">{{ fileIcon(current.file_type, current.name) }}</span>
                <p>视频预览加载失败</p>
                <a
                  class="btn btn-primary btn-sm"
                  href="#"
                  @click.prevent="downloadCurrent"
                >
                  ⬇️ 下载文件
                </a>
              </div>
            </template>
            <template v-else-if="isAudio">
              <audio
                v-if="!mediaError"
                :src="mediaSrc"
                controls
                autoplay
                @error="mediaError = true"
              />
              <div v-else class="fallback">
                <span class="emoji">{{ fileIcon(current.file_type, current.name) }}</span>
                <p>音频预览加载失败</p>
                <a
                  class="btn btn-primary btn-sm"
                  href="#"
                  @click.prevent="downloadCurrent"
                >
                  ⬇️ 下载文件
                </a>
              </div>
            </template>
            <iframe
              v-else-if="isPdf"
              :src="mediaSrc"
              class="pdf"
              title="PDF 预览"
              sandbox="allow-same-origin allow-downloads"
            />
            <div v-else class="fallback">
              <span class="emoji">{{ fileIcon(current.file_type, current.name) }}</span>
              <p>该文件类型暂不支持在线预览</p>
              <a
                class="btn btn-primary btn-sm"
                :href="downloadHref"
                :download="current.name"
              >
                ⬇️ 下载文件
              </a>
            </div>
          </div>
        </div>

        <footer class="info-bar">
          <span>{{ current.formatted_size || formatSize(current.size) }}</span>
          <span class="dot">·</span>
          <span>{{ formatDate(current.uploaded_at) }}</span>
          <span class="dot">·</span>
          <span class="truncate">{{ current.name }}</span>
        </footer>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.preview {
  position: fixed;
  inset: 0;
  background: rgba(8, 10, 18, 0.92);
  z-index: 8000;
  display: flex;
  flex-direction: column;
}
.bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 16px;
  background: rgba(0, 0, 0, 0.3);
  color: #fff;
}
.title {
  font-size: 0.95rem;
  font-weight: 600;
  color: #fff;
  max-width: 60vw;
}
.bar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.counter {
  font-size: 0.82rem;
  margin-right: 4px;
}
.bar .btn {
  color: #fff;
  text-decoration: none;
}
.bar .btn-ghost:hover {
  background: rgba(255, 255, 255, 0.15);
}

.stage {
  flex: 1 1 auto;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  min-height: 0;
}
.viewer {
  max-width: 100%;
  max-height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
}
.viewer img {
  max-width: 100%;
  max-height: 80vh;
  object-fit: contain;
  border-radius: 4px;
  box-shadow: 0 10px 40px rgba(0, 0, 0, 0.5);
}
.viewer video {
  max-width: 100%;
  max-height: 80vh;
}
.pdf {
  width: min(90vw, 900px);
  height: 80vh;
  border: none;
  border-radius: 6px;
  background: #fff;
}
.fallback {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
  color: #cfd3e0;
}
.fallback .emoji {
  font-size: 4rem;
}

.nav {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  width: 52px;
  height: 52px;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.12);
  color: #fff;
  font-size: 2rem;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background-color 0.15s ease;
}
.nav:hover {
  background: rgba(255, 255, 255, 0.25);
}
.nav.prev {
  left: 16px;
}
.nav.next {
  right: 16px;
}

.info-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 16px;
  background: rgba(0, 0, 0, 0.3);
  color: #cfd3e0;
  font-size: 0.82rem;
}
.dot {
  opacity: 0.5;
}
.spinner {
  width: 40px;
  height: 40px;
  border-color: rgba(255, 255, 255, 0.25);
  border-top-color: #fff;
}

@media (max-width: 768px) {
  .nav {
    width: 44px;
    height: 44px;
    font-size: 1.6rem;
  }
  .nav.prev {
    left: 8px;
  }
  .nav.next {
    right: 8px;
  }
  .title {
    max-width: 50vw;
  }
}
</style>
