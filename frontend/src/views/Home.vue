<script setup>
import { ref, computed, onMounted, watch, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  listFiles,
  listFolders,
  createFolder,
  renameFile,
  renameFolder,
  deleteFile,
  deleteFolder,
  batchMove,
  batchCopy,
  batchDelete,
  authUrl,
} from '../api'
import { useToast } from '../composables/useToast'
import { confirm } from '../composables/useConfirm'
import FileCard from '../components/FileCard.vue'
import FilePreview from '../components/FilePreview.vue'
import UploadZone from '../components/UploadZone.vue'
import ShareDialog from '../components/ShareDialog.vue'
import BatchToolbar from '../components/BatchToolbar.vue'
import Breadcrumb from '../components/Breadcrumb.vue'

const route = useRoute()
const router = useRouter()
const toast = useToast()

/* ----------------------------- Folder context ---------------------------- */
const breadcrumb = ref([]) // [{ id, name }]; empty = root
const currentFolderId = computed(() =>
  breadcrumb.value.length ? breadcrumb.value[breadcrumb.value.length - 1].id : null
)

const files = ref([])
const folders = ref([])
const loading = ref(false)
const errorMsg = ref('')

const CRUMB_KEY = 'pan_crumb'
function persistCrumb() {
  try {
    sessionStorage.setItem(CRUMB_KEY, JSON.stringify(breadcrumb.value))
  } catch {
    /* ignore */
  }
}

function enterFolder(folder) {
  breadcrumb.value = [...breadcrumb.value, { id: folder.id, name: folder.name }]
  persistCrumb()
  clearSelection()
}
function navigateTo(item) {
  // item.id null means root
  if (item.id == null) {
    breadcrumb.value = []
  } else {
    const idx = breadcrumb.value.findIndex((c) => c.id === item.id)
    if (idx >= 0) breadcrumb.value = breadcrumb.value.slice(0, idx + 1)
    else breadcrumb.value = [{ id: item.id, name: item.name }]
  }
  persistCrumb()
  clearSelection()
}

async function load() {
  loading.value = true
  errorMsg.value = ''
  try {
    const [f, d] = await Promise.all([
      listFiles(currentFolderId.value),
      listFolders(currentFolderId.value),
    ])
    files.value = f || []
    // 后端 /api/folders 返回 { folders, breadcrumbs }
    folders.value = d?.folders || []
  } catch (e) {
    errorMsg.value = e.message || '加载失败'
    files.value = []
    folders.value = []
  } finally {
    loading.value = false
  }
}

/* ----------------------------- Selection --------------------------------- */
const selectedFiles = ref(new Set())
const selectedFolders = ref(new Set())
const selectionMode = computed(
  () => selectedFiles.value.size + selectedFolders.value.size > 0
)
const selectedCount = computed(
  () => selectedFiles.value.size + selectedFolders.value.size
)

function toggleSelect(item, kind) {
  const set = kind === 'folder' ? selectedFolders.value : selectedFiles.value
  if (set.has(item.id)) set.delete(item.id)
  else set.add(item.id)
  // Trigger reactivity for Set mutations
  selectedFiles.value = new Set(selectedFiles.value)
  selectedFolders.value = new Set(selectedFolders.value)
}
function clearSelection() {
  selectedFiles.value = new Set()
  selectedFolders.value = new Set()
}
function selectAll() {
  selectedFiles.value = new Set(files.value.map((f) => f.id))
  selectedFolders.value = new Set(folders.value.map((f) => f.id))
}

/* ----------------------------- Preview ----------------------------------- */
const preview = ref({ visible: false, index: 0 })
function openPreview(file) {
  const idx = files.value.findIndex((f) => f.id === file.id)
  preview.value = { visible: true, index: idx < 0 ? 0 : idx }
}

/* ----------------------------- Card events ------------------------------- */
function onCardClick(item, kind) {
  if (kind === 'folder') enterFolder(item)
  else openPreview(item)
}

function downloadFile(file) {
  const a = document.createElement('a')
  a.href = authUrl(file.download_url)
  a.download = file.name
  document.body.appendChild(a)
  a.click()
  a.remove()
}

async function onRename(item, kind) {
  const name = await confirm({
    title: kind === 'folder' ? '重命名文件夹' : '重命名文件',
    inputLabel: '新名称',
    inputValue: item.name,
    confirmText: '保存',
  })
  if (name == null) return
  const trimmed = String(name).trim()
  if (!trimmed) {
    toast.warning('名称不能为空')
    return
  }
  try {
    if (kind === 'folder') await renameFolder(item.id, trimmed)
    else await renameFile(item.id, trimmed)
    toast.success('已重命名')
    await load()
  } catch (e) {
    toast.error(e.message || '重命名失败')
  }
}

