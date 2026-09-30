<script setup>
import { ref, computed, onMounted, nextTick } from 'vue'
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
import AppIcon from '../components/AppIcon.vue'
import PasswordInput from '../components/PasswordInput.vue'
import { useTheme } from '../composables/useTheme'
import { useTransfer } from '../composables/useTransfer'

const route = useRoute()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()
const transfer = useTransfer()

const share = ref(null)
const loading = ref(true)
const loadError = ref('')
const needsPassword = ref(false)
const password = ref('')
const pwdError = ref('')
const pwdEl = ref(null)
const verifying = ref(false)
const verified = ref(false)
const ticket = ref('')

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

function downloadShared() {
  // 纵深防御：即使按钮因模板调整意外可见，未过密码校验也不放行。
  if (needsPassword.value && !verified.value) {
    toast.error('请先输入访问密码')
    return
  }
  if (!share.value) return
  // 下载进入全局下载队列（公开分享使用访问凭证，无用户 token）
  transfer.enqueueDownload({
    filename: share.value.file_name,
    url: publicShareDownloadUrl(share.value.id, ticket.value),
    authed: false,
  })
}

const canShowMedia = computed(() => {
  if (!share.value) return false
  if (isFolder.value) return false
  // 显式排除「有密码但未验证」，而不是依赖模板的 v-else-if 分支顺序。
  //
  // 安全性此前完全靠 <template> 里 `v-else-if="needsPassword"` 恰好排在
  // 媒体区之前：一旦有人调整分支顺序、或 needsPassword 因某种原因没被置位
  // （例如后端漏返 has_password），密码门的媒体区就会直接裸奔。
  // 把判断收敛到计算属性里，改模板顺序不会再影响安全性。
  if (needsPassword.value && !verified.value) return false
  return (
    isImage.value || isVideo.value || isAudio.value || isPdf.value
  )
})

/** 同理收紧下载入口：未通过密码校验时不给下载按钮。 */
const canDownload = computed(() => !!share.value && !isFolder.value)

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    share.value = await getPublicShare(id.value)
    // 后端一定会返回 has_password；这里对「字段缺失」也保守处理成
    // 「需要密码」——宁可多问一次口令，也不能因为后端改了字段名就静默
    // 放行。verified 为真时（刷新后重验通过）不受影响。
    if (!verified.value) {
      const pwdFlag = share.value?.has_password
      needsPassword.value = pwdFlag === undefined ? true : !!pwdFlag
    }
  } catch (e) {
    loadError.value = e.message || '分享不存在或已失效'
  } finally {
    loading.value = false
  }
}

/** 把错误同时放到字段旁边（可读、可复制、不会 3 秒后消失）并聚焦回输入框 */
async function failVerify(msg) {
  pwdError.value = msg
  await nextTick()
  pwdEl.value?.focus()
}

async function verify() {
  pwdError.value = ''
  if (!password.value) return failVerify('请输入访问密码')
  verifying.value = true
  try {
    const data = await verifySharePassword(id.value, password.value)
    // 后端签发与本次分享绑定的短时效访问凭证，媒体与下载请求需携带它
    ticket.value = data?.ticket || ''
    verified.value = true
    needsPassword.value = false
    // 后端对**有密码的分享**不下发 preview_url / thumb_url（少给一个能被
    // 利用的环节），所以验码后必须重新拉一次详情才能拿到媒体地址。
    // 不重新拉的话，验完口令仍然看不到预览。
    await load()
    toast.success('密码正确')
  } catch (e) {
    // 原先只弹 toast——3 秒后消失，且不在字段旁，读屏用户也接不到
    failVerify(e.message || '密码错误')
  } finally {
    verifying.value = false
  }
}

/** 给媒体/缩略图 URL 追加访问凭证（受密码保护时）。 */
function withTicket(url) {
  if (!url || !ticket.value) return url
  const sep = url.includes('?') ? '&' : '?'
  return `${url}${sep}ticket=${encodeURIComponent(ticket.value)}`
}

