<script setup>
/* ===========================================================================
   公开分享页 —— 批次只读浏览器
   ---------------------------------------------------------------------------
   一个分享是一个**批次**：里面可以同时装多个文件与多个文件夹，文件夹可以
   一直点进去看。客户端能做的只有两件事——**看**和**下载**。

   与主文件管理页共用同一套 `FileCard`（context="share" 会裁掉重命名/分享/
   删除，只留下载与预览），不复用的话就会长出第二套卡片，
   然后两边的视觉与交互各自漂移。

   只读的边界靠三处共同保证，缺一不可：
     · 前端不渲染任何写入入口（本文件里根本没有上传/新建/重命名/删除的代码）
     · 后端对每个 file_id / folder_id 做子树授权（share_service::file_in_share）
     · 服务端只暴露 /api/public/* 的只读接口给未登录请求
   =========================================================================== */

import { ref, computed, onMounted, nextTick } from 'vue'
import { useRoute } from 'vue-router'
import {
  getPublicShare,
  getPublicShareItems,
  verifySharePassword,
  withShareTicket,
  formatDate,
} from '../api'
import { useToast } from '../composables/useToast'
import AppIcon from '../components/AppIcon.vue'
import PasswordInput from '../components/PasswordInput.vue'
import FileCard from '../components/FileCard.vue'
import FilePreview from '../components/FilePreview.vue'
import LoadMore from '../components/LoadMore.vue'
import { useTheme } from '../composables/useTheme'
import { useTransfer } from '../composables/useTransfer'

const route = useRoute()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()
// 顶层解构，模板里才能自动解包 ref
const { enqueueDownload, downloadActiveCount } = useTransfer()

/**
 * 一次最多入队多少个下载。
 *
 * 不打包 zip，所以「下载所选」是**并发发起 N 个下载**。浏览器对同时多个
 * 下载有弹窗拦截，而下载实现是先把整个文件读进内存再落盘——选 20 张大图
 * 已经能把标签页压得很紧。超过就明确告诉用户分批，而不是让他点完卡死。
 */
const MAX_BATCH_DOWNLOAD = 20

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

/* ---------------- 浏览状态 ---------------- */

/** 当前所在目录；`null` = 批次根（列出批次里直接包含的条目） */
const folderId = ref(null)
const breadcrumbs = ref([])
const items = ref([])
const total = ref(0)
const hasMore = ref(false)
const nextCursor = ref(null)
const browsing = ref(false)
const browseError = ref('')

const selected = ref(new Set())
const preview = ref({ visible: false, list: [], index: 0 })

const keyOf = (it) => `${it.item_type}:${it.id}`
const isFolder = (it) => it.item_type === 'folder'

const selectedCount = computed(() => selected.value.size)
const selectedFiles = computed(() =>
  items.value.filter((i) => !isFolder(i) && selected.value.has(keyOf(i)))
)
const allSelected = computed(
  () =>
    items.value.filter((i) => !isFolder(i)).length > 0 &&
    items.value.filter((i) => !isFolder(i)).every((i) => selected.value.has(keyOf(i)))
)

/* ---------------- 加载 ---------------- */

async function loadShare() {
  loading.value = true
  loadError.value = ''
  try {
    // 带上访问凭证：受密码保护的分享，只有带了有效 ticket，后端才会下发
    // 媒体地址。verify() 通过后会重新调用本函数，此时 ticket 已就位。
    share.value = await getPublicShare(id.value, ticket.value)
    // 后端一定会返回 has_password；对「字段缺失」也保守处理成「需要密码」——
    // 宁可多问一次口令，也不能因为后端改了字段名就静默放行。
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

/** 列出某个目录的内容。`target = null` 表示回到批次根。 */
async function browse(target = null, { append = false } = {}) {
  browsing.value = true
  browseError.value = ''
  try {
    const page = await getPublicShareItems(id.value, {
      folderId: target,
      ticket: ticket.value,
      cursor: append ? nextCursor.value : null,
    })
    folderId.value = page.folderId
    breadcrumbs.value = page.breadcrumbs
    items.value = append ? items.value.concat(page.items) : page.items
    total.value = page.total
    hasMore.value = page.hasMore
    nextCursor.value = page.nextCursor
  } catch (e) {
    if (!append) {
      items.value = []
      breadcrumbs.value = []
    }
    browseError.value = e.message || '加载失败'
  } finally {
    browsing.value = false
  }
}

function enterFolder(item) {
  if (!isFolder(item)) return
  selected.value = new Set()
  browse(item.id)
}

function enterRoot() {
  selected.value = new Set()
  browse(null)
}

function loadMore() {
  if (hasMore.value && !browsing.value) browse(folderId.value, { append: true })
}

/* ---------------- 密码门 ---------------- */

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
    ticket.value = data?.ticket || ''
    verified.value = true
    needsPassword.value = false
    // 后端对**有密码的分享**在未解锁时不下发媒体地址，所以验码后必须重新
    // 拉一次详情与内容。不重新拉的话，验完口令仍然看不到任何东西。
    await loadShare()
    await browse(null)
    toast.success('密码正确')
  } catch (e) {
    failVerify(e.message || '密码错误')
  } finally {
    verifying.value = false
  }
}

