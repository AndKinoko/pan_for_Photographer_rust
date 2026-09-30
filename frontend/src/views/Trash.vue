<script setup>
import { ref, onMounted } from 'vue'
import {
  listTrash,
  emptyTrash,
  restoreFile,
  restoreFolder,
  permanentDeleteFile,
  permanentDeleteFolder,
  authUrl,
  fileIcon,
  formatSize,
  formatDate,
} from '../api'
import AppIcon from '../components/AppIcon.vue'
import LoadMore from '../components/LoadMore.vue'
import { useToast } from '../composables/useToast'
import { useProgressiveList } from '../composables/useProgressiveList'
import { confirm } from '../composables/useConfirm'

const toast = useToast()

/* 服务端游标分页状态机。回收站堆积几千项是常态（尤其是清空前）。 */
const {
  items: files,
  folders,
  total: totalFiles,
  totalFolders,
  hasMore: hasMoreFiles,
  loadingMore,
  error: pageError,
  loadFirst: loadFirstPage,
  loadMore: loadMoreFiles,
} = useProgressiveList(({ limit, cursor }) => listTrash({ limit, cursor }))
const loading = ref(false)
const error = ref('')

/** 回收站总项数 = 文件总数（服务端报告，不是已加载数）+ 文件夹数。
    用它来决定「清空回收站」是否可点、以及空态判断 —— 两者都该看真实总数。 */
const total = () => totalFiles.value + totalFolders.value

async function load() {
  loading.value = true
  error.value = ''
  // 恢复/永久删除之后重新加载：复位到第一页，
  // 因为列表内容已变，之前取到的游标不再对应任何有意义的位置
  await loadFirstPage()
  error.value = pageError.value
  loading.value = false
}

async function onRestoreFile(f) {
  try {
    await restoreFile(f.id)
    toast.success('已恢复')
    await load()
  } catch (e) {
    toast.error(e.message || '恢复失败')
  }
}
async function onRestoreFolder(f) {
  try {
    await restoreFolder(f.id)
    toast.success('已恢复')
    await load()
  } catch (e) {
    toast.error(e.message || '恢复失败')
  }
}
async function onPermanentFile(f) {
  const ok = await confirm({
    title: '永久删除',
    message: `“${f.name}” 将被永久删除，无法恢复。确定继续？`,
    variant: 'danger',
    confirmText: '永久删除',
  })
  if (!ok) return
  try {
    await permanentDeleteFile(f.id)
    toast.success('已永久删除')
    await load()
  } catch (e) {
    toast.error(e.message || '删除失败')
  }
}
async function onPermanentFolder(f) {
  const ok = await confirm({
    title: '永久删除',
    message: `文件夹 “${f.name}” 及其内容将被永久删除，无法恢复。确定继续？`,
    variant: 'danger',
    confirmText: '永久删除',
  })
  if (!ok) return
  try {
    await permanentDeleteFolder(f.id)
    toast.success('已永久删除')
    await load()
  } catch (e) {
    toast.error(e.message || '删除失败')
  }
}

async function onEmpty() {
  if (!total()) return
  const ok = await confirm({
    title: '清空回收站',
    message: '将永久删除回收站中的所有项目，无法恢复。确定继续？',
    variant: 'danger',
    confirmText: '清空回收站',
  })
  if (!ok) return
  try {
    const res = await emptyTrash()
    toast.success(`已清空 ${res.deleted_count} 项`)
    await load()
  } catch (e) {
    toast.error(e.message || '清空失败')
  }
}

onMounted(load)
</script>

