<script setup>
import { ref, reactive, onMounted, onBeforeUnmount, watch } from 'vue'
import { useRouter } from 'vue-router'
import {
  searchFiles,
  renameFile,
  deleteFile,
} from '../api'
import { useToast } from '../composables/useToast'
import { confirm } from '../composables/useConfirm'
import { useTransfer } from '../composables/useTransfer'
import { useProgressiveList } from '../composables/useProgressiveList'
import FileCard from '../components/FileCard.vue'
import AppIcon from '../components/AppIcon.vue'
import LoadMore from '../components/LoadMore.vue'
import FilePreview from '../components/FilePreview.vue'
import ShareDialog from '../components/ShareDialog.vue'

const router = useRouter()
const toast = useToast()
const transfer = useTransfer()

const q = ref('')
const filters = reactive({
  type: '',
  minSize: '',
  maxSize: '',
  dateFrom: '',
  dateTo: '',
  sort: 'uploaded_at',
  order: 'desc',
})

/** 当前搜索条件（不含分页）。
    loadMore 必须用与首页**完全相同**的条件：游标里带着排序标识，
    服务端会校验它与本次请求的排序是否一致，不一致直接 400。 */
function buildSearchParams() {
  const params = { q: q.value.trim() }
  if (filters.type) params.type = filters.type
  if (filters.minSize !== '') {
    params.min_size = Math.round(Number(filters.minSize) * 1024 * 1024)
  }
  if (filters.maxSize !== '') {
    params.max_size = Math.round(Number(filters.maxSize) * 1024 * 1024)
  }
  if (filters.dateFrom) params.date_from = filters.dateFrom
  if (filters.dateTo) params.date_to = filters.dateTo
  params.sort = filters.sort
  params.order = filters.order
  return params
}

/* 服务端游标分页状态机。
   FilePreview 拿到的是**已加载**的 files，所以预览里的左右翻页范围是
   「本次已取回的这些」，而不是整个结果集 —— 点「加载更多」可以扩大它。 */
const {
  items: files,
  folders,
  fileTypes,
  total: fileTotal,
  hasMore: hasMoreFiles,
  loadingMore,
  error: pageError,
  loadFirst: loadFirstPage,
  loadMore: loadMoreFiles,
  reset: resetPaging,
} = useProgressiveList(({ limit, cursor }) =>
  searchFiles({ ...buildSearchParams(), limit, cursor })
)
const loading = ref(false)
const error = ref('')
const hasSearched = ref(false)

const preview = ref({ visible: false, index: 0 })
const showShare = ref(false)
/** 批次内容：`[{type:'file'|'folder', id}]` */
const shareItems = ref([])

let timer = null
function scheduleSearch() {
  clearTimeout(timer)
  timer = setTimeout(runSearch, 350)
}

async function runSearch() {
  const term = q.value.trim()
  if (!term) {
    // reset() 会同时作废在途请求，所以「清空关键词后又回来」不会把旧结果填回
    resetPaging()
    hasSearched.value = false
    return
  }
  loading.value = true
  // 新的一次搜索 = 新的一次查询：loadFirstPage 内部会递增序号，
  // 在途的旧请求（含翻页请求）回来时会被丢弃
  await loadFirstPage()
  error.value = pageError.value
  hasSearched.value = true
  loading.value = false
}

watch(q, scheduleSearch)
watch(filters, scheduleSearch, { deep: true })

onMounted(() => {
  // Focus search input
})

onBeforeUnmount(() => clearTimeout(timer))

function onFileClick(file) {
  const idx = files.value.findIndex((f) => f.id === file.id)
  preview.value = { visible: true, index: idx < 0 ? 0 : idx }
}
function onFolderClick(folder) {
  router.push({ path: '/', query: { folder: folder.id } })
}

function downloadFile(file) {
  // 下载进入全局下载队列（抽屉内实时进度）
  transfer.enqueueDownload({ filename: file.name, url: file.download_url, authed: true })
}

async function onRename(file) {
  const name = await confirm({
    title: '重命名文件',
    inputLabel: '新名称',
    inputValue: file.name,
    confirmText: '保存',
  })
  if (name == null) return
  const trimmed = String(name).trim()
  if (!trimmed) return toast.warning('名称不能为空')
  try {
    await renameFile(file.id, trimmed)
    toast.success('已重命名')
    runSearch()
  } catch (e) {
    toast.error(e.message || '重命名失败')
  }
}

async function onRemove(file) {
  const ok = await confirm({
    title: '移入回收站',
    message: `确定将 “${file.name}” 移入回收站？`,
    variant: 'danger',
    confirmText: '删除',
  })
  if (!ok) return
  try {
    await deleteFile(file.id)
    toast.success('已移入回收站')
    runSearch()
  } catch (e) {
    toast.error(e.message || '删除失败')
  }
}

function openShare(item, kind = 'file') {
  shareItems.value = [{ type: kind, id: item.id }]
  showShare.value = true
}

function resetFilters() {
  filters.type = ''
  filters.minSize = ''
  filters.maxSize = ''
  filters.dateFrom = ''
  filters.dateTo = ''
  filters.sort = 'uploaded_at'
  filters.order = 'desc'
}
</script>