function onKeydown(e) {
  if (e.key !== 'Enter') return
  // 焦点在按钮上时交给按钮自己处理，否则按 Enter 会提交两次
  if (e.target instanceof HTMLButtonElement) return
  verify()
}

/* ---------------- 选择与下载 ---------------- */

function toggleSelect(item) {
  if (isFolder(item)) return
  const next = new Set(selected.value)
  const k = keyOf(item)
  if (next.has(k)) next.delete(k)
  else next.add(k)
  selected.value = next
}

function toggleSelectAll() {
  if (allSelected.value) {
    selected.value = new Set()
  } else {
    selected.value = new Set(items.value.filter((i) => !isFolder(i)).map(keyOf))
  }
}

function enqueue(item) {
  enqueueDownload({
    filename: item.name,
    // 公开分享没有登录态，凭据只能走查询串（后端对每个 file_id 做子树校验）
    url: withShareTicket(item.download_url, ticket.value),
    authed: false,
  })
}

function downloadOne(item) {
  if (isFolder(item)) return
  enqueue(item)
}

function downloadSelected() {
  const files = selectedFiles.value
  if (files.length === 0) {
    toast.warning('请先选择要下载的文件')
    return
  }
  if (files.length > MAX_BATCH_DOWNLOAD) {
    toast.warning(
      `一次最多下载 ${MAX_BATCH_DOWNLOAD} 个文件，当前选了 ${files.length} 个，请分批下载`
    )
    return
  }
  for (const f of files) enqueue(f)
  toast.success(`已加入下载队列：${files.length} 个文件`)
  selected.value = new Set()
}

/* ---------------- 预览 ---------------- */

/** FilePreview 需要 `media_url`（GIF 播原图用），而批次条目里后端只给了
 *  预览地址与下载地址。这里按同一套公开接口补一个「原文件」地址。 */
function toPreviewEntry(item) {
  return {
    ...item,
    media_url: `/api/public/shares/${id.value}/media?file_id=${item.id}`,
  }
}

function previewOne(item) {
  if (isFolder(item)) return
  const playable = items.value.filter((i) => !isFolder(i) && isPreviewableItem(i))
  const idx = playable.findIndex((i) => i.id === item.id)
  if (idx < 0) return
  preview.value = { visible: true, list: playable.map(toPreviewEntry), index: idx }
}

function isPreviewableItem(item) {
  const ext = (item.name || '').split('.').pop()?.toLowerCase() || ''
  return [
    'jpg', 'jpeg', 'png', 'gif', 'bmp', 'webp', 'tiff', 'tif', 'avif', 'heic',
    'nef', 'cr2', 'cr3', 'crw', 'arw', 'sr2', 'srf', 'dng', 'raf', 'orf', 'rw2', 'nrw',
    'mp4', 'mov', 'avi', 'mkv', 'webm', 'm4v',
    'mp3', 'wav', 'flac', 'ogg', 'aac', 'm4a',
    'pdf',
  ].includes(ext)
}

function onItemClick(item) {
  if (isFolder(item)) {
    enterFolder(item)
    return
  }
  if (isPreviewableItem(item)) previewOne(item)
  else downloadOne(item)
}

