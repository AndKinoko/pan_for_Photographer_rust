<script setup>
import { ref, computed, onMounted, watch, provide } from 'vue'
import { useRoute, useRouter, RouterView } from 'vue-router'
import { getMe } from './api'
import { useTheme } from './composables/useTheme'
import { useToast } from './composables/useToast'
import Toast from './components/Toast.vue'
import ConfirmDialog from './components/ConfirmDialog.vue'

const route = useRoute()
const router = useRouter()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()

const user = ref(null)
const loadingUser = ref(true)
const sidebarOpen = ref(false)

provide('currentUser', user)

const isPublicRoute = computed(() => route.meta?.public === true)

const navItems = computed(() => {
  const items = [
    { to: '/', label: '我的文件', icon: '🗂️' },
    { to: '/search', label: '搜索', icon: '🔍' },
    { to: '/shares', label: '我的分享', icon: '🔗' },
    { to: '/trash', label: '回收站', icon: '🗑️' },
  ]
  if (user.value?.role === 'admin') {
    items.push({ to: '/admin', label: '管理后台', icon: '⚙️' })
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
  }
)

onMounted(loadUser)

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
      <!-- Sidebar (desktop) / drawer (mobile) -->
      <aside
        class="sidebar"
        :class="{ open: sidebarOpen }"
        :aria-hidden="!sidebarOpen"
      >
        <div class="brand">
          <span class="logo">📷</span>
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
            <span class="nav-icon">{{ item.icon }}</span>
            <span>{{ item.label }}</span>
          </RouterLink>
        </nav>
        <div class="sidebar-foot">
          <button class="theme-toggle" @click="toggleTheme">
            <span>{{ theme === 'dark' ? '☀️' : '🌙' }}</span>
            <span>{{ theme === 'dark' ? '浅色模式' : '深色模式' }}</span>
          </button>
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
        <header class="topbar">
          <button
            class="hamburger btn-icon btn-ghost"
            aria-label="菜单"
            @click="sidebarOpen = !sidebarOpen"
          >
            ☰
          </button>
          <h1 class="page-title">{{ route.meta?.title || '我的文件' }}</h1>
          <div class="grow" />
          <div class="user-menu">
            <span class="user-name truncate">
              {{ user?.username || '用户' }}
            </span>
            <span
              v-if="user?.role === 'admin'"
              class="badge"
              title="管理员"
            >
              管理员
            </span>
            <button
              class="theme-btn btn-icon btn-ghost"
              :aria-label="theme === 'dark' ? '切换到浅色' : '切换到深色'"
              @click="toggleTheme"
            >
              {{ theme === 'dark' ? '☀️' : '🌙' }}
            </button>
            <button class="btn btn-sm btn-ghost" @click="logout">
              退出
            </button>
          </div>
        </header>

        <main class="content">
          <RouterView />
        </main>
      </div>
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
  z-index: 120;
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 8px 18px;
}
.logo {
  font-size: 1.6rem;
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
.nav-icon {
  font-size: 1.1rem;
  width: 22px;
  text-align: center;
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
.topbar {
  position: sticky;
  top: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  gap: 12px;
  height: var(--topbar-height);
  padding: 0 18px;
  background: var(--bg-elevated);
  border-bottom: 1px solid var(--border);
}
.hamburger {
  display: none;
}
.page-title {
  font-size: 1.1rem;
  color: var(--text-heading);
}
.user-menu {
  display: flex;
  align-items: center;
  gap: 10px;
}
.user-name {
  max-width: 140px;
  font-weight: 500;
  color: var(--text-heading);
}
.theme-btn {
  display: none;
}
.content {
  flex: 1 1 auto;
  padding: 20px;
  min-width: 0;
}

.backdrop {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
  z-index: 110;
}

/* Mobile: sidebar becomes a drawer */
@media (max-width: 768px) {
  .hamburger {
    display: inline-flex;
  }
  .theme-btn {
    display: inline-flex;
  }
  .user-name {
    max-width: 90px;
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