function onKeydown(e) {
  if (e.key !== 'Enter') return
  // 焦点在按钮上时交给按钮自己处理，否则按 Enter 会提交两次
  if (e.target instanceof HTMLButtonElement) return
  verify()
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
      <AppIcon :name="theme === 'dark' ? 'Sun' : 'Moon'" size="sm" />
    </button>

    <div class="brand">
      <AppIcon class="logo" name="Camera" size="lg" />
      <span>摄影师网盘</span>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="card center-card">
      <div class="spinner" />
      <p class="muted">正在加载分享…</p>
    </div>

    <!-- Error / unavailable -->
    <div v-else-if="loadError" class="card center-card">
      <AppIcon class="center-icon" name="Ban" size="xl" />
      <h1>无法访问该分享</h1>
      <p class="muted">{{ loadError }}</p>
    </div>

    <div v-else-if="!share.is_active || share.is_expired" class="card center-card">
      <AppIcon class="center-icon" name="Clock" size="xl" />
      <h1>分享已失效</h1>
      <p class="muted">
        {{ !share.is_active ? '该分享链接已被关闭' : '该分享链接已过期' }}
      </p>
    </div>

    <!-- Password gate -->
    <div v-else-if="needsPassword" class="card center-card">
      <AppIcon class="center-icon" name="Lock" size="xl" />
      <h1>需要访问密码</h1>
      <p class="muted">此分享受密码保护，请输入密码继续</p>
      <div class="pwd-form" @keydown="onKeydown">
        <label class="sr-only" for="share-password">访问密码</label>
        <PasswordInput
          id="share-password"
          ref="pwdEl"
          v-model="password"
          placeholder="访问密码"
          autocomplete="off"
          autofocus
          :aria-invalid="pwdError ? 'true' : undefined"
          :aria-describedby="pwdError ? 'share-password-error' : undefined"
        />
        <button class="btn btn-primary" :disabled="verifying" @click="verify">
          {{ verifying ? '验证中…' : '验证' }}
        </button>
      </div>
      <!-- 原为 autocomplete="current-password"：会让密码管理器把用户自己的
           站点密码填进「分享访问密码」，两者是不同的凭据。
           这里是一次性凭证，用 off 更准确。 -->
      <p v-if="pwdError" id="share-password-error" class="err" role="alert">
        {{ pwdError }}
      </p>
    </div>

    <!-- Content -->
    <div v-else class="card content-card">
      <div class="file-head">
        <span class="thumb">
          <img
            v-if="isImage && share.thumb_url"
            :src="withTicket(share.thumb_url)"
            alt=""
            @error="$event.target.style.display = 'none'"
          />
          <AppIcon
            v-else
            class="file-icon"
            :name="isFolder ? 'Folder' : fileIcon(share.file_type, share.file_name)"
            size="lg"
          />
        </span>
        <div class="file-meta">
          <h1 class="file-name truncate" :title="share.file_name">
            {{ share.file_name }}
          </h1>
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
          :src="withTicket(share.preview_url)"
          :alt="share.file_name"
        />
        <video v-else-if="isVideo" :src="withTicket(share.preview_url)" controls />
        <audio v-else-if="isAudio" :src="withTicket(share.preview_url)" controls />
        <iframe
          v-else-if="isPdf"
          :src="withTicket(share.preview_url)"
          class="pdf"
          title="PDF 预览"
          sandbox="allow-same-origin allow-downloads"
        />
      </div>

      <div v-else-if="isFolder" class="folder-note state">
        <AppIcon class="state-icon" name="FolderOpen" size="xl" />
        <p>这是一个文件夹分享</p>
      </div>

      <div v-else class="fallback state">
        <AppIcon
          class="state-icon"
          :name="fileIcon(share.file_type, share.file_name)"
          size="xl"
        />
        <p>该文件类型暂不支持在线预览</p>
      </div>

      <div class="actions">
        <button
          v-if="canDownload && !(needsPassword && !verified)"
          class="btn btn-primary download"
          @click="downloadShared"
        >
          <AppIcon name="Download" size="sm" /> 下载文件
        </button>
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
.center-card .center-icon {
  color: var(--text-muted);
}
.center-card h1 {
  font-size: 1.2rem;
}
.pwd-form {
  display: flex;
  gap: 10px;
  margin-top: 12px;
  width: min(100%, 360px);
}
/* PasswordInput 的根是 .pwd-wrap，输入框在里面——flex 要加在包裹层上 */
.pwd-form .pwd-wrap {
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
.thumb .file-icon {
  color: var(--text-muted);
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
/* .state .state-icon 的样式由 style.css 统一提供 */
@media (max-width: 600px) {
  .pwd-form {
    flex-direction: column;
  }
}
</style>
