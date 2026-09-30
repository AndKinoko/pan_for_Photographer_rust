<script setup>
import {
  ref,
  computed,
  onMounted,
  onBeforeUnmount,
  watch,
  nextTick,
  provide,
} from 'vue'
import { useRoute, useRouter, RouterView } from 'vue-router'
import { getMe, formatSize } from './api'
import { useTheme } from './composables/useTheme'
import { useToast } from './composables/useToast'
import { useTransfer } from './composables/useTransfer'
import Toast from './components/Toast.vue'
import ConfirmDialog from './components/ConfirmDialog.vue'
import TransferDrawer from './components/TransferDrawer.vue'
import AppIcon from './components/AppIcon.vue'

const route = useRoute()
const router = useRouter()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()
const transfer = useTransfer()

const activeTransferCount = computed(
  () => transfer.uploadActiveCount.value + transfer.downloadActiveCount.value
)

const user = ref(null)
const loadingUser = ref(true)
const sidebarOpen = ref(false)
const mainEl = ref(null)

/* 路由切换后把焦点移到主内容区。
   只监听 route.path 而不含 query：Home 的目录导航用 ?folder= 表达同一路径下的
   不同目录，那种情况焦点应该留在文件网格里，不该被抢走。 */
watch(
  () => route.path,
  async () => {
    await nextTick()
    mainEl.value?.focus()
  }
)

provide('currentUser', user)

const isPublicRoute = computed(() => route.meta?.public === true)

// icon 存的是 src/icons.js 注册表里的键名，交给 <AppIcon> 渲染
const navItems = computed(() => {
  const items = [
    { to: '/', label: '我的文件', icon: 'Files' },
    { to: '/search', label: '搜索', icon: 'Search' },
    { to: '/shares', label: '我的分享', icon: 'Link' },
    { to: '/trash', label: '回收站', icon: 'Trash2' },
  ]
  if (user.value?.role === 'admin') {
    items.push({ to: '/admin', label: '管理后台', icon: 'Settings' })
  }
  return items
})

async function loadUser() {
  const token = localStorage.getItem('token')
  if (!token) {
    loadingUser.value = false
    user.value = null
    return
  }
  try {
    const data = await getMe()
    user.value = data
    localStorage.setItem('user', JSON.stringify(data))
  } catch {
    user.value = null
  } finally {
    loadingUser.value = false
  }
}

function logout() {
  localStorage.removeItem('token')
  localStorage.removeItem('user')
  user.value = null
  toast.info('已退出登录')
  router.push('/login')
}

watch(
  () => route.fullPath,
  () => {
    sidebarOpen.value = false
    contextCollapsed.value = false
  }
)

const storagePct = computed(() => {
  const quota = user.value?.quota_bytes || 0
  const used = user.value?.used_bytes || 0
  if (quota <= 0) return 100
  return Math.min(100, Math.round((used / quota) * 100))
})

let offUploadComplete = null
// 移动端顶栏第二排（上下文栏）滚动自动收起
const contextCollapsed = ref(false)
function onWinScroll() {
  contextCollapsed.value = window.scrollY > 48
}
onMounted(() => {
  loadUser()
  // 上传完成后刷新用户信息，同步最新容量用量
  offUploadComplete = transfer.onUploadComplete(() => loadUser())
  window.addEventListener('scroll', onWinScroll, { passive: true })
})
onBeforeUnmount(() => {
  if (offUploadComplete) offUploadComplete()
  window.removeEventListener('scroll', onWinScroll)
})

// Keep user state fresh when token appears (e.g. after login redirect).
router.afterEach(() => {
  if (!user.value && localStorage.getItem('token')) {
    loadUser()
  }
})
</script>

