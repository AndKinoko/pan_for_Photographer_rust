<script setup>
import { ref, reactive, inject, onMounted } from 'vue'
import {
  adminGetStats,
  adminListUsers,
  adminUpdateUserRole,
  adminDeleteUser,
  adminCreateUser,
  adminUpdateUser,
  adminListUserFolders,
  adminCreateUserFolder,
  adminUploadToUser,
  formatDate,
  formatSize,
} from '../api'
import { useToast } from '../composables/useToast'
import { confirm } from '../composables/useConfirm'

const toast = useToast()
const currentUser = inject('currentUser', ref(null))

const stats = ref({})
const users = ref([])
const loadingStats = ref(false)
const loadingUsers = ref(false)
const error = ref('')

const statCards = [
  { key: 'users', label: '用户', icon: '👥' },
  { key: 'files', label: '文件', icon: '📄' },
  { key: 'folders', label: '文件夹', icon: '📁' },
  { key: 'shares', label: '分享', icon: '🔗' },
  { key: 'trash_items', label: '回收站', icon: '🗑️' },
]

/* ---------------- 加载 ---------------- */
async function loadStats() {
  loadingStats.value = true
  try {
    stats.value = (await adminGetStats()) || {}
  } catch (e) {
    toast.error(e.message || '加载统计失败')
  } finally {
    loadingStats.value = false
  }
}

async function loadUsers() {
  loadingUsers.value = true
  error.value = ''
  try {
    const list = (await adminListUsers()) || []
    // 计算每条用户的过期标记
    users.value = list.map((u) => ({ ...u, _expired: isExpired(u.expires_at) }))
  } catch (e) {
    error.value = e.message || '加载用户失败'
  } finally {
    loadingUsers.value = false
  }
}

function isExpired(expiresAt) {
  if (!expiresAt) return false
  const t = new Date(expiresAt.replace(' ', 'T'))
  return !Number.isNaN(t.getTime()) && t.getTime() < Date.now()
}

function expiryText(u) {
  if (!u.expires_at) return '永久有效'
  if (u._expired) return `已过期 · ${formatDate(u.expires_at)}`
  return `至 ${formatDate(u.expires_at)}`
}

/* ---------------- 角色 / 删除 ---------------- */
async function onRoleChange(user, event) {
  const role = event.target.value
  try {
    await adminUpdateUserRole(user.id, role)
    user.role = role
    toast.success(`已将 “${user.username}” 设为${role === 'admin' ? '管理员' : '普通用户'}`)
  } catch (e) {
    toast.error(e.message || '更新失败')
    event.target.value = user.role
  }
}

async function onDeleteUser(user) {
  if (currentUser.value && user.id === currentUser.value.id) {
    toast.warning('不能删除当前登录的账户')
    return
  }
  const ok = await confirm({
    title: '删除用户',
    message: `确定删除用户 “${user.username}”？其名下文件与分享将一并删除，无法恢复。`,
    variant: 'danger',
    confirmText: '删除用户',
  })
  if (!ok) return
  try {
    await adminDeleteUser(user.id)
    toast.success('用户已删除')
    await loadUsers()
    await loadStats()
  } catch (e) {
    toast.error(e.message || '删除失败')
  }
}

/* ---------------- 新建用户 ---------------- */
const showCreate = ref(false)
const cForm = reactive({ username: '', password: '', role: 'user', expires_at: '' })
const cSaving = ref(false)
const cErr = ref('')

function openCreate() {
  cForm.username = ''
  cForm.password = ''
  cForm.role = 'user'
  cForm.expires_at = ''
  if (currentUser.value?.role === 'admin') cForm.role = 'user'
  cErr.value = ''
  showCreate.value = true
}

async function submitCreate() {
  cErr.value = ''
  if (!cForm.username.trim()) { cErr.value = '用户名不能为空'; return }
  if (cForm.password.length < 6) { cErr.value = '密码长度至少 6 位'; return }
  cSaving.value = true
  try {
    const payload = {
      username: cForm.username.trim(),
      password: cForm.password,
      role: cForm.role,
      expires_at: cForm.expires_at || null, // null 表示清除/不设
    }
    await adminCreateUser(payload)
    toast.success('用户已创建')
    showCreate.value = false
    await loadUsers()
    await loadStats()
  } catch (e) {
    cErr.value = e.message || '创建失败'
  } finally {
    cSaving.value = false
  }
}

