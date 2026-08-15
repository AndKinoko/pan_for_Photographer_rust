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
import { useToast } from '../composables/useToast'
import { confirm } from '../composables/useConfirm'

const toast = useToast()

const files = ref([])
const folders = ref([])
const loading = ref(false)
const error = ref('')

const total = () => files.value.length + folders.value.length

async function load() {
  loading.value = true
  error.value = ''
  try {
    const data = await listTrash()
    files.value = data.files || []
    folders.value = data.folders || []
  } catch (e) {
    error.value = e.message || '加载失败'
  } finally {
    loading.value = false
  }
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
        <h2>回收站</h2>
        <p class="muted">共 {{ total() }} 项已删除</p>
      </div>
      <button
        class="btn btn-sm btn-danger"
        :disabled="!total() || loading"
        @click="onEmpty"
      >
        🧹 清空回收站
      </button>
    </div>

    <div v-if="loading" class="center" style="padding: 48px">
      <div class="spinner" />
    </div>

    <div v-else-if="error" class="state">
      <span class="emoji">⚠️</span>
      <h3>加载失败</h3>
      <p>{{ error }}</p>
      <button class="btn btn-primary btn-sm" @click="load">重试</button>
    </div>

    <div v-else-if="!total()" class="state">
      <span class="emoji">♻️</span>
      <h3>回收站为空</h3>
      <p>删除的文件会出现在这里，30 天内可恢复</p>
    </div>

    <template v-else>
      <div v-if="folders.length" class="section">
        <h3 class="sec-title">文件夹 ({{ folders.length }})</h3>
        <ul class="list card">
          <li v-for="f in folders" :key="'d' + f.id">
            <span class="emoji">📁</span>
            <div class="li-main">
              <div class="li-name truncate">{{ f.name }}</div>
              <div class="li-sub muted">
                删除于 {{ formatDate(f.deleted_at) }}
              </div>
            </div>
            <div class="li-actions">
              <button class="btn btn-sm" @click="onRestoreFolder(f)">♻️ 恢复</button>
              <button class="btn btn-sm btn-danger" @click="onPermanentFolder(f)">永久删除</button>
            </div>
          </li>
        </ul>
      </div>

      <div v-if="files.length" class="section">
        <h3 class="sec-title">文件 ({{ files.length }})</h3>
        <ul class="list card">
          <li v-for="f in files" :key="'f' + f.id">
            <span class="thumb">
              <img
                v-if="f.thumb_url"
                :src="authUrl(f.thumb_url)"
                alt=""
                @error="$event.target.style.display = 'none'"
              />
              <span v-else class="emoji">{{ fileIcon(f.file_type, f.name) }}</span>
            </span>
            <div class="li-main">
              <div class="li-name truncate">{{ f.name }}</div>
              <div class="li-sub muted">
                {{ f.formatted_size || formatSize(f.size) }} · 删除于 {{ formatDate(f.deleted_at) }}
              </div>
            </div>
            <div class="li-actions">
              <button class="btn btn-sm" @click="onRestoreFile(f)">♻️ 恢复</button>
              <button class="btn btn-sm btn-danger" @click="onPermanentFile(f)">永久删除</button>
            </div>
          </li>
        </ul>
      </div>
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
.head h2 {
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
.thumb .emoji {
  font-size: 1.4rem;
}
.list .emoji {
  font-size: 1.5rem;
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