<template>
  <!-- Public routes (auth / public share) render without the app shell -->
  <RouterView v-if="isPublicRoute" />
  <div v-else-if="loadingUser || !user" class="boot">
    <div class="spinner" />
    <p class="muted">加载中…</p>
  </div>
  <div v-else class="layout">
      <!-- 桌面端侧边栏有 5 个导航项挡在内容之前，键盘用户每次都要逐个 Tab 过去 -->
      <a href="#main" class="skip-link">跳到主要内容</a>

      <!-- Sidebar (desktop) / drawer (mobile) -->
      <aside
        class="sidebar"
        :class="{ open: sidebarOpen }"
        :aria-hidden="!sidebarOpen"
      >
        <div class="brand">
          <AppIcon class="logo" name="Camera" size="lg" />
          <div class="brand-text">
            <strong>摄影师网盘</strong>
            <small>Pan for Photographer</small>
          </div>
        </div>
        <nav class="nav">
          <RouterLink
            v-for="item in navItems"
            :key="item.to"
            :to="item.to"
            class="nav-item"
            @click="sidebarOpen = false"
          >
            <AppIcon class="nav-icon" :name="item.icon" size="sm" />
            <span>{{ item.label }}</span>
          </RouterLink>
          <!-- 传输抽屉入口（非路由，点击弹出上传/下载队列） -->
          <button
            class="nav-item transfer-entry"
            @click="transfer.openDrawer('upload')"
          >
            <AppIcon class="nav-icon" name="Package" size="sm" />
            <span>传输</span>
            <span v-if="activeTransferCount" class="badge-transfer">
              {{ activeTransferCount }}
            </span>
          </button>
        </nav>
        <div class="sidebar-foot">
          <div class="user-box">
            <div class="user-row">
              <span class="user-name truncate" :title="user?.username">
                {{ user?.username || '用户' }}
              </span>
              <span v-if="user?.role === 'admin'" class="badge" title="管理员">管理员</span>
            </div>
            <div class="quota-bar">
              <div
                class="quota-fill"
                :class="{ over: storagePct >= 100 }"
                :style="{ width: storagePct + '%' }"
              />
            </div>
            <div class="quota-text muted small">
              {{ formatSize(user?.used_bytes || 0) }} / {{ formatSize(user?.quota_bytes || 0) }}
            </div>
            <div class="user-actions">
              <button
                class="theme-btn btn-icon btn-ghost"
                :aria-label="theme === 'dark' ? '切换到浅色' : '切换到深色'"
                @click="toggleTheme"
              >
                <AppIcon :name="theme === 'dark' ? 'Sun' : 'Moon'" size="sm" />
              </button>
              <button class="btn btn-sm btn-ghost logout-btn" @click="logout">退出</button>
            </div>
          </div>
        </div>
      </aside>
      <Transition name="fade">
        <div
          v-if="sidebarOpen"
          class="backdrop"
          @click="sidebarOpen = false"
        />
      </Transition>

      <!-- Main column -->
      <div class="main">
        <header class="mobile-topbar">
          <div class="mobile-row">
            <button
              class="btn-icon btn-ghost"
              aria-label="菜单"
              @click="sidebarOpen = !sidebarOpen"
            >
              <AppIcon name="Menu" size="sm" />
            </button>
            <h1 class="mobile-title">{{ route.meta?.title || '我的文件' }}</h1>
            <button
              class="mobile-transfer btn-icon btn-ghost"
              aria-label="传输"
              @click="transfer.openDrawer('upload')"
            >
              <AppIcon name="Package" size="sm" />
              <span v-if="activeTransferCount" class="badge-transfer">
                {{ activeTransferCount }}
              </span>
            </button>
          </div>
          <!-- 上下文栏：内容由各页面 Teleport 注入（如 Home 的面包屑），空时自动隐藏 -->
          <div class="mobile-context" :class="{ collapsed: contextCollapsed }">
            <div id="mobile-context-bar" class="mobile-context-inner"></div>
          </div>
        </header>

        <main id="main" ref="mainEl" class="content" tabindex="-1">
          <RouterView />
        </main>
      </div>

      <TransferDrawer />
    </div>

  <Toast />
  <ConfirmDialog />
</template>

<style scoped>
.boot {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
}

.layout {
  display: flex;
  min-height: 100vh;
}

