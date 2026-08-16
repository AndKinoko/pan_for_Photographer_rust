<script setup>
import { ref, inject, onMounted } from 'vue'
import {
  adminGetStats,
  adminListUsers,
  adminUpdateUserRole,
  adminDeleteUser,
  formatDate,
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
    users.value = (await adminListUsers()) || []
  } catch (e) {
    error.value = e.message || '加载用户失败'
  } finally {
    loadingUsers.value = false
  }
}

async function onRoleChange(user, event) {
  const role = event.target.value
  try {
    await adminUpdateUserRole(user.id, role)
    user.role = role
    toast.success(`已将 “${user.username}” 设为${role === 'admin' ? '管理员' : '普通用户'}`)
  } catch (e) {
    toast.error(e.message || '更新失败')
    // restore select
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
    message: `确定删除用户 “${user.username}”？该用户的文件将被一并删除，无法恢复。`,
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
        <button class="btn btn-sm btn-ghost" @click="loadUsers">刷新</button>
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

      <div v-else class="table-wrap card">
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>用户名</th>
              <th>角色</th>
              <th>注册时间</th>
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
              <td class="muted small">{{ formatDate(u.created_at) }}</td>
              <td>
                <button
                  class="btn btn-sm btn-danger"
                  :disabled="currentUser && u.id === currentUser.id"
                  @click="onDeleteUser(u)"
                >
                  删除
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
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
  min-width: 560px;
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
.small {
  font-size: 0.8rem;
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
.role-select:focus {
  outline: none;
  border-color: var(--primary);
}
@media (max-width: 768px) {
  .stats-grid {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>
