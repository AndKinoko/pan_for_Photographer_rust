<script setup>
import { ref, computed, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import {
  getPublicShare,
  verifySharePassword,
  publicShareDownloadUrl,
  fileIcon,
  formatSize,
  formatDate,
} from '../api'
import { useToast } from '../composables/useToast'
import { useTheme } from '../composables/useTheme'

const route = useRoute()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()

const share = ref(null)
const loading = ref(true)
const loadError = ref('')
const needsPassword = ref(false)
const password = ref('')
const verifying = ref(false)
const verified = ref(false)

const id = computed(() => route.params.id)

const isImage = computed(() => {
  if (!share.value) return false
  const ext = share.value.file_type?.toLowerCase()
  const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'bmp', 'webp', 'tiff', 'tif', 'svg', 'avif']
  const rawExts = ['nef', 'cr2', 'cr3', 'crw', 'arw', 'sr2', 'srf', 'dng', 'raf', 'orf', 'rw2', 'nrw']
  return imageExts.includes(ext) || rawExts.includes(ext)
})
const isVideo = computed(() =>
  ['mp4', 'webm', 'mov', 'ogg', 'mkv'].includes(share.value?.file_type?.toLowerCase())
)
const isAudio = computed(() =>
  ['mp3', 'wav', 'flac', 'ogg', 'aac', 'm4a'].includes(share.value?.file_type?.toLowerCase())
)
const isPdf = computed(() => share.value?.file_type?.toLowerCase() === 'pdf')
const isFolder = computed(() => share.value?.file_type === 'folder')

const downloadHref = computed(() =>
  share.value ? publicShareDownloadUrl(share.value.id) : '#'
)

const canShowMedia = computed(() => {
  if (!share.value) return false
  if (isFolder.value) return false
  return (
    isImage.value || isVideo.value || isAudio.value || isPdf.value
  )
})

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    share.value = await getPublicShare(id.value)
    if (share.value.has_password && !verified.value) {
      needsPassword.value = true
    }
  } catch (e) {
    loadError.value = e.message || '分享不存在或已失效'
  } finally {
    loading.value = false
  }
}

async function verify() {
  if (!password.value) {
    toast.warning('请输入访问密码')
    return
  }
  verifying.value = true
  try {
    await verifySharePassword(id.value, password.value)
    verified.value = true
    needsPassword.value = false
    toast.success('密码正确')
  } catch (e) {
    toast.error(e.message || '密码错误')
  } finally {
    verifying.value = false
  }
}

function onKeydown(e) {
  if (e.key === 'Enter') verify()
}

onMounted(load)
</script>