onMounted(async () => {
  await loadShare()
  // 未解锁时不预取内容——后端会 401，白跑一趟还多一条报错
  if (!needsPassword.value && !loadError.value) {
    await browse(null)
  }
})
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
      <p v-if="pwdError" id="share-password-error" class="err" role="alert">
        {{ pwdError }}
      </p>
    </div>

    <!-- Content -->
    <div v-else class="card content-card">
      <header class="batch-head">
        <h1 class="batch-title">{{ share.owner_name }} 分享的内容</h1>
        <div class="meta-row muted">
          <span v-if="share.total_folder_count">{{ share.total_folder_count }} 个文件夹</span>
          <span v-if="share.total_folder_count" class="dot">·</span>
          <span>{{ share.total_file_count }} 个文件</span>
          <span class="dot">·</span>
          <span>{{ share.formatted_size }}</span>
        </div>
        <div class="meta-row muted">
          <span class="badge-readonly">
            <AppIcon name="Lock" size="sm" /> 只读
          </span>
          <span v-if="share.expires_at">到期：{{ formatDate(share.expires_at) }}</span>
          <span v-else>永久有效</span>
          <span v-if="share.max_downloads" class="dot">·</span>
          <span v-if="share.max_downloads">
            最多下载 {{ share.max_downloads }} 个文件（已用 {{ share.download_count }}）
          </span>
        </div>
      </header>

      <!-- 进入子目录后的返回路径。只在扎根时显示第一条。 -->
      <nav v-if="folderId !== null" class="nav" aria-label="目录导航">
        <button class="crumb" @click="enterRoot">
          <AppIcon name="FolderOpen" size="sm" /> 全部内容
        </button>
        <template v-for="(c, i) in breadcrumbs" :key="c.id">
          <AppIcon class="sep" name="ChevronRight" size="sm" />
          <button
            class="crumb"
            :class="{ current: i === breadcrumbs.length - 1 }"
            @click="browse(c.id)"
          >
            {{ c.name }}
          </button>
        </template>
      </nav>

      <div v-if="browseError" class="state err-state">
        <AppIcon class="state-icon" name="CircleAlert" size="xl" />
        <p>{{ browseError }}</p>
      </div>

      <div v-else-if="browsing && items.length === 0" class="state">
        <div class="spinner" />
      </div>

      <div v-else-if="items.length === 0" class="state">
        <AppIcon class="state-icon" name="FolderOpen" size="xl" />
        <p>这里没有内容</p>
      </div>

      <!-- 与主文件管理页**同一个卡片组件**，靠 context 裁掉写操作 -->
      <div v-else class="grid">
        <FileCard
          v-for="it in items"
          :key="keyOf(it)"
          :item="it"
          :kind="isFolder(it) ? 'folder' : 'file'"
          context="share"
          :ticket="ticket"
          :selected="selected.has(keyOf(it))"
          :selectable="!isFolder(it)"
          @click="onItemClick(it)"
          @toggle-select="toggleSelect(it)"
          @download="downloadOne(it)"
          @preview="previewOne(it)"
        />
      </div>

      <LoadMore
        v-if="hasMore"
        :has-more="hasMore"
        :loaded="items.length"
        :total="total"
        :loading="browsing"
        @more="loadMore"
      />
    </div>

    <!-- 批量条：公开页只有「下载」一个动作，所以没有复用 BatchToolbar -->
    <div v-if="selectedCount" class="sel-bar">
      <span class="sel-count">已选 {{ selectedCount }} 项</span>
      <button class="btn btn-ghost" @click="toggleSelectAll">
        {{ allSelected ? '取消全选' : '全选本页' }}
      </button>
      <button class="btn btn-ghost" @click="selected = new Set()">清空</button>
      <button class="btn btn-primary" @click="downloadSelected">
        <AppIcon name="Download" size="sm" /> 下载所选
      </button>
    </div>

    <!-- 下载进度：公开页没有全局传输抽屉，进度必须就地反馈 -->
    <div v-if="downloadActiveCount > 0" class="dl-status" role="status">
      <div class="spinner spinner-sm" />
      正在下载 {{ downloadActiveCount }} 个文件…
    </div>

    <FilePreview
      v-model:visible="preview.visible"
      mode="share"
      :ticket="ticket"
      :files="preview.list"
      :index="preview.index"
      @close="preview.visible = false"
      @update:index="preview.index = $event"
    />
  </div>
</template>

<style scoped>
.pub {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 32px 16px 96px;
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
  width: min(94vw, 1100px);
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
  max-width: 480px;
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
.batch-head {
  margin-bottom: 16px;
}
.batch-title {
  font-size: 1.15rem;
  margin-bottom: 4px;
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
.badge-readonly {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 1px 8px;
  border-radius: 999px;
  background: var(--bg-hover);
  color: var(--text-muted);
  font-size: 0.76rem;
}

.nav {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  padding-bottom: 12px;
  margin-bottom: 12px;
  border-bottom: 1px solid var(--border);
}
.crumb {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  color: var(--text-body);
  font-size: 0.86rem;
}
.crumb:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
}
.crumb.current {
  color: var(--text-heading);
  font-weight: 600;
}
.sep {
  color: var(--text-muted);
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 16px;
}
.state {
  padding: 48px 24px;
  text-align: center;
  color: var(--text-muted);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}
.state-icon {
  color: var(--text-muted);
}
.err-state {
  color: var(--danger);
}

.sel-bar {
  position: fixed;
  left: 50%;
  bottom: 20px;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  border-radius: var(--radius-lg);
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  box-shadow: var(--shadow-lg);
  z-index: var(--z-popover);
}
.sel-count {
  font-size: 0.86rem;
  color: var(--text-body);
  white-space: nowrap;
}

.dl-status {
  position: fixed;
  right: 16px;
  bottom: 20px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 14px;
  border-radius: 999px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  box-shadow: var(--shadow);
  font-size: 0.84rem;
  color: var(--text-body);
  z-index: var(--z-popover);
}
.spinner-sm {
  width: 14px;
  height: 14px;
  border-width: 2px;
}

@media (max-width: 600px) {
  .pwd-form {
    flex-direction: column;
  }
  .grid {
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 12px;
  }
  .sel-bar {
    width: calc(100vw - 24px);
    flex-wrap: wrap;
    justify-content: center;
  }
}
</style>
