<script setup>
import { ref, onMounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import {
  listShares,
  deleteShare,
  authUrl,
  fileIcon,
  formatDate,
} from '../api'
import AppIcon from '../components/AppIcon.vue'
import { useToast } from '../composables/useToast'
import { confirm } from '../composables/useConfirm'

const router = useRouter()
const toast = useToast()

const shares = ref([])
const loading = ref(false)
const error = ref('')

/* ---------------- 批次展示辅助 ----------------
   列表里的每一项现在是一个**批次**（可能装着多个文件与文件夹），
   所以缩略图、标题、副标题都要从 items 里推出来，不再有单一的 file_name。 */

/** 批次缩略图取第一个「有缩略图的文件」。
 *
 *  注意这里走的是**登录态**的 `/api/files/:id/media`，而不是后端在
 *  `items[].thumb_url` 里下发的公开地址——后者要走分享的密码门，
 *  而所有者手上只有 JWT，没有那张 ticket。两套地址服务两个场景。 */
function shareThumb(s) {
  const it = (s.items || []).find((i) => i.item_type === 'file' && i.thumb_url)
  return it ? `/api/files/${it.id}/media?thumb=1` : null
}

/** 条目名：只有一个时直接用它的名字，多个时给个概括。 */
function shareTitle(s) {
  const items = s.items || []
  if (items.length === 0) return '(空)'
  if (items.length === 1) return items[0].name
  const folders = items.filter((i) => i.item_type === 'folder').length
  const files = items.length - folders
  const parts = []
  if (folders) parts.push(`${folders} 个文件夹`)
  if (files) parts.push(`${files} 个文件`)
  return parts.join(' · ')
}

/** 副标题：递归统计——「这批交付了多少东西」比「顶层几个条目」有用。 */
function shareSubtitle(s) {
  const n = s.total_file_count || 0
  const folders = s.total_folder_count || 0
  const parts = []
  if (folders) parts.push(`${folders} 个文件夹`)
  parts.push(`${n} 个文件`)
  return `${parts.join(' · ')} · ${s.formatted_size || '0 B'}`
}

function shareIcon(s) {
  const it = (s.items || [])[0]
  if (!it) return 'File'
  return it.item_type === 'folder' ? 'Folder' : fileIcon(it.file_type, it.name)
}

const activeCount = computed(
  () => shares.value.filter((s) => s.is_active && !s.is_expired).length
)

async function load() {
  loading.value = true
  error.value = ''
  try {
    shares.value = (await listShares()) || []
  } catch (e) {
    error.value = e.message || '加载失败'
  } finally {
    loading.value = false
  }
}

function absoluteUrl(share) {
  const path = share.share_url || `/share/${share.id}`
  return `${window.location.origin}${path}`
}

async function copyLink(share) {
  const url = absoluteUrl(share)
  try {
    await navigator.clipboard.writeText(url)
    toast.success('链接已复制到剪贴板')
  } catch {
    // Fallback for non-secure contexts
    const ta = document.createElement('textarea')
    ta.value = url
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    try {
      document.execCommand('copy')
      toast.success('链接已复制')
    } catch {
      toast.error('复制失败，请手动复制')
    }
    ta.remove()
  }
}

async function onDelete(share) {
  const ok = await confirm({
    title: '删除分享',
    message: `确定删除 “${share.file_name}” 的分享链接？删除后该链接将立即失效。`,
    variant: 'danger',
    confirmText: '删除',
  })
  if (!ok) return
  try {
    await deleteShare(share.id)
    toast.success('分享已删除')
    await load()
  } catch (e) {
    toast.error(e.message || '删除失败')
  }
}

function openShare(share) {
  router.push(`/share/${share.id}`)
}

function expiryText(share) {
  if (!share.expires_at) return '永久有效'
  return `到期：${formatDate(share.expires_at)}`
}

onMounted(load)
</script>

<template>
  <div class="shares">
    <div class="head">
      <div>
        <h1>我的分享</h1>
        <p class="muted">共 {{ shares.length }} 个分享，{{ activeCount }} 个有效</p>
      </div>
      <button class="btn btn-sm btn-primary" @click="router.push('/')">
        <AppIcon name="Files" size="sm" /> 去文件管理创建分享
      </button>
    </div>

    <div v-if="loading" class="grid">
      <div v-for="i in 4" :key="i" class="sk-card">
        <div class="skeleton sk-thumb" />
        <div class="skeleton sk-line" />
        <div class="skeleton sk-line short" />
      </div>
    </div>

    <div v-else-if="error" class="state">
      <AppIcon class="state-icon" name="CircleAlert" size="xl" />
      <h3>加载失败</h3>
      <p>{{ error }}</p>
      <button class="btn btn-primary btn-sm" @click="load">重试</button>
    </div>

    <div v-else-if="!shares.length" class="state">
      <AppIcon class="state-icon" name="Link" size="xl" />
      <h3>还没有分享</h3>
      <p>在文件管理中选择文件即可创建分享链接</p>
    </div>

    <div v-else class="grid">
      <article
        v-for="s in shares"
        :key="s.id"
        class="share-card card"
        :class="{ expired: s.is_expired || !s.is_active }"
      >
        <div class="top" @click="openShare(s)">
          <span class="thumb">
            <img
              v-if="shareThumb(s)"
              :src="authUrl(shareThumb(s))"
              alt=""
              @error="$event.target.style.display = 'none'"
            />
            <AppIcon
              v-else
              class="file-icon"
              :name="shareIcon(s)"
              size="lg"
            />
          </span>
          <div class="meta">
            <div class="name truncate" :title="shareTitle(s)">{{ shareTitle(s) }}</div>
            <div class="sub muted truncate">
              {{ shareSubtitle(s) }}
            </div>
            <div class="badges">
              <span v-if="s.is_expired" class="badge badge-danger">已过期</span>
              <span v-else-if="!s.is_active" class="badge badge-muted">已停用</span>
              <span v-else class="badge">有效</span>
              <span v-if="s.has_password" class="badge badge-muted">
                <AppIcon name="Lock" size="sm" /> 加密
              </span>
            </div>
          </div>
        </div>

        <div class="stats">
          <div class="stat">
            <strong>{{ s.download_count }}</strong>
            <small class="muted">下载</small>
          </div>
          <div class="stat">
            <small class="muted">{{ expiryText(s) }}</small>
          </div>
        </div>

        <div class="actions">
          <button class="btn btn-sm grow" @click="copyLink(s)">
            <AppIcon name="Copy" size="sm" /> 复制链接
          </button>
          <button class="btn btn-sm btn-ghost" @click="openShare(s)">查看</button>
          <button class="btn btn-sm btn-danger" @click="onDelete(s)">删除</button>
        </div>
      </article>
    </div>
  </div>
</template>

<style scoped>
.shares {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}
.head h1 {
  font-size: 1.15rem;
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 14px;
}
.share-card {
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  transition: box-shadow 0.16s ease, opacity 0.16s ease;
}
.share-card:hover {
  box-shadow: var(--shadow);
}
.share-card.expired {
  opacity: 0.7;
}
.top {
  display: flex;
  gap: 12px;
  cursor: pointer;
}
.thumb {
  width: 56px;
  height: 56px;
  flex: 0 0 56px;
  border-radius: var(--radius-sm);
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
.meta {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.name {
  font-weight: 600;
  color: var(--text-heading);
  font-size: 0.92rem;
}
.sub {
  font-size: 0.78rem;
}
.badges {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 4px;
}
.stats {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 0;
  border-top: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
}
.stat {
  display: flex;
  flex-direction: column;
  font-size: 0.8rem;
  color: var(--text-heading);
}
.stat small {
  font-size: 0.74rem;
}
.actions {
  display: flex;
  gap: 8px;
}
.actions .btn {
  flex: 0 0 auto;
}

.sk-card {
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 14px;
}
.sk-thumb {
  width: 100%;
  height: 56px;
  border-radius: var(--radius-sm);
}
.sk-line {
  height: 12px;
  margin-top: 12px;
}
.sk-line.short {
  width: 50%;
}
@media (max-width: 768px) {
  .grid {
    grid-template-columns: 1fr;
  }
}
</style>