async function onRemove(item, kind) {
  const ok = await confirm({
    title: '移入回收站',
    message: `确定将 “${item.name}” 移入回收站？可从回收站恢复。`,
    variant: 'danger',
    confirmText: '删除',
  })
  if (!ok) return
  try {
    if (kind === 'folder') await deleteFolder(item.id)
    else await deleteFile(item.id)
    toast.success('已移入回收站')
    await load()
  } catch (e) {
    toast.error(e.message || '删除失败')
  }
}

/* ----------------------------- Upload ------------------------------------ */
const uploadRef = ref(null)
const showUpload = ref(true)
function onUploaded(fileInfo) {
  if (!files.value.some((f) => f.id === fileInfo.id)) {
    files.value = [fileInfo, ...files.value]
  }
}
function onAllDone() {
  /* keep queue visible; parent already appended items */
}
function onGridDrop(e) {
  const dropped = e.dataTransfer?.files
  if (dropped && dropped.length) {
    uploadRef.value?.addFiles(dropped)
    showUpload.value = true
  }
}

/* ----------------------------- New folder -------------------------------- */
const showNewFolder = ref(false)
const newFolderName = ref('')
const newFolderEl = ref(null)
function openNewFolder() {
  newFolderName.value = ''
  showNewFolder.value = true
  nextTick(() => newFolderEl.value?.focus())
}
async function createNewFolder() {
  const name = newFolderName.value.trim()
  if (!name) {
    toast.warning('请输入文件夹名称')
    return
  }
  try {
    await createFolder(name, currentFolderId.value)
    toast.success('文件夹已创建')
    showNewFolder.value = false
    await load()
  } catch (e) {
    toast.error(e.message || '创建失败')
  }
}

/* ----------------------------- Share ------------------------------------- */
const showShare = ref(false)
const shareFileIds = ref([])
function openShare(file) {
  shareFileIds.value = [file.id]
  showShare.value = true
}
function openBatchShare() {
  shareFileIds.value = Array.from(selectedFiles.value)
  showShare.value = true
}
async function onShareCreated() {
  // Optionally refresh; shares page is separate.
}

/* ----------------------------- Batch ops --------------------------------- */
async function onBatchDelete() {
  const fileIds = Array.from(selectedFiles.value)
  const folderIds = Array.from(selectedFolders.value)
  if (!fileIds.length && !folderIds.length) return
  const ok = await confirm({
    title: '批量删除',
    message: `确定将选中的 ${fileIds.length + folderIds.length} 项移入回收站？`,
    variant: 'danger',
    confirmText: '删除',
  })
  if (!ok) return
  try {
    const res = await batchDelete({ file_ids: fileIds, folder_ids: folderIds })
    toast.success(`已删除 ${res.deleted} 项，失败 ${res.failed} 项`)
    clearSelection()
    await load()
  } catch (e) {
    toast.error(e.message || '批量删除失败')
  }
}

/* Move / Copy dialog */
const moveState = ref({
  open: false,
  mode: 'move', // 'move' | 'copy'
  crumbs: [], // [{ id, name }] inside dialog
  folders: [],
  loading: false,
  conflict: 'rename',
})
const moveTargetId = computed(() =>
  moveState.value.crumbs.length
    ? moveState.value.crumbs[moveState.value.crumbs.length - 1].id
    : null
)
async function loadMoveFolders(parentId) {
  moveState.value.loading = true
  try {
    const res = await listFolders(parentId)
    moveState.value.folders = res?.folders || []
  } catch {
    moveState.value.folders = []
  } finally {
    moveState.value.loading = false
  }
}
function openMoveCopy(mode) {
  if (!selectedCount.value) return
  moveState.value = {
    open: true,
    mode,
    crumbs: [],
    folders: [],
    loading: false,
    conflict: 'rename',
  }
  loadMoveFolders(null)
}
function moveEnter(folder) {
  moveState.value.crumbs = [
    ...moveState.value.crumbs,
    { id: folder.id, name: folder.name },
  ]
  loadMoveFolders(folder.id)
}
function moveUp(idx) {
  moveState.value.crumbs = moveState.value.crumbs.slice(0, idx + 1)
  const parentId = moveTargetId.value
  loadMoveFolders(parentId)
}
function moveToRoot() {
  moveState.value.crumbs = []
  loadMoveFolders(null)
}
async function confirmMoveCopy() {
  const fileIds = Array.from(selectedFiles.value)
  const folderIds = Array.from(selectedFolders.value)
  const target = moveTargetId.value
  try {
    const payload = {
      file_ids: fileIds,
      folder_ids: folderIds,
      target_folder_id: target,
      conflict_strategy: moveState.value.conflict,
    }
    const res =
      moveState.value.mode === 'move'
        ? await batchMove(payload)
        : await batchCopy(payload)
    toast.success(
      `${moveState.value.mode === 'move' ? '移动' : '复制'}成功 ${res.succeeded} 项，跳过 ${res.skipped}，失败 ${res.failed}`
    )
    moveState.value.open = false
    clearSelection()
    await load()
  } catch (e) {
    toast.error(e.message || '操作失败')
  }
}