.sidebar {
  width: var(--sidebar-width);
  flex: 0 0 var(--sidebar-width);
  background: var(--bg-elevated);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  padding: 16px 12px;
  position: sticky;
  top: 0;
  height: 100vh;
  z-index: var(--z-sidebar);
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 8px 18px;
}
.logo {
  color: var(--primary);
}
.brand-text {
  display: flex;
  flex-direction: column;
  line-height: 1.15;
}
.brand-text strong {
  color: var(--text-heading);
  font-size: 1rem;
}
.brand-text small {
  color: var(--text-muted);
  font-size: 0.72rem;
}
.nav {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: 1 1 auto;
}
.nav-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 14px;
  border-radius: var(--radius-sm);
  color: var(--text);
  font-size: 0.92rem;
  font-weight: 500;
  text-decoration: none;
  min-height: 44px;
  transition: background-color 0.15s ease, color 0.15s ease;
}
.nav-item:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
  text-decoration: none;
}
.nav-item.router-link-active {
  background: var(--primary-soft);
  color: var(--primary);
}
.nav-item.transfer-entry {
  border: none;
  width: 100%;
  cursor: pointer;
}
.nav-icon {
  width: 22px;
  justify-content: center;
}
.badge-transfer {
  margin-left: auto;
  min-width: 20px;
  height: 20px;
  padding: 0 6px;
  border-radius: 999px;
  background: var(--primary);
  color: #fff;
  font-size: 0.72rem;
  font-weight: 700;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.sidebar-foot {
  margin-top: 8px;
}
.theme-toggle {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 14px;
  border-radius: var(--radius-sm);
  color: var(--text);
  font-size: 0.9rem;
  min-height: 44px;
}
.theme-toggle:hover {
  background: var(--bg-hover);
}

.main {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.mobile-topbar {
  display: none;
}
.user-box {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  border-radius: var(--radius-sm);
  background: var(--bg-hover);
}
.user-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.user-name {
  min-width: 0;
  font-weight: 600;
  color: var(--text-heading);
}
.quota-bar {
  height: 6px;
  border-radius: 999px;
  background: var(--border);
  overflow: hidden;
}
.quota-fill {
  height: 100%;
  background: var(--primary);
  transition: width 0.2s ease;
}
.quota-fill.over {
  background: var(--danger);
}
.user-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.theme-btn {
  flex: 0 0 auto;
}
.logout-btn {
  flex: 1 1 auto;
}
.content {
  flex: 1 1 auto;
  padding: 20px;
  min-width: 0;
  /* tabindex="-1" 是给路由切换后程序化聚焦用的，不该出现聚焦框 */
  outline: none;
}

.backdrop {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
  z-index: var(--z-backdrop);
}

/* Mobile: sidebar becomes a drawer, dedicated compact topbar */
@media (max-width: 768px) {
  .mobile-topbar {
    display: flex;
    flex-direction: column;
    position: sticky;
    top: 0;
    z-index: var(--z-sticky);
    background: var(--bg-elevated);
    border-bottom: 1px solid var(--border);
  }
  .mobile-row {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 44px;
    padding: 0 10px;
  }
  .mobile-title {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 1rem;
    color: var(--text-heading);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mobile-transfer {
    position: relative;
  }
  .mobile-transfer .badge-transfer {
    position: absolute;
    top: 2px;
    right: 0;
    margin-left: 0;
  }
  /* 上下文栏：有内容时 34px，滚动时收起；无内容时整体隐藏 */
  .mobile-context {
    max-height: 34px;
    overflow: hidden;
    transition: max-height 0.2s ease;
  }
  .mobile-context.collapsed {
    max-height: 0;
  }
  .mobile-context-inner {
    display: flex;
    align-items: center;
    padding: 2px 10px;
  }
  .mobile-context-inner:empty {
    display: none;
  }
  /* Teleport 进来的面包屑在窄栏内压缩：不换行、去最小高度 */
  .mobile-context-inner :deep(.breadcrumb) {
    min-height: 0;
    padding: 0;
    flex-wrap: nowrap;
    overflow: hidden;
  }
  .sidebar {
    position: fixed;
    left: 0;
    top: 0;
    transform: translateX(-100%);
    transition: transform 0.25s ease;
    box-shadow: var(--shadow-lg);
  }
  .sidebar.open {
    transform: translateX(0);
  }
  .content {
    padding: 14px;
  }
}
</style>
