import { createRouter, createWebHistory } from 'vue-router'

const routes = [
  {
    path: '/login',
    name: 'login',
    component: () => import('./views/Auth.vue'),
    meta: { public: true, title: '登录' },
  },
  {
    path: '/register',
    name: 'register',
    component: () => import('./views/Auth.vue'),
    meta: { public: true, title: '注册' },
  },
  {
    path: '/share/:id',
    name: 'public-share',
    component: () => import('./views/PublicShare.vue'),
    meta: { public: true, title: '分享' },
  },
  {
    path: '/',
    name: 'home',
    component: () => import('./views/Home.vue'),
    meta: { title: '我的文件' },
  },
  {
    path: '/search',
    name: 'search',
    component: () => import('./views/Search.vue'),
    meta: { title: '搜索' },
  },
  {
    path: '/shares',
    name: 'shares',
    component: () => import('./views/Shares.vue'),
    meta: { title: '我的分享' },
  },
  {
    path: '/trash',
    name: 'trash',
    component: () => import('./views/Trash.vue'),
    meta: { title: '回收站' },
  },
  {
    path: '/admin',
    name: 'admin',
    component: () => import('./views/Admin.vue'),
    meta: { title: '管理后台', admin: true },
  },
  {
    path: '/:pathMatch(.*)*',
    redirect: '/',
  },
]

const router = createRouter({
  history: createWebHistory(),
  routes,
  scrollBehavior() {
    return { top: 0 }
  },
})

router.beforeEach((to) => {
  const token = localStorage.getItem('token')

  // Public routes: only redirect away when already logged in.
  if (to.meta.public) {
    if (token && (to.name === 'login' || to.name === 'register')) {
      return { name: 'home' }
    }
    return true
  }

  // Protected routes require a token.
  if (!token) {
    return { name: 'login', query: { redirect: to.fullPath } }
  }

  // Admin routes require the admin role.
  if (to.meta.admin) {
    let role = ''
    try {
      role = JSON.parse(localStorage.getItem('user') || '{}').role || ''
    } catch {
      role = ''
    }
    if (role !== 'admin') {
      return { name: 'home' }
    }
  }

  return true
})

router.afterEach((to) => {
  const base = '摄影师网盘'
  document.title = to.meta?.title ? `${to.meta.title} · ${base}` : base
})

export default router