/* ----------------------------- Lifecycle --------------------------------- */
const isInitializing = ref(true)

watch(currentFolderId, async (id) => {
  if (isInitializing.value) return
  const current = route.query.folder
  const next = id == null ? undefined : String(id)
  if (current !== next) {
    router.replace({ query: { ...route.query, folder: next } })
  }
  await load()
})

onMounted(() => {
  let crumb = []
  try {
    crumb = JSON.parse(sessionStorage.getItem(CRUMB_KEY) || '[]')
  } catch {
    crumb = []
  }
  const q = route.query.folder
  if (q != null && q !== '') {
    const id = Number(q)
    const idx = crumb.findIndex((c) => c.id === id)
    if (idx >= 0) breadcrumb.value = crumb.slice(0, idx + 1)
    else breadcrumb.value = [{ id, name: '当前文件夹' }]
  } else {
    breadcrumb.value = []
  }
  isInitializing.value = false
  load()
})
</script>

<template>
  <div class="home" @drop.prevent="onGridDrop" @dragover.prevent>
    <div class="toolbar card">
      <Breadcrumb :path="breadcrumb" @navigate="navigateTo" />
      <div class="actions">
        <button class="btn btn-sm" @click="openNewFolder">＋ 新建文件夹</button>
        <button
          class="btn btn-sm"
          :class="{ 'btn-primary': showUpload }"
          @click="showUpload = !showUpload"
        >
          ⬆️ 上传
        </button>
        <button v-if="files.length || folders.length" class="btn btn-sm btn-ghost" @click="selectAll">
          全选
        </button>
      </div>
    </div>

    <UploadZone
      v-if="showUpload"
      ref="uploadRef"
      :folder-id="currentFolderId"
      compact
      @uploaded="onUploaded"
      @all-done="onAllDone"
    />

    <!-- Loading -->
    <div v-if="loading" class="grid">
      <div
        v-for="i in 8"
        :key="'sk' + i"
        class="sk-card"
      >
        <div class="skeleton sk-thumb" />
        <div class="skeleton sk-line" />
        <div class="skeleton sk-line short" />
      </div>
    </div>

    <!-- Error -->
    <div v-else-if="errorMsg" class="state">
      <span class="emoji">⚠️</span>
      <h3>加载失败</h3>
      <p>{{ errorMsg }}</p>
      <button class="btn btn-primary btn-sm" @click="load">重试</button>
    </div>

    <!-- Empty -->
    <div
      v-else-if="!folders.length && !files.length"
      class="state"
    >
      <span class="emoji">📂</span>
      <h3>此文件夹为空</h3>
      <p>上传文件或新建文件夹来开始管理</p>
      <button class="btn btn-primary btn-sm" @click="showUpload = true">
        ⬆️ 上传文件
      </button>
    </div>

    <!-- Content -->
    <template v-else>
      <div v-if="folders.length" class="section">
        <h2 class="sec-title">文件夹 ({{ folders.length }})</h2>
        <div class="grid">
          <FileCard
            v-for="f in folders"
            :key="'d' + f.id"
            :item="f"
            kind="folder"
            :selected="selectedFolders.has(f.id)"
            :selectable="selectionMode"
            @click="onCardClick(f, 'folder')"
            @toggle-select="toggleSelect(f, 'folder')"
            @rename="onRename(f, 'folder')"
            @remove="onRemove(f, 'folder')"
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
            :selected="selectedFiles.has(f.id)"
            :selectable="selectionMode"
            @click="onCardClick(f, 'file')"
            @toggle-select="toggleSelect(f, 'file')"
            @rename="onRename(f, 'file')"
            @remove="onRemove(f, 'file')"
            @share="openShare(f)"
            @download="downloadFile(f)"
          />
        </div>
      </div>
    </template>

    <!-- New folder dialog -->
    <Transition name="fade">
      <div v-if="showNewFolder" class="overlay" @mousedown.self="showNewFolder = false">
        <div class="dialog card" role="dialog" aria-modal="true" @keydown.enter="createNewFolder" @keydown.esc="showNewFolder = false">
          <h3>新建文件夹</h3>
          <input
            ref="newFolderEl"
            v-model="newFolderName"
            class="input"
            type="text"
            placeholder="文件夹名称"
          />
          <div class="dialog-actions">
            <button class="btn btn-ghost" @click="showNewFolder = false">取消</button>
            <button class="btn btn-primary" @click="createNewFolder">创建</button>
          </div>
        </div>
      </div>
    </Transition>

    <!-- Move / Copy dialog -->
    <Transition name="fade">
      <div v-if="moveState.open" class="overlay" @mousedown.self="moveState.open = false">
        <div class="dialog move-dialog card" role="dialog" aria-modal="true">
          <div class="row between">
            <h3>{{ moveState.mode === 'move' ? '移动到' : '复制到' }}</h3>
            <button class="btn-icon btn-ghost" @click="moveState.open = false">✕</button>
          </div>
          <div class="move-crumb">
            <button class="crumb-link" :class="{ active: !moveState.crumbs.length }" @click="moveToRoot">
              🏠 根目录
            </button>
            <template v-for="(c, i) in moveState.crumbs" :key="c.id">
              <span class="sep">/</span>
              <button class="crumb-link" :class="{ active: i === moveState.crumbs.length - 1 }" @click="moveUp(i)">
                {{ c.name }}
              </button>
            </template>
          </div>
          <div class="move-list">
            <div v-if="moveState.loading" class="center" style="padding: 24px">
              <div class="spinner" />
            </div>
            <div v-else-if="!moveState.folders.length" class="muted" style="padding: 18px; text-align: center">
              此目录下没有子文件夹
            </div>
            <ul v-else>
              <li v-for="f in moveState.folders" :key="f.id" @dblclick="moveEnter(f)" @click="moveEnter(f)">
                <span class="emoji">📁</span>
                <span class="truncate">{{ f.name }}</span>
                <span class="muted small">{{ f.file_count }} / {{ f.subfolder_count }}</span>
              </li>
            </ul>
          </div>
          <div class="field">
            <label>冲突处理</label>
            <select v-model="moveState.conflict" class="select">
              <option value="rename">重命名（自动加后缀）</option>
              <option value="skip">跳过</option>
              <option value="overwrite">覆盖</option>
            </select>
          </div>
          <div class="dialog-actions">
            <button class="btn btn-ghost" @click="moveState.open = false">取消</button>
            <button class="btn btn-primary" @click="confirmMoveCopy">
              {{ moveState.mode === 'move' ? '移动到此' : '复制到此' }}
            </button>
          </div>
        </div>
      </div>
    </Transition>

    <FilePreview
      :visible="preview.visible"
      :files="files"
      :index="preview.index"
      @close="preview.visible = false"
      @update:index="preview.index = $event"
    />

    <ShareDialog
      v-model:visible="showShare"
      :file-ids="shareFileIds"
      @created="onShareCreated"
    />

    <BatchToolbar
      :selected-count="selectedCount"
      :file-selected-count="selectedFiles.size"
      :folder-selected-count="selectedFolders.size"
      @move="openMoveCopy('move')"
      @copy="openMoveCopy('copy')"
      @delete="onBatchDelete"
      @share="openBatchShare"
      @clear="clearSelection"
    />
  </div>