<template>
  <div class="search">
    <h1 class="sr-only">搜索</h1>

    <div class="searchbar card">
      <AppIcon class="icon" name="Search" size="sm" />
      <input
        v-model="q"
        class="grow"
        type="text"
        placeholder="搜索文件名或文件夹名…"
        autofocus
      />
      <button v-if="q" class="btn-icon btn-ghost" aria-label="清除" @click="q = ''">
        <AppIcon name="X" size="sm" />
      </button>
    </div>

    <div class="filters card">
      <div class="field-inline">
        <label>类型</label>
        <select v-model="filters.type" class="select">
          <option value="">全部</option>
          <option v-for="t in fileTypes" :key="t" :value="t">{{ t }}</option>
        </select>
      </div>
      <div class="field-inline">
        <label>最小 (MB)</label>
        <input
          v-model="filters.minSize"
          class="input"
          type="number"
          min="0"
          placeholder="0"
        />
      </div>
      <div class="field-inline">
        <label>最大 (MB)</label>
        <input
          v-model="filters.maxSize"
          class="input"
          type="number"
          min="0"
          placeholder="不限"
        />
      </div>
      <div class="field-inline">
        <label>起始日期</label>
        <input v-model="filters.dateFrom" class="input" type="date" />
      </div>
      <div class="field-inline">
        <label>结束日期</label>
        <input v-model="filters.dateTo" class="input" type="date" />
      </div>
      <div class="field-inline">
        <label>排序</label>
        <select v-model="filters.sort" class="select">
          <option value="uploaded_at">上传时间</option>
          <option value="name">名称</option>
          <option value="size">大小</option>
        </select>
      </div>
      <div class="field-inline">
        <label>方向</label>
        <select v-model="filters.order" class="select">
          <option value="desc">降序</option>
          <option value="asc">升序</option>
        </select>
      </div>
      <button class="btn btn-sm btn-ghost" @click="resetFilters">重置</button>
    </div>

    <div v-if="loading" class="grid">
      <div v-for="i in 6" :key="'sk' + i" class="sk-card">
        <div class="skeleton sk-thumb" />
        <div class="skeleton sk-line" />
        <div class="skeleton sk-line short" />
      </div>
    </div>

    <div v-else-if="error" class="state">
      <AppIcon class="state-icon" name="CircleAlert" size="xl" />
      <h3>搜索失败</h3>
      <p>{{ error }}</p>
    </div>

    <div v-else-if="!q.trim() && !hasSearched" class="state">
      <AppIcon class="state-icon" name="Search" size="xl" />
      <h3>输入关键词开始搜索</h3>
      <p>支持按类型、大小、日期组合筛选</p>
    </div>

    <div v-else-if="!files.length && !folders.length" class="state">
      <AppIcon class="state-icon" name="SearchX" size="xl" />
      <h3>未找到匹配结果</h3>
      <p>试试调整关键词或筛选条件</p>
    </div>

    <template v-else>
      <div v-if="folders.length" class="section">
        <h2 class="sec-title">文件夹 ({{ folders.length }})</h2>
        <div class="grid">
          <FileCard
            v-for="f in folders"
            :key="'d' + f.id"
            :item="f"
            kind="folder"
            @click="onFolderClick(f)"
            @share="openShare(f, 'folder')"
          />
        </div>
      </div>
      <div v-if="files.length" class="section">
        <h2 class="sec-title">文件 ({{ fileTotal }})</h2>
        <div class="grid">
          <FileCard
            v-for="f in files"
            :key="'f' + f.id"
            :item="f"
            kind="file"
            @click="onFileClick(f)"
            @rename="onRename(f)"
            @remove="onRemove(f)"
            @share="openShare(f)"
            @download="downloadFile(f)"
          />
        </div>
      </div>

      <LoadMore
        :has-more="hasMoreFiles"
        :loaded="files.length"
        :total="fileTotal"
        :loading="loadingMore"
        @more="loadMoreFiles"
      />
    </template>

    <FilePreview
      :visible="preview.visible"
      :files="files"
      :index="preview.index"
      @close="preview.visible = false"
      @update:index="preview.index = $event"
    />

    <ShareDialog v-model:visible="showShare" :items="shareItems" />
  </div>
</template>

<style scoped>
.search {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.searchbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
}
.searchbar .icon {
  color: var(--text-muted);
}
.searchbar input {
  border: none;
  background: transparent;
  outline: none;
  min-height: 40px;
  font-size: 1rem;
  color: var(--text-heading);
}
.filters {
  display: flex;
  align-items: flex-end;
  gap: 12px;
  padding: 14px;
  flex-wrap: wrap;
}
.field-inline {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 120px;
}
.field-inline label {
  font-size: 0.74rem;
  color: var(--text-muted);
  font-weight: 500;
}
.field-inline .input,
.field-inline .select {
  min-height: 40px;
}
.section {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.sec-title {
  font-size: 0.95rem;
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 14px;
}
.sk-card {
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
  padding-bottom: 10px;
}
.sk-thumb {
  width: 100%;
  aspect-ratio: 4 / 3;
  border-radius: 0;
}
.sk-line {
  height: 12px;
  margin: 10px 12px 0;
}
.sk-line.short {
  width: 50%;
}
@media (max-width: 768px) {
  .grid {
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 10px;
  }
  .field-inline {
    min-width: 100px;
  }
}
@media (max-width: 480px) {
  .grid {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>
