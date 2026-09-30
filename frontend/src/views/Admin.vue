<script setup>
import { ref, reactive, inject, onMounted, nextTick } from 'vue'
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
  adminListInviteCodes,
  adminCreateInviteCodes,
  adminDeleteInviteCode,
  adminRunGcCleanup,
  adminGetGcStatus,
  formatDate,
  formatSize,
  parseUtcDate,
} from '../api'
import AppIcon from '../components/AppIcon.vue'
import PasswordInput from '../components/PasswordInput.vue'
import { useToast } from '../composables/useToast'
import { confirm } from '../composables/useConfirm'
import { useModal } from '../composables/useModal'

const toast = useToast()
const currentUser = inject('currentUser', ref(null))

// 出错时把焦点移到对应字段
const cUsernameEl = ref(null)
const cPasswordEl = ref(null)
const cQuotaEl = ref(null)
const eUsernameEl = ref(null)
const ePasswordEl = ref(null)
const eQuotaEl = ref(null)

const stats = ref({})
const users = ref([])
const loadingStats = ref(false)
const loadingUsers = ref(false)
const error = ref('')

// icon 存的是 src/icons.js 注册表里的键名
const statCards = [
  { key: 'users', label: '用户', icon: 'Users' },
  { key: 'files', label: '文件', icon: 'FileText' },
  { key: 'folders', label: '文件夹', icon: 'Folder' },
  { key: 'shares', label: '分享', icon: 'Link' },
  { key: 'trash_items', label: '回收站', icon: 'Trash2' },
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
  const t = parseUtcDate(expiresAt)
  return !!t && t.getTime() < Date.now()
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
const cForm = reactive({ username: '', password: '', role: 'user', expires_at: '', quota: 5 })
const cSaving = ref(false)
const cErr = ref('')

function openCreate() {
  cForm.username = ''
  cForm.password = ''
  cForm.role = 'user'
  cForm.expires_at = ''
  cForm.quota = 5
  if (currentUser.value?.role === 'admin') cForm.role = 'user'
  cErr.value = ''
  showCreate.value = true
}

/** 记下错误并把焦点移到出错的字段——键盘/读屏用户不用自己去找哪里填错了 */
async function failCreate(msg, el) {
  cErr.value = msg
  await nextTick()
  el?.value?.focus()
}

async function submitCreate() {
  cErr.value = ''
  if (!cForm.username.trim()) return failCreate('用户名不能为空', cUsernameEl)
  if (cForm.password.length < 6) return failCreate('密码长度至少 6 位', cPasswordEl)
  cSaving.value = true
  try {
    const quotaGb = Number(cForm.quota)
    if (!Number.isFinite(quotaGb) || quotaGb < 0) {
      cSaving.value = false
      return failCreate('配额必须是不小于 0 的数字', cQuotaEl)
    }
    const payload = {
      username: cForm.username.trim(),
      password: cForm.password,
      role: cForm.role,
      expires_at: fromLocalInput(cForm.expires_at), // null 表示清除/不设
      quota_bytes: Math.round(quotaGb * 1024 * 1024 * 1024),
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
  quotaGb: '',
})
const eSaving = ref(false)
const eErr = ref('')

/** 后端 UTC 字符串 → datetime-local 输入框所需的本地时间（YYYY-MM-DDTHH:MM） */
function toLocalInput(v) {
  const d = parseUtcDate(v)
  if (!d) return ''
  const p = (x) => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(
    d.getHours()
  )}:${p(d.getMinutes())}`
}

/** datetime-local 输入框的本地时间 → 后端 UTC 字符串（YYYY-MM-DD HH:MM:SS） */
function fromLocalInput(v) {
  if (!v) return null
  const d = new Date(v)
  if (Number.isNaN(d.getTime())) return null
  return d.toISOString().slice(0, 19).replace('T', ' ')
}

function openEdit(user) {
  eForm.id = user.id
  eForm.username = user.username
  eForm.role = user.role
  eForm.password = ''
  eForm.expires_at = toLocalInput(user.expires_at)
  eForm.keepExpiry = true
  eForm.quotaGb =
    user.quota_bytes != null ? String(Number((user.quota_bytes / 1073741824).toFixed(2))) : ''
  eErr.value = ''
  showEdit.value = true
}

async function failEdit(msg, el) {
  eErr.value = msg
  await nextTick()
  el?.value?.focus()
}

async function submitEdit() {
  eErr.value = ''
  if (!eForm.username.trim()) return failEdit('用户名不能为空', eUsernameEl)
  if (eForm.password && eForm.password.length < 6) {
    return failEdit('新密码长度至少 6 位', ePasswordEl)
  }
  eSaving.value = true
  try {
    const payload = { username: eForm.username.trim() }
    if (eForm.role) payload.role = eForm.role
    if (eForm.password) payload.password = eForm.password
    if (eForm.quotaGb !== '') {
      const q = Number(eForm.quotaGb)
      if (!Number.isFinite(q) || q < 0) {
        eSaving.value = false
        return failEdit('配额必须是不小于 0 的数字', eQuotaEl)
      }
      payload.quota_bytes = Math.round(q * 1024 * 1024 * 1024)
    }
    // keepExpiry：不传 expires_at（保持不变）；
    // 否则按输入值或 null（清除）提交
    if (!eForm.keepExpiry) {
      payload.expires_at = fromLocalInput(eForm.expires_at)
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

/* ---------------- 对话框行为 ---------------- */
/* 三个模态的焦点陷阱 / Esc / 焦点归还 / 背景 inert。
   这三处原先连 role="dialog" 与 aria-modal 都没有，打开后按 Tab 会直接
   走到背后的用户表格上，Esc 也关不掉。

   注意：这几行必须在 showCreate/showEdit/showUpload 声明之后。
   useModal 内部是 watch(() => showX.value)，watch 会立即求值一次 getter，
   写在 const 之前会命中暂时性死区。 */
const createDialogEl = ref(null)
const editDialogEl = ref(null)
const uploadDialogEl = ref(null)
useModal(() => showCreate.value, {
  container: createDialogEl,
  onClose: () => (showCreate.value = false),
})
useModal(() => showEdit.value, {
  container: editDialogEl,
  onClose: () => (showEdit.value = false),
})
useModal(() => showUpload.value, {
  container: uploadDialogEl,
  onClose: () => (showUpload.value = false),
})

/* ---------------- 注册邀请码 ----------------
   注册制下，摄影师在这里发码给客户。设计取向：码是「照着抄」的东西，
   所以默认一码一人、不过期，并把「复制」放在最显眼的位置——
   发码这个动作的常见形态是「复制 → 粘贴进微信」。 */
const invites = ref([])
const loadingInvites = ref(false)
const inviteError = ref('')
const generatingInvites = ref(false)
const showGenInvites = ref(false)
const inviteForm = reactive({
  count: 1,
  max_uses: 1,
  expires_hours: null,
  note: '',
})

async function loadInvites() {
  loadingInvites.value = true
  inviteError.value = ''
  try {
    const res = await adminListInviteCodes()
    invites.value = res.codes || []
  } catch (err) {
    inviteError.value = err.message || '邀请码加载失败'
  } finally {
    loadingInvites.value = false
  }
}

/* ---------------- 孤儿文件清理 ----------------
   后台每 24 小时自动跑一次；这里是手动入口。
   「上次执行时间」刻意不落盘——重启后重新计 24 小时是符合预期的，
   运维重启后点一次即可。 */
const gcCleaning = ref(false)
const gcStatus = ref(null)

async function loadGcStatus() {
  try {
    gcStatus.value = await adminGetGcStatus()
  } catch {
    // 状态拿不到不影响主流程，按钮仍可用
  }
}

async function runGcCleanup() {
  if (gcCleaning.value) return
  gcCleaning.value = true
  try {
    const r = await adminRunGcCleanup()
    const d = r || {}
    const parts = [
      d.orphan_files ? `孤立文件 ${d.orphan_files} 个` : null,
      d.orphan_previews ? `孤立预览 ${d.orphan_previews} 个` : null,
      d.stale_parts ? `超龄临时文件 ${d.stale_parts} 个` : null,
    ].filter(Boolean)
    toast.success(
      parts.length
        ? `清理完成：${parts.join('、')}（耗时 ${d.elapsed_ms ?? 0} ms）`
        : '清理完成：没有发现需要清理的文件'
    )
  } catch (err) {
    toast.error(err.message || '清理失败')
  } finally {
    gcCleaning.value = false
  }
}

function openGenInvites() {
  inviteForm.count = 1
  inviteForm.max_uses = 1
  inviteForm.expires_hours = null
  inviteForm.note = ''
  showGenInvites.value = true
}

async function generateInvites() {
  generatingInvites.value = true
  try {
    const payload = {
      count: Number(inviteForm.count) || 1,
      max_uses: Number(inviteForm.max_uses) || 1,
      note: inviteForm.note,
    }
    // 空字符串表示「不过期」，不能原样发过去
    if (inviteForm.expires_hours) payload.expires_hours = Number(inviteForm.expires_hours)
    await adminCreateInviteCodes(payload)
    showGenInvites.value = false
    await loadInvites()
    toast.success(`已生成 ${payload.count} 个邀请码`)
  } catch (err) {
    toast.error(err.message || '生成失败')
  } finally {
    generatingInvites.value = false
  }
}

async function removeInvite(c) {
  const ok = await confirm({
    title: '删除邀请码',
    message: c.used_count > 0
      ? `这个码已被 ${c.used_by_username || '某个用户'} 使用过。删除只是让它从列表消失，不影响已创建的账号。`
      : `确定删除邀请码 ${c.code}？尚未使用，删掉后无法找回。`,
    variant: 'danger',
    confirmText: '删除',
  })
  if (!ok) return
  try {
    await adminDeleteInviteCode(c.id)
    toast.success('已删除')
    await loadInvites()
  } catch (err) {
    toast.error(err.message || '删除失败')
  }
}

/* 复制邀请码。navigator.clipboard 只在 HTTPS 或 localhost 下可用——
   通过 Cloudflare Tunnel 访问时是 HTTPS，没问题；但局域网 http 访问
   会静默失败，所以留一个兜底并明确告诉用户。 */
async function copyInvite(c) {
  try {
    await navigator.clipboard.writeText(c.code)
    toast.success(`已复制 ${c.code}`)
  } catch {
    // 退化路径：临时 textarea + execCommand，仍然可用
    const ta = document.createElement('textarea')
    ta.value = c.code
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    try {
      document.execCommand('copy')
      toast.success(`已复制 ${c.code}`)
    } catch {
      toast.error('复制失败，请手动选中复制：' + c.code)
    } finally {
      document.body.removeChild(ta)
    }
  }
}

/* 邀请码状态的中文描述。「用完」和「过期」要分开说：
   前者说明客户已经注册过了，后者说明码本身还有余额但时间到了，
   摄影师据此判断要不要补发。 */
function inviteStatus(c) {
  if (c.used_count >= c.max_uses) {
    return { text: c.used_by_username ? `已用于 ${c.used_by_username}` : '已使用', cls: 'used' }
  }
  // parseUtcDate 对无法解析的脏数据返回 null——此时不判过期，
  // 与后端 is_expired_utc「不可解析时退化为宽松比较」的口径一致。
  const exp = parseUtcDate(c.expires_at)
  if (exp && exp < new Date()) {
    return { text: '已过期', cls: 'expired' }
  }
  if (c.max_uses > 1) return { text: `可用 ${c.max_uses - c.used_count}/${c.max_uses}`, cls: 'ok' }
  return { text: '未使用', cls: 'ok' }
}

/* ---------------- 生命周期 ---------------- */
onMounted(() => {
  loadStats()
  loadUsers()
  loadInvites()
  loadGcStatus()
})</script>

<template>
  <div class="admin">
    <h1 class="sr-only">管理后台</h1>

    <section>
      <h2 class="sec-title">系统统计</h2>
      <div v-if="loadingStats" class="center" style="padding: 32px">
        <div class="spinner" />
      </div>
      <div v-else class="stats-grid">
        <div v-for="c in statCards" :key="c.key" class="stat-card card">
          <AppIcon class="icon" :name="c.icon" size="lg" />
          <div>
            <strong>{{ stats[c.key] ?? 0 }}</strong>
            <small class="muted">{{ c.label }}</small>
          </div>
        </div>
        <div class="stat-card card storage">
          <AppIcon class="icon" name="HardDrive" size="lg" />
          <div>
            <strong>{{ stats.formatted_size || '0 B' }}</strong>
            <small class="muted">总存储</small>
          </div>
        </div>
      </div>

      <!-- 孤儿文件清理。孤儿只在异常路径后产生（进程被杀、用户被删、
           上传中断），不是正常产物，所以后台每 24 小时自动清一次即可。
           手动入口留给「磁盘占用异常，需要立刻处理」的场景。 -->
      <div class="gc-row">
        <button class="btn btn-sm btn-ghost" :disabled="gcCleaning" @click="runGcCleanup">
          <AppIcon name="RotateCcw" size="sm" />
          {{ gcCleaning ? '清理中…' : '立即清理孤儿文件' }}
        </button>
        <span class="muted gc-hint">
          扫描 uploads/ 中没有数据库记录的文件并删除。
          后台每 {{ gcStatus?.auto_interval_hours ?? 24 }} 小时自动执行一次。
          缩略图补生成不受此间隔影响，仍按既定周期独立运行。
        </span>
      </div>
    </section>

    <section>
      <div class="row between">
        <h2 class="sec-title">用户管理</h2>
        <div class="toolbar-actions">
          <button class="btn btn-sm btn-primary" @click="openCreate">
            <AppIcon name="Plus" size="sm" /> 新建用户
          </button>
          <button class="btn btn-sm btn-ghost" @click="loadUsers">刷新</button>
        </div>
      </div>

      <div v-if="loadingUsers" class="center" style="padding: 32px">
        <div class="spinner" />
      </div>
      <div v-else-if="error" class="state">
        <AppIcon class="state-icon" name="CircleAlert" size="xl" />
        <h3>加载失败</h3>
        <p>{{ error }}</p>
        <button class="btn btn-primary btn-sm" @click="loadUsers">重试</button>
      </div>
      <div v-else-if="users.length === 0" class="state">
        <AppIcon class="state-icon" name="Users" size="xl" />
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
              <th>配额</th>
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
              <td>
                <div class="quota-cell">
                  <div class="quota-bar">
                    <div
                      :class="{ over: (u.usage_percent || 0) >= 100 }"
                      :style="{ width: Math.min(100, u.usage_percent || 0) + '%' }"
                    />
                  </div>
                  <span class="quota-text muted small">
                    {{ u.formatted_used || '0 B' }} / {{ formatSize(u.quota_bytes || 0) }}
                  </span>
                </div>
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

    <!-- 注册邀请码 -->
    <section>
      <div class="row between">
        <h2 class="sec-title">注册邀请码</h2>
        <div class="toolbar-actions">
          <button class="btn btn-sm btn-primary" @click="openGenInvites">
            <AppIcon name="Plus" size="sm" /> 生成邀请码
          </button>
          <button class="btn btn-sm btn-ghost" @click="loadInvites">刷新</button>
        </div>
      </div>

      <p class="muted invite-intro">
        本网盘为邀请制。把邀请码发给客户后，客户在登录页点「注册」即可自助开通账号；
        码用一次即失效。
      </p>

      <div v-if="loadingInvites" class="center" style="padding: 32px">
        <div class="spinner" />
      </div>
      <div v-else-if="inviteError" class="state">
        <AppIcon class="state-icon" name="CircleAlert" size="xl" />
        <h3>加载失败</h3>
        <p>{{ inviteError }}</p>
        <button class="btn btn-primary btn-sm" @click="loadInvites">重试</button>
      </div>
      <div v-else-if="invites.length === 0" class="state">
        <AppIcon class="state-icon" name="Lock" size="xl" />
        <h3>还没有邀请码</h3>
        <p class="muted">生成一个发给客户，他们就能自助注册了</p>
        <button class="btn btn-primary btn-sm" @click="openGenInvites">生成邀请码</button>
      </div>
      <div v-else class="table-wrap card">
        <table>
          <thead>
            <tr>
              <th>邀请码</th>
              <th>状态</th>
              <th>备注</th>
              <th>有效期</th>
              <th>创建时间</th>
              <th class="ta-r">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="c in invites" :key="c.id">
              <td>
                <code class="invite-code">{{ c.code }}</code>
              </td>
              <td>
                <span class="pill" :class="inviteStatus(c).cls">{{ inviteStatus(c).text }}</span>
              </td>
              <td class="muted">{{ c.note || '—' }}</td>
              <td class="muted">{{ c.expires_at ? formatDate(c.expires_at) : '永久' }}</td>
              <td class="muted">{{ formatDate(c.created_at) }}</td>
              <td class="ta-r">
                <div class="row-actions">
                  <button
                    class="btn btn-xs btn-ghost"
                    :disabled="!c.usable"
                    :title="c.usable ? '复制到剪贴板' : '该码已不可用'"
                    @click="copyInvite(c)"
                  >
                    <AppIcon name="Copy" size="sm" /> 复制
                  </button>
                  <button class="btn btn-xs btn-danger-ghost" @click="removeInvite(c)">
                    <AppIcon name="Trash2" size="sm" /> 删除
                  </button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <!-- 生成邀请码 -->
    <div v-if="showGenInvites" class="modal-mask" @click.self="showGenInvites = false">
      <div class="modal card" role="dialog" aria-modal="true" aria-labelledby="gen-invite-title">
        <h3 id="gen-invite-title">生成邀请码</h3>

        <label class="field">
          <span>生成数量</span>
          <input v-model.number="inviteForm.count" class="input" type="number" min="1" max="100" />
        </label>

        <label class="field">
          <span>每个码可用次数</span>
          <input v-model.number="inviteForm.max_uses" class="input" type="number" min="1" />
        </label>

        <label class="field">
          <span>有效小时数（留空 = 永久有效）</span>
          <input
            v-model.number="inviteForm.expires_hours"
            class="input"
            type="number"
            min="1"
            placeholder="例如 72 表示 3 天"
          />
        </label>

        <label class="field">
          <span>备注（仅自己可见）</span>
          <input
            v-model="inviteForm.note"
            class="input"
            type="text"
            maxlength="200"
            placeholder="例如：张三夫妇 婚礼"
          />
        </label>

        <div class="row gap" style="margin-top: 18px">
          <button
            class="btn btn-primary"
            style="flex: 1"
            :disabled="generatingInvites"
            @click="generateInvites"
          >
            {{ generatingInvites ? '生成中…' : '生成' }}
          </button>
          <button class="btn btn-ghost" @click="showGenInvites = false">取消</button>
        </div>
      </div>
    </div>

    <!-- 新建用户 -->
    <div v-if="showCreate" class="modal-mask" @click.self="showCreate = false">
      <div
        ref="createDialogEl"
        class="modal card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-user-title"
        tabindex="-1"
      >
        <div class="modal-head">
          <h3 id="create-user-title">新建用户</h3>
          <button class="icon-btn" aria-label="关闭" @click="showCreate = false">
            <AppIcon name="X" size="sm" />
          </button>
        </div>
        <div class="field">
          <label for="create-username">用户名 *</label>
          <input
            id="create-username"
            ref="cUsernameEl"
            v-model="cForm.username"
            class="input"
            type="text"
            placeholder="登录账号"
            :aria-invalid="cErr ? 'true' : undefined"
          />
        </div>
        <div class="field">
          <label for="create-password">初始密码 *（至少 6 位）</label>
          <PasswordInput
            id="create-password"
            ref="cPasswordEl"
            v-model="cForm.password"
            placeholder="初始密码"
            autocomplete="new-password"
            :aria-invalid="cErr ? 'true' : undefined"
          />
        </div>
        <div class="field">
          <label for="create-role">角色</label>
          <select id="create-role" v-model="cForm.role" class="select">
            <option value="user">普通用户</option>
            <option value="admin">管理员</option>
          </select>
        </div>
        <div class="field">
          <label for="create-expires">有效期（可选，留空 = 永久有效）</label>
          <input
            id="create-expires"
            v-model="cForm.expires_at"
            class="input"
            type="datetime-local"
          />
        </div>
        <div class="field">
          <label for="create-quota">网盘配额（GB）</label>
          <input
            id="create-quota"
            ref="cQuotaEl"
            v-model.number="cForm.quota"
            class="input"
            type="number"
            min="0"
            step="0.1"
          />
        </div>
        <p v-if="cErr" id="create-user-error" class="err" role="alert">{{ cErr }}</p>
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
      <div
        ref="editDialogEl"
        class="modal card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="edit-user-title"
        tabindex="-1"
      >
        <div class="modal-head">
          <h3 id="edit-user-title">编辑用户</h3>
          <button class="icon-btn" aria-label="关闭" @click="showEdit = false">
            <AppIcon name="X" size="sm" />
          </button>
        </div>
        <div class="field">
          <label for="edit-username">用户名</label>
          <input
            id="edit-username"
            ref="eUsernameEl"
            v-model="eForm.username"
            class="input"
            type="text"
            :aria-invalid="eErr ? 'true' : undefined"
          />
        </div>
        <div class="field">
          <label for="edit-password">重置密码（留空则不修改）</label>
          <PasswordInput
            id="edit-password"
            ref="ePasswordEl"
            v-model="eForm.password"
            placeholder="留空保持不变"
            autocomplete="new-password"
            :aria-invalid="eErr ? 'true' : undefined"
          />
        </div>
        <div class="field">
          <label for="edit-role">角色</label>
          <select id="edit-role" v-model="eForm.role" class="select">
            <option value="user">普通用户</option>
            <option value="admin">管理员</option>
          </select>
        </div>
        <div class="field">
          <label for="edit-expires">有效期</label>
          <input
            id="edit-expires"
            v-model="eForm.expires_at"
            class="input"
            type="datetime-local"
            :disabled="eForm.keepExpiry"
          />
        </div>
        <div class="field">
          <label for="edit-quota">网盘配额（GB，留空保持不变）</label>
          <input
            id="edit-quota"
            ref="eQuotaEl"
            v-model="eForm.quotaGb"
            class="input"
            type="number"
            min="0"
            step="0.1"
            placeholder="留空保持不变"
          />
        </div>
        <label class="check">
          <input v-model="eForm.keepExpiry" type="checkbox" />
          保留当前有效期（勾选则不更改）
        </label>
        <p v-if="eErr" id="edit-user-error" class="err" role="alert">{{ eErr }}</p>
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
      <div
        ref="uploadDialogEl"
        class="modal card up-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="upload-modal-title"
        tabindex="-1"
      >
        <div class="modal-head">
          <h3 id="upload-modal-title">为「{{ uUser?.username }}」上传原图</h3>
          <button class="icon-btn" aria-label="关闭" @click="showUpload = false">
            <AppIcon name="X" size="sm" />
          </button>
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
            >
              <AppIcon name="Plus" size="sm" /> 新建文件夹
            </button>
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
            <span class="name" :title="f.name">{{ f.name }}</span>
            <span class="muted small">{{ formatSize(f.size) }}</span>
          </li>
        </ul>

        <div v-if="uUploading" class="progress"><div :style="{ width: uProgress + '%' }" /></div>

        <ul v-if="uResults.length" class="result-list" aria-live="polite">
          <li v-for="(r, i) in uResults" :key="i" :class="r.ok ? 'ok' : 'fail'">
            <AppIcon :name="r.ok ? 'Check' : 'CircleX'" size="sm" />
            {{ r.name }}{{ r.msg ? ` · ${r.msg}` : '' }}
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
  color: var(--primary);
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

/* 孤儿清理按钮行：按钮 + 说明并排，说明文字占剩余宽度。
   用 flex-wrap 是为了窄屏下说明能换行而不是被压扁。 */
.gc-row {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-top: 12px;
}
.gc-hint {
  font-size: 0.8rem;
  line-height: 1.5;
  flex: 1;
  min-width: 220px;
}

.table-wrap {
  overflow-x: auto;
  padding: 4px;
}

/* ---------------- 注册邀请码 ----------------
   邀请码的主要动作是「照着抄」——等宽字体 + 字间距让漏抄一眼能看出来。
   letter-spacing 也会让 0/O、1/I 这类混淆字符更难混。 */
.invite-intro {
  font-size: 0.85rem;
  line-height: 1.6;
  margin: 0 0 14px;
}
.invite-code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.95rem;
  letter-spacing: 0.12em;
  font-weight: 600;
  color: var(--text-heading);
  background: var(--bg-hover);
  padding: 4px 8px;
  border-radius: 6px;
  user-select: all; /* 一键全选，方便直接复制 */
}
.pill {
  display: inline-block;
  padding: 3px 9px;
  border-radius: 999px;
  font-size: 0.76rem;
  font-weight: 600;
  white-space: nowrap;
}
.pill.ok {
  background: rgba(34, 197, 94, 0.14);
  color: #16a34a;
}
.pill.used {
  background: var(--bg-hover);
  color: var(--text-muted);
}
.pill.expired {
  background: rgba(234, 179, 8, 0.16);
  color: #b45309;
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
.quota-cell {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 130px;
}
.quota-bar {
  height: 6px;
  border-radius: 999px;
  background: var(--bg-hover);
  overflow: hidden;
}
.quota-bar div {
  height: 100%;
  background: var(--primary);
}
.quota-bar div.over {
  background: var(--danger);
}
.quota-text {
  white-space: nowrap;
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
/* 原为 z-index: 100，低于侧边栏(120)与传输抽屉(140)，打开模态时遮罩盖不住左侧栏 */
.modal-mask {
  position: fixed;
  inset: 0;
  z-index: var(--z-modal);
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
/* .err 已统一到 style.css（原先此处 0.85rem、Auth 0.78rem，已漂移） */
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