/* ---------------- 编辑用户 ---------------- */
const showEdit = ref(false)
const eForm = reactive({
  id: null,
  username: '',
  role: 'user',
  password: '',
  expires_at: '',
  keepExpiry: true,
})
const eSaving = ref(false)
const eErr = ref('')

function toLocal(v) {
  if (!v) return ''
  return v.replace(' ', 'T').slice(0, 16)
}

function openEdit(user) {
  eForm.id = user.id
  eForm.username = user.username
  eForm.role = user.role
  eForm.password = ''
  eForm.expires_at = toLocal(user.expires_at)
  eForm.keepExpiry = true
  eErr.value = ''
  showEdit.value = true
}

async function submitEdit() {
  eErr.value = ''
  if (!eForm.username.trim()) { eErr.value = '用户名不能为空'; return }
  if (eForm.password && eForm.password.length < 6) { eErr.value = '新密码长度至少 6 位'; return }
  eSaving.value = true
  try {
    const payload = { username: eForm.username.trim() }
    if (eForm.role) payload.role = eForm.role
    if (eForm.password) payload.password = eForm.password
    // keepExpiry：不传 expires_at（保持不变）；
    // 否则按输入值或 null（清除）提交
    if (!eForm.keepExpiry) {
      payload.expires_at = eForm.expires_at || null
    }
    await adminUpdateUser(eForm.id, payload)
    toast.success('用户已更新')
    showEdit.value = false
    await loadUsers()
  } catch (e) {
    eErr.value = e.message || '更新失败'
  } finally {
    eSaving.value = false
  }
}

/* ---------------- 为指定用户上传原图 ---------------- */
const showUpload = ref(false)
const uUser = ref(null)
const uFolderList = ref([])
const uFolderId = ref('')
const uNewFolderMode = ref(false)
const uNewFolderName = ref('')
const uFileList = ref([])
const uUploading = ref(false)
const uProgress = ref(0)
const uResults = ref([])
const uLoadingFolders = ref(false)

function openUpload(user) {
  uUser.value = user
  uFolderList.value = []
  uFolderId.value = ''
  uNewFolderMode.value = false
  uNewFolderName.value = ''
  uFileList.value = []
  uUploading.value = false
  uProgress.value = 0
  uResults.value = []
  showUpload.value = true
  loadFolders(user.id)
}

async function loadFolders(userId) {
  uLoadingFolders.value = true
  try {
    const data = (await adminListUserFolders(userId)) || {}
    const folders = data.folders || []
    uFolderList.value = folders
    // 默认选中「原图」文件夹
    const hasFiles = folders.some((f) => f.name === '原图')
    if (hasFiles) {
      const orig = folders.find((f) => f.name === '原图')
      uFolderId.value = orig ? String(orig.id) : ''
    }
  } catch (e) {
    toast.error(e.message || '加载文件夹失败')
  } finally {
    uLoadingFolders.value = false
  }
}

async function submitNewFolder() {
  const name = uNewFolderName.value.trim()
  if (!name) { toast.warning('请输入文件夹名称'); return }
  try {
    await adminCreateUserFolder(uUser.value.id, name, null)
    uNewFolderName.value = ''
    uNewFolderMode.value = false
    await loadFolders(uUser.value.id)
    toast.success('文件夹已创建')
  } catch (e) {
    toast.error(e.message || '新建失败')
  }
}

function onPickFiles(event) {
  uFileList.value = Array.from(event.target.files || [])
}