</template>

<style scoped>
.home {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 14px;
  flex-wrap: wrap;
}
.actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.section {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.sec-title {
  font-size: 0.95rem;
  color: var(--text-heading);
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

.overlay {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
  backdrop-filter: blur(2px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  z-index: 8500;
}
.dialog {
  width: min(92vw, 420px);
  background: var(--bg-elevated);
  padding: 22px;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}
.dialog h3 {
  margin-bottom: 12px;
  font-size: 1.1rem;
}
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 16px;
}

.move-dialog {
  width: min(94vw, 520px);
}
.move-crumb {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  padding: 8px 0 12px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 8px;
}
.crumb-link {
  padding: 4px 8px;
  border-radius: 6px;
  color: var(--text-muted);
  font-size: 0.85rem;
}
.crumb-link:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
}
.crumb-link.active {
  color: var(--primary);
  font-weight: 600;
}
.sep {
  color: var(--text-muted);
}
.move-list {
  max-height: 280px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  margin-bottom: 14px;
}
.move-list ul {
  list-style: none;
  margin: 0;
  padding: 4px;
}
.move-list li {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 0.9rem;
  color: var(--text-heading);
}
.move-list li:hover {
  background: var(--bg-hover);
}
.move-list .emoji {
  font-size: 1.2rem;
}
.small {
  font-size: 0.74rem;
}

@media (max-width: 768px) {
  .grid {
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 10px;
  }
  .toolbar {
    padding: 10px;
  }
}
@media (max-width: 480px) {
  .grid {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>
