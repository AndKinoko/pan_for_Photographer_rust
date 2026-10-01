<script setup>
import { computed, watch, ref, onMounted, onBeforeUnmount } from 'vue'
import { authUrl, fileIcon, formatSize, formatDate, isImageFile, withShareTicket } from '../api'
import { useTransfer } from '../composables/useTransfer'
import AppIcon from './AppIcon.vue'
import { useModal } from '../composables/useModal'

const transfer = useTransfer()

const props = defineProps({
  visible: { type: Boolean, default: false },
  /** Array of file objects to preview. */
  files: { type: Array, default: () => [] },
  /** Current index within files. */
  index: { type: Number, default: 0 },
  /**
   * `'auth'` —— 登录态（默认）。媒体地址走 authUrl，下载带 JWT。
   * `'share'` —— 公开分享页。媒体地址补分享票据，下载不带任何登录凭据。
   *
   * 两种模式的凭证处理完全不同，收在这里而不是让调用方各传一套 URL ——
   * 否则「下载按钮用了登录态地址」这种错误只会在客户点下载时才暴露。
   */
  mode: { type: String, default: 'auth' },
  /** mode 为 share 时的访问票据（无密码分享传空串即可） */
  ticket: { type: String, default: '' },
})

const isShareMode = computed(() => props.mode === 'share')

/** 统一处理媒体地址的凭证。 */
function mediaUrl(url) {
  if (!url) return url
  return isShareMode.value ? withShareTicket(url, props.ticket) : authUrl(url)
}

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

/* GIF 走原文件，<img> 天然就会播动图；其它类型维持现状（预览图优先）。
   列表页的 hover 动图见 FileCard.vue —— 这里管的是「点开大图」。 */
const isGif = computed(() => {
  const name = current.value?.name || ''
  return name.split('.').pop()?.toLowerCase() === 'gif'
})

/* 超大 GIF 播起来是持续解码，占内存且耗 CPU，可能让浏览器卡住。
   表情包通常几十 KB，这个阈值只有少数超大 GIF 会被挡在门外。 */
const GIF_MAX_BYTES = 8 * 1024 * 1024

const mediaSrc = computed(() => {
  if (!current.value) return ''
  const url =
    isGif.value && (current.value.size || 0) <= GIF_MAX_BYTES
      ? current.value.media_url
      : current.value.preview_url || current.value.media_url
  return mediaUrl(url)
})

function downloadCurrent() {
  if (!current.value) return
  // 下载进入全局下载队列（抽屉内实时进度）
  transfer.enqueueDownload({
    filename: current.value.name,
    // 公开分享页没有登录态，凭据走分享票据；登录态则走 Authorization 头。
    url: isShareMode.value
      ? withShareTicket(current.value.download_url, props.ticket)
      : current.value.download_url,
    authed: !isShareMode.value,
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
// 只处理左右方向键；Esc 交给 useModal，避免同一次按键 emit 两次 close
function onKey(e) {
  if (!props.visible) return
  if (e.key === 'ArrowLeft') prev()
  else if (e.key === 'ArrowRight') next()
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))

const previewEl = ref(null)
// 全屏预览同样是模态：焦点要锁在预览内，Esc 关闭，关闭后归还焦点
useModal(() => props.visible, { container: previewEl, onClose: () => emit('close') })

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
      <div
        v-if="visible && current"
        ref="previewEl"
        class="preview"
        role="dialog"
        aria-modal="true"
        tabindex="-1"
      >
        <header class="bar">
          <div class="title truncate" :title="current.name">
            {{ current.name }}
          </div>
          <div class="bar-actions">
            <span class="counter muted">{{ index + 1 }} / {{ files.length }}</span>
            <a
              class="btn btn-sm btn-ghost"
              href="#"
              @click.prevent="downloadCurrent"
            >
              <AppIcon name="Download" size="sm" /> 下载
            </a>
            <button class="btn-icon btn-ghost" aria-label="关闭" @click="$emit('close')">
              <AppIcon name="X" size="sm" />
            </button>
          </div>
        </header>

        <button
          v-if="hasPrev"
          class="nav prev"
          aria-label="上一个"
          @click="prev"
        >
          <AppIcon name="ChevronLeft" size="lg" />
        </button>
        <button
          v-if="hasNext"
          class="nav next"
          aria-label="下一个"
          @click="next"
        >
          <AppIcon name="ChevronRight" size="lg" />
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
                <AppIcon
                  class="fallback-icon"
                  :name="fileIcon(current.file_type, current.name)"
                  size="xl"
                />
                <p>预览加载失败</p>
                <a
                  class="btn btn-primary btn-sm"
                  href="#"
                  @click.prevent="downloadCurrent"
                >
                  <AppIcon name="Download" size="sm" /> 下载文件
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
                <AppIcon
                  class="fallback-icon"
                  :name="fileIcon(current.file_type, current.name)"
                  size="xl"
                />
                <p>视频预览加载失败</p>
                <a
                  class="btn btn-primary btn-sm"
                  href="#"
                  @click.prevent="downloadCurrent"
                >
                  <AppIcon name="Download" size="sm" /> 下载文件
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
                <AppIcon
                  class="fallback-icon"
                  :name="fileIcon(current.file_type, current.name)"
                  size="xl"
                />
                <p>音频预览加载失败</p>
                <a
                  class="btn btn-primary btn-sm"
                  href="#"
                  @click.prevent="downloadCurrent"
                >
                  <AppIcon name="Download" size="sm" /> 下载文件
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
              <AppIcon
                class="fallback-icon"
                :name="fileIcon(current.file_type, current.name)"
                size="xl"
              />
              <p>该文件类型暂不支持在线预览</p>
              <!-- 原为 :href="downloadHref"，但 downloadHref 全项目未定义，
                   这个回退下载按钮实际是坏的。改为与其它三处一致走下载队列。 -->
              <a class="btn btn-primary btn-sm" href="#" @click.prevent="downloadCurrent">
                <AppIcon name="Download" size="sm" /> 下载文件
              </a>
            </div>
          </div>
        </div>

        <footer class="info-bar">
          <span>{{ current.formatted_size || formatSize(current.size) }}</span>
          <span class="dot">·</span>
          <span>{{ formatDate(current.uploaded_at) }}</span>
          <span class="dot">·</span>
          <span class="truncate" :title="current.name">{{ current.name }}</span>
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
  z-index: var(--z-preview);
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
  /* 原为硬编码 0 10px 40px，绕过了 --shadow-* 三档令牌 */
  box-shadow: var(--shadow-lg);
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
/* 颜色继承 .fallback 的 #cfd3e0，不需要单独指定 */
.fallback .fallback-icon {
  opacity: 0.9;
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