async function startUpload() {
  if (!uUser.value) return
  if (uFileList.value.length === 0) { toast.warning('请选择要上传的文件'); return }
  if (!uFolderId.value) { toast.warning('请选择目标文件夹'); return }

  uUploading.value = true
  uProgress.value = 0
  uResults.value = []
  const total = uFileList.value.length
  let ok = 0
  let fail = 0
  const failures = []
  const folderId = uFolderId.value ? Number(uFolderId.value) : null
  const user = uUser.value

  // 逐文件串行上传，避免瞬时并发过大
  for (let i = 0; i < total; i++) {
    const f = uFileList.value[i]
    try {
      const data = await adminUploadToUser(user.id, folderId, f, (e) => {
        if (e.total) {
          const per = i + (e.loaded / e.total)
          uProgress.value = Math.round((per / total) * 100)
        }
      })
      const errs = data?.errors || []
      const files = data?.files || []
      if (files.length) {
        ok += 1
        uResults.value.push({ name: f.name, ok: true })
      } else {
        fail += 1
        failures.push(`${f.name}（${errs[0] || '未知错误'}）`)
        uResults.value.push({ name: f.name, ok: false, msg: errs[0] || '未知错误' })
      }
    } catch (e) {
      fail += 1
      failures.push(`${f.name}（${e.message || '上传失败'}）`)
      uResults.value.push({ name: f.name, ok: false, msg: e.message || '上传失败' })
    }
    uProgress.value = Math.round(((i + 1) / total) * 100)
  }

  uUploading.value = false
  if (fail === 0) {
    toast.success(`共上传 ${ok} 个文件`)
  } else {
    toast.error(`成功 ${ok} 个，失败 ${fail} 个`)
    if (failures.length) console.warn('上传失败项：', failures)
  }
}

/* ---------------- 生命周期 ---------------- */
onMounted(() => {
  loadStats()
  loadUsers()
})
</script>