<template>
  <div class="trash">
    <div class="head">
      <div>
        <h1>回收站</h1>
        <p class="muted">共 {{ total() }} 项已删除</p>
      </div>
      <button
        class="btn btn-sm btn-danger"
        :disabled="!total() || loading"
        @click="onEmpty"
      >
        <AppIcon name="Trash2" size="sm" /> 清空回收站
      </button>
    </div>

    <div v-if="loading" class="center" style="padding: 48px">
      <div class="spinner" />
    </div>

    <div v-else-if="error" class="state">
      <AppIcon class="state-icon" name="CircleAlert" size="xl" />
      <h3>加载失败</h3>
      <p>{{ error }}</p>
      <button class="btn btn-primary btn-sm" @click="load">重试</button>
    </div>

    <div v-else-if="!total()" class="state">
      <AppIcon class="state-icon" name="RotateCcw" size="xl" />
      <h3>回收站为空</h3>
      <p>删除的文件会出现在这里，30 天内可恢复</p>
    </div>

    <template v-else>
      <div v-if="folders.length" class="section">
        <h2 class="sec-title">文件夹 ({{ folders.length }})</h2>
        <ul class="list card">
          <li v-for="f in folders" :key="'d' + f.id">
            <AppIcon class="folder-ico" name="Folder" size="lg" />
            <div class="li-main">
              <div class="li-name truncate" :title="f.name">{{ f.name }}</div>
              <div class="li-sub muted">
                删除于 {{ formatDate(f.deleted_at) }}
              </div>
            </div>
            <div class="li-actions">
              <button class="btn btn-sm" @click="onRestoreFolder(f)">
                <AppIcon name="RotateCcw" size="sm" /> 恢复
              </button>
              <button class="btn btn-sm btn-danger" @click="onPermanentFolder(f)">永久删除</button>
            </div>
          </li>
        </ul>
      </div>

      <div v-if="files.length" class="section">
        <h2 class="sec-title">文件 ({{ totalFiles }})</h2>
        <ul class="list card">
          <li v-for="f in files" :key="'f' + f.id">
            <span class="thumb">
              <img
                v-if="f.thumb_url"
                :src="authUrl(f.thumb_url)"
                alt=""
                @error="$event.target.style.display = 'none'"
              />
              <AppIcon
                v-else
                class="file-icon"
                :name="fileIcon(f.file_type, f.name)"
                size="lg"
              />
            </span>
            <div class="li-main">
              <div class="li-name truncate" :title="f.name">{{ f.name }}</div>
              <div class="li-sub muted">
                {{ f.formatted_size || formatSize(f.size) }} · 删除于 {{ formatDate(f.deleted_at) }}
              </div>
            </div>
            <div class="li-actions">
              <button class="btn btn-sm" @click="onRestoreFile(f)">
                <AppIcon name="RotateCcw" size="sm" /> 恢复
              </button>
              <button class="btn btn-sm btn-danger" @click="onPermanentFile(f)">永久删除</button>
            </div>
          </li>
        </ul>
      </div>

      <LoadMore
        :has-more="hasMoreFiles"
        :loaded="files.length"
        :total="totalFiles"
        :loading="loadingMore"
        @more="loadMoreFiles"
      />
    </template>
  </div>
</template>

<style scoped>
.trash {
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
.section {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.sec-title {
  font-size: 0.92rem;
  color: var(--text-heading);
  padding-left: 4px;
}
.list {
  list-style: none;
  margin: 0;
  padding: 4px;
  overflow: hidden;
}
.list li {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 12px;
  border-radius: var(--radius-sm);
  transition: background-color 0.15s ease;
}
.list li:hover {
  background: var(--bg-hover);
}
.thumb {
  width: 44px;
  height: 44px;
  flex: 0 0 44px;
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
.list .folder-ico {
  color: var(--primary);
}
.li-main {
  flex: 1 1 auto;
  min-width: 0;
}
.li-name {
  font-weight: 600;
  color: var(--text-heading);
  font-size: 0.92rem;
}
.li-sub {
  font-size: 0.76rem;
}
.li-actions {
  display: flex;
  gap: 8px;
  flex: 0 0 auto;
}
@media (max-width: 560px) {
  .li-actions {
    flex-direction: column;
  }
  .list li {
    flex-wrap: wrap;
  }
}
</style>
