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
import FileCard from '../components/FileCard.vue'
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

const fileTypes = ref([])
const files = ref([])
const folders = ref([])
const loading = ref(false)
const error = ref('')
const hasSearched = ref(false)

const preview = ref({ visible: false, index: 0 })
const showShare = ref(false)
const shareFileIds = ref([])

let timer = null
function scheduleSearch() {
  clearTimeout(timer)
  timer = setTimeout(runSearch, 350)
}

async function runSearch() {
  const term = q.value.trim()
  if (!term) {
    files.value = []
    folders.value = []
    hasSearched.value = false
    return
  }
  loading.value = true
  error.value = ''
  const params = { q: term }
  if (filters.type) params.type = filters.type
  if (filters.minSize !== '') params.min_size = Math.round(Number(filters.minSize) * 1024 * 1024)
  if (filters.maxSize !== '') params.max_size = Math.round(Number(filters.maxSize) * 1024 * 1024)
  if (filters.dateFrom) params.date_from = filters.dateFrom
  if (filters.dateTo) params.date_to = filters.dateTo
  params.sort = filters.sort
  params.order = filters.order
  try {
    const data = await searchFiles(params)
    files.value = data.files || []
    folders.value = data.folders || []
    fileTypes.value = data.file_types || []
    hasSearched.value = true
  } catch (e) {
    error.value = e.message || '搜索失败'
  } finally {
    loading.value = false
  }
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

function openShare(file) {
  shareFileIds.value = [file.id]
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
    <div class="searchbar card">
      <span class="icon">🔍</span>
      <input
        v-model="q"
        class="grow"
        type="text"
        placeholder="搜索文件名或文件夹名…"
        autofocus
      />
      <button v-if="q" class="btn-icon btn-ghost" aria-label="清除" @click="q = ''">
        ✕
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
      <span class="emoji">⚠️</span>
      <h3>搜索失败</h3>
      <p>{{ error }}</p>
    </div>

    <div v-else-if="!q.trim() && !hasSearched" class="state">
      <span class="emoji">🔎</span>
      <h3>输入关键词开始搜索</h3>
      <p>支持按类型、大小、日期组合筛选</p>
    </div>

    <div v-else-if="!files.length && !folders.length" class="state">
      <span class="emoji">🗂️</span>
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
          />
        </div>
      </div>
      <div v-if="files.length" class="section">
        <h2 class="sec-title">文件 ({{ files.length }})</h2>
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
    </template>

    <FilePreview
      :visible="preview.visible"
      :files="files"
      :index="preview.index"
      @close="preview.visible = false"
      @update:index="preview.index = $event"
    />

    <ShareDialog v-model:visible="showShare" :file-ids="shareFileIds" />
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
  font-size: 1.1rem;
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