<template>
  <div class="pub">
    <button
      class="theme-fab"
      :aria-label="theme === 'dark' ? '浅色' : '深色'"
      @click="toggleTheme"
    >
      {{ theme === 'dark' ? '☀️' : '🌙' }}
    </button>

    <div class="brand">
      <span class="logo">📷</span>
      <span>摄影师网盘</span>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="card center-card">
      <div class="spinner" />
      <p class="muted">正在加载分享…</p>
    </div>

    <!-- Error / unavailable -->
    <div v-else-if="loadError" class="card center-card">
      <span class="emoji">⊘</span>
      <h2>无法访问该分享</h2>
      <p class="muted">{{ loadError }}</p>
    </div>

    <div v-else-if="!share.is_active || share.is_expired" class="card center-card">
      <span class="emoji">⌛</span>
      <h2>分享已失效</h2>
      <p class="muted">
        {{ !share.is_active ? '该分享链接已被关闭' : '该分享链接已过期' }}
      </p>
    </div>

    <!-- Password gate -->
    <div v-else-if="needsPassword" class="card center-card">
      <span class="emoji">🔒</span>
      <h2>需要访问密码</h2>
      <p class="muted">此分享受密码保护，请输入密码继续</p>
      <div class="pwd-form" @keydown="onKeydown">
        <input
          v-model="password"
          class="input"
          type="password"
          placeholder="访问密码"
          autocomplete="current-password"
          autofocus
        />
        <button class="btn btn-primary" :disabled="verifying" @click="verify">
          {{ verifying ? '验证中…' : '验证' }}
        </button>
      </div>
    </div>

    <!-- Content -->
    <div v-else class="card content-card">
      <div class="file-head">
        <span class="thumb">
          <img
            v-if="isImage && share.thumb_url"
            :src="share.thumb_url"
            alt=""
            @error="$event.target.style.display = 'none'"
          />
          <span v-else class="emoji">{{ isFolder ? '📁' : fileIcon(share.file_type, share.file_name) }}</span>
        </span>
        <div class="file-meta">
          <h1 class="file-name truncate">{{ share.file_name }}</h1>
          <div class="meta-row muted">
            <span v-if="!isFolder">{{ share.formatted_size || formatSize(share.file_size) }}</span>
            <span v-if="!isFolder" class="dot">·</span>
            <span>{{ share.owner_name }} 分享</span>
            <span class="dot">·</span>
            <span>{{ share.download_count }} 次下载</span>
          </div>
          <div class="meta-row muted">
            <span v-if="share.expires_at">到期：{{ formatDate(share.expires_at) }}</span>
            <span v-else>永久有效</span>
            <span v-if="share.max_downloads" class="dot">·</span>
            <span v-if="share.max_downloads">最多 {{ share.max_downloads }} 次</span>
          </div>
        </div>
      </div>

      <div v-if="canShowMedia && share.preview_url" class="media">
        <img
          v-if="isImage"
          :src="share.preview_url"
          :alt="share.file_name"
        />
        <video v-else-if="isVideo" :src="share.preview_url" controls />
        <audio v-else-if="isAudio" :src="share.preview_url" controls />
        <iframe
          v-else-if="isPdf"
          :src="share.preview_url"
          class="pdf"
          title="PDF 预览"
        />
      </div>

      <div v-else-if="isFolder" class="folder-note state">
        <span class="emoji">📁</span>
        <p>这是一个文件夹分享</p>
      </div>

      <div v-else class="fallback state">
        <span class="emoji">{{ fileIcon(share.file_type, share.file_name) }}</span>
        <p>该文件类型暂不支持在线预览</p>
      </div>

      <div class="actions">
        <a
          v-if="!isFolder"
          class="btn btn-primary download"
          :href="downloadHref"
          :download="share.file_name"
        >
          ⬇️ 下载文件
        </a>
        <span v-else class="muted">文件夹暂不支持打包下载</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pub {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 32px 16px 64px;
  background: radial-gradient(
      circle at 50% 0%,
      var(--primary-soft),
      transparent 60%
    ),
    var(--bg);
}
.theme-fab {
  position: fixed;
  top: 16px;
  right: 16px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  box-shadow: var(--shadow);
  font-size: 1.1rem;
}
.brand {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 700;
  color: var(--text-heading);
  margin-bottom: 24px;
}
.brand .logo {
  font-size: 1.6rem;
}
.card {
  width: min(94vw, 720px);
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}
.center-card {
  padding: 48px 28px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}
.center-card .emoji {
  font-size: 3rem;
}
.center-card h2 {
  font-size: 1.2rem;
}
.pwd-form {
  display: flex;
  gap: 10px;
  margin-top: 12px;
  width: min(100%, 360px);
}
.pwd-form .input {
  flex: 1 1 auto;
}

.content-card {
  padding: 20px;
}
.file-head {
  display: flex;
  gap: 14px;
  align-items: center;
  margin-bottom: 16px;
}
.thumb {
  width: 64px;
  height: 64px;
  flex: 0 0 64px;
  border-radius: var(--radius);
  background: var(--bg-hover);
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}
.thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.thumb .emoji {
  font-size: 1.8rem;
}
.file-meta {
  flex: 1 1 auto;
  min-width: 0;
}
.file-name {
  font-size: 1.15rem;
}
.meta-row {
  font-size: 0.82rem;
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 2px;
}
.dot {
  opacity: 0.5;
}
.media {
  background: var(--bg);
  border-radius: var(--radius);
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 16px;
}
.media img {
  max-width: 100%;
  max-height: 70vh;
  display: block;
}
.media video {
  width: 100%;
  max-height: 70vh;
}
.pdf {
  width: 100%;
  height: 70vh;
  border: none;
  border-radius: var(--radius);
  background: #fff;
}
.fallback {
  padding: 48px 24px;
}
.folder-note {
  padding: 40px 24px;
}
.actions {
  display: flex;
  justify-content: center;
  gap: 12px;
  margin-top: 8px;
}
.download {
  min-width: 200px;
}
.state .emoji {
  font-size: 2.6rem;
}
@media (max-width: 600px) {
  .pwd-form {
    flex-direction: column;
  }
}
</style>