<template>
  <div class="admin">
    <section>
      <h2 class="sec-title">系统统计</h2>
      <div v-if="loadingStats" class="center" style="padding: 32px">
        <div class="spinner" />
      </div>
      <div v-else class="stats-grid">
        <div v-for="c in statCards" :key="c.key" class="stat-card card">
          <span class="icon">{{ c.icon }}</span>
          <div>
            <strong>{{ stats[c.key] ?? 0 }}</strong>
            <small class="muted">{{ c.label }}</small>
          </div>
        </div>
        <div class="stat-card card storage">
          <span class="icon">💾</span>
          <div>
            <strong>{{ stats.formatted_size || '0 B' }}</strong>
            <small class="muted">总存储</small>
          </div>
        </div>
      </div>
    </section>

    <section>
      <div class="row between">
        <h2 class="sec-title">用户管理</h2>
        <div class="toolbar-actions">
          <button class="btn btn-sm btn-primary" @click="openCreate">＋ 新建用户</button>
          <button class="btn btn-sm btn-ghost" @click="loadUsers">刷新</button>
        </div>
      </div>

      <div v-if="loadingUsers" class="center" style="padding: 32px">
        <div class="spinner" />
      </div>
      <div v-else-if="error" class="state">
        <span class="emoji">⚠️</span>
        <h3>加载失败</h3>
        <p>{{ error }}</p>
        <button class="btn btn-primary btn-sm" @click="loadUsers">重试</button>
      </div>
      <div v-else-if="users.length === 0" class="state">
        <span class="emoji">👥</span>
        <h3>暂无用户</h3>
        <button class="btn btn-primary btn-sm" @click="openCreate">新建用户</button>
      </div>

      <div v-else class="table-wrap card">
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>用户名</th>
              <th>角色</th>
              <th>有效期</th>
              <th>文件数</th>
              <th>创建时间</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="u in users" :key="u.id">
              <td class="muted">{{ u.id }}</td>
              <td class="uname">
                {{ u.username }}
                <span
                  v-if="currentUser && u.id === currentUser.id"
                  class="badge"
                >你</span>
              </td>
              <td>
                <select
                  class="role-select"
                  :value="u.role"
                  @change="onRoleChange(u, $event)"
                >
                  <option value="user">普通用户</option>
                  <option value="admin">管理员</option>
                </select>
              </td>
              <td>
                <span
                  class="expiry"
                  :class="{ expired: u._expired }"
                >{{ expiryText(u) }}</span>
              </td>
              <td class="muted">{{ u.file_count || 0 }}</td>
              <td class="muted small">{{ formatDate(u.created_at) }}</td>
              <td>
                <div class="row-ops">
                  <button
                    class="btn btn-sm btn-ghost"
                    title="为该用户上传原图"
                    @click="openUpload(u)"
                  >上传</button>
                  <button
                    class="btn btn-sm btn-ghost"
                    @click="openEdit(u)"
                  >编辑</button>
                  <button
                    class="btn btn-sm btn-danger"
                    :disabled="currentUser && u.id === currentUser.id"
                    @click="onDeleteUser(u)"
                  >删除</button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <!-- 新建用户 -->
    <div v-if="showCreate" class="modal-mask" @click.self="showCreate = false">
      <div class="modal card">
        <div class="modal-head">
          <h3>新建用户</h3>
          <button class="icon-btn" @click="showCreate = false">✕</button>
        </div>
        <div class="field">
          <label>用户名 *</label>
          <input v-model="cForm.username" class="input" type="text" placeholder="登录账号" />
        </div>
        <div class="field">
          <label>初始密码 *（至少 6 位）</label>
          <input v-model="cForm.password" class="input" type="password" placeholder="初始密码" autocomplete="new-password" />
        </div>
        <div class="field">
          <label>角色</label>
          <select v-model="cForm.role" class="select">
            <option value="user">普通用户</option>
            <option value="admin">管理员</option>
          </select>
        </div>
        <div class="field">
          <label>有效期（可选，留空 = 永久有效）</label>
          <input v-model="cForm.expires_at" class="input" type="datetime-local" />
        </div>
        <p v-if="cErr" class="err">{{ cErr }}</p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="showCreate = false">取消</button>
          <button class="btn btn-primary" :disabled="cSaving" @click="submitCreate">
            {{ cSaving ? '创建中…' : '创建' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 编辑用户 -->
    <div v-if="showEdit" class="modal-mask" @click.self="showEdit = false">
      <div class="modal card">
        <div class="modal-head">
          <h3>编辑用户</h3>
          <button class="icon-btn" @click="showEdit = false">✕</button>
        </div>
        <div class="field">
          <label>用户名</label>
          <input v-model="eForm.username" class="input" type="text" />
        </div>
        <div class="field">
          <label>重置密码（留空则不修改）</label>
          <input v-model="eForm.password" class="input" type="password" placeholder="留空保持不变" autocomplete="new-password" />
        </div>
        <div class="field">
          <label>角色</label>
          <select v-model="eForm.role" class="select">
            <option value="user">普通用户</option>
            <option value="admin">管理员</option>
          </select>
        </div>
        <div class="field">
          <label>有效期</label>
          <input v-model="eForm.expires_at" class="input" type="datetime-local" :disabled="eForm.keepExpiry" />
        </div>
        <label class="check">
          <input v-model="eForm.keepExpiry" type="checkbox" />
          保留当前有效期（勾选则不更改）
        </label>
        <p v-if="eErr" class="err">{{ eErr }}</p>
        <div class="modal-actions">
          <button class="btn btn-ghost" @click="showEdit = false">取消</button>
          <button class="btn btn-primary" :disabled="eSaving" @click="submitEdit">
            {{ eSaving ? '保存中…' : '保存' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 为指定用户上传原图 -->
    <div v-if="showUpload" class="modal-mask" @click.self="showUpload = false">
      <div class="modal card up-modal">
        <div class="modal-head">
          <h3>为「{{ uUser?.username }}」上传原图</h3>
          <button class="icon-btn" @click="showUpload = false">✕</button>
        </div>

        <div class="field">
          <label>目标文件夹</label>
          <div class="row gap">
            <select
              v-model="uFolderId"
              class="select grow"
              :disabled="uLoadingFolders || uUploading"
            >
              <option value="" disabled>选择文件夹</option>
              <option v-for="f in uFolderList" :key="f.id" :value="String(f.id)">
                {{ f.name }}
              </option>
            </select>
            <button
              class="btn btn-sm btn-ghost"
              :disabled="uUploading"
              @click="uNewFolderMode = !uNewFolderMode"
            >＋ 新建文件夹</button>
          </div>
          <div v-if="uNewFolderMode" class="row gap" style="margin-top: 8px">
            <input
              v-model="uNewFolderName"
              class="input grow"
              type="text"
              placeholder="新文件夹名称"
              @keydown.enter="submitNewFolder"
            />
            <button class="btn btn-sm btn-primary" @click="submitNewFolder">创建</button>
          </div>
        </div>

        <label class="drop" :class="{ busy: uUploading }">
          <input
            type="file"
            multiple
            hidden
            :disabled="uUploading"
            @change="onPickFiles"
          />
          <span class="drop-text">
            点击选择或拖拽图片至此
            <small class="muted">支持批量选择多张图片</small>
          </span>
        </label>

        <ul v-if="uFileList.length" class="drop-list">
          <li v-for="(f, i) in uFileList" :key="i">
            <span class="name">{{ f.name }}</span>
            <span class="muted small">{{ formatSize(f.size) }}</span>
          </li>
        </ul>

        <div v-if="uUploading" class="progress"><div :style="{ width: uProgress + '%' }" /></div>

        <ul v-if="uResults.length" class="result-list">
          <li v-for="(r, i) in uResults" :key="i" :class="r.ok ? 'ok' : 'fail'">
            {{ r.ok ? '✅' : '❌' }} {{ r.name }}{{ r.msg ? ` · ${r.msg}` : '' }}
          </li>
        </ul>

        <div class="modal-actions">
          <button class="btn btn-primary" :disabled="uUploading || !uFileList.length" @click="startUpload">
            {{ uUploading ? `上传中 ${uProgress}%…` : `上传 ${uFileList.length ? uFileList.length + ' 个文件' : ''}` }}
          </button>
          <button class="btn btn-ghost" :disabled="uUploading" @click="showUpload = false">完成</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.admin {
  display: flex;
  flex-direction: column;
  gap: 22px;
}
.sec-title {
  font-size: 1.1rem;
  margin-bottom: 12px;
}
.toolbar-actions {
  display: flex;
  gap: 8px;
}
.stats-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 14px;
}
.stat-card {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 16px 18px;
}
.stat-card .icon {
  font-size: 1.8rem;
}
.stat-card strong {
  display: block;
  font-size: 1.5rem;
  color: var(--text-heading);
  font-weight: 700;
}
.stat-card small {
  font-size: 0.78rem;
}
.stat-card.storage {
  grid-column: span 1;
}

.table-wrap {
  overflow-x: auto;
  padding: 4px;
}
table {
  width: 100%;
  border-collapse: collapse;
  min-width: 720px;
}
th,
td {
  text-align: left;
  padding: 12px 14px;
  border-bottom: 1px solid var(--border);
  font-size: 0.9rem;
  color: var(--text-heading);
}
th {
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
tbody tr:last-child td {
  border-bottom: none;
}
tbody tr:hover {
  background: var(--bg-hover);
}
.uname {
  font-weight: 600;
}
.expiry.expired {
  color: var(--danger);
  font-weight: 600;
}
.small {
  font-size: 0.8rem;
}
.row-ops {
  display: flex;
  gap: 6px;
  align-items: center;
  flex-wrap: wrap;
}
.role-select {
  min-height: 36px;
  padding: 0 8px;
  background: var(--bg-input);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text-heading);
  font-size: 0.85rem;
}

/* 模态框 */
.modal-mask {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}
.modal {
  width: min(92vw, 440px);
  max-height: 90vh;
  overflow: auto;
  padding: 22px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}
.up-modal {
  width: min(92vw, 520px);
}
.modal-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.modal-head h3 {
  font-size: 1.05rem;
}
.icon-btn {
  width: 32px;
  height: 32px;
  border-radius: 50%;
  background: var(--bg-hover);
  border: 1px solid transparent;
  color: var(--text-muted);
  cursor: pointer;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.field label {
  font-size: 0.82rem;
  color: var(--text-muted);
}
.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 4px;
}
.err {
  color: var(--danger);
  font-size: 0.85rem;
}
.check {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 0.85rem;
  color: var(--text-muted);
}
.row.gap {
  gap: 8px;
}
.grow {
  flex: 1 1 auto;
}

/* 上传控件 */
.drop {
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1.5px dashed var(--border);
  border-radius: var(--radius);
  padding: 26px 16px;
  text-align: center;
  cursor: pointer;
  transition: border-color 0.15s;
}
.drop:hover {
  border-color: var(--primary);
}
.drop.busy {
  opacity: 0.6;
  pointer-events: none;
}
.drop-text {
  font-size: 0.9rem;
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--text-heading);
}
.drop-list,
.result-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 180px;
  overflow: auto;
}
.drop-list li {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  font-size: 0.85rem;
  padding: 6px 8px;
  background: var(--bg-hover);
  border-radius: var(--radius-sm);
}
.drop-list .name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.result-list li {
  font-size: 0.82rem;
}
.result-list li.ok {
  color: var(--success);
}
.result-list li.fail {
  color: var(--danger);
}
.progress {
  height: 8px;
  border-radius: 999px;
  background: var(--bg-hover);
  overflow: hidden;
}
.progress div {
  height: 100%;
  background: var(--primary);
  transition: width 0.15s;
}

@media (max-width: 768px) {
  .stats-grid {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>