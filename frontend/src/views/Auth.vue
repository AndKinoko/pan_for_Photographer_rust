<script setup>
import { ref, computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { login, register } from '../api'
import { useToast } from '../composables/useToast'
import { useTheme } from '../composables/useTheme'

const route = useRoute()
const router = useRouter()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()

const tab = ref(route.name === 'register' ? 'register' : 'login')
const username = ref('')
const password = ref('')
const loading = ref(false)
const errors = ref({ username: '', password: '' })

watch(tab, () => {
  errors.value = { username: '', password: '' }
})

const isRegister = computed(() => tab.value === 'register')

function validate() {
  const e = { username: '', password: '' }
  if (!username.value.trim()) e.username = '请输入用户名'
  else if (username.value.trim().length < 2) e.username = '用户名至少 2 个字符'
  if (!password.value) e.password = '请输入密码'
  else if (password.value.length < 6) e.password = '密码至少 6 位'
  errors.value = e
  return !e.username && !e.password
}

async function submit() {
  if (!validate()) return
  loading.value = true
  try {
    const data =
      tab.value === 'login'
        ? await login(username.value.trim(), password.value)
        : await register(username.value.trim(), password.value)
    localStorage.setItem('token', data.token)
    localStorage.setItem('user', JSON.stringify(data.user))
    toast.success(isRegister.value ? '注册成功' : '登录成功')
    const redirect = route.query.redirect
    router.push(typeof redirect === 'string' ? redirect : '/')
  } catch (err) {
    toast.error(err.message || (isRegister.value ? '注册失败' : '登录失败'))
  } finally {
    loading.value = false
  }
}

function onKeydown(e) {
  if (e.key === 'Enter') submit()
}
</script>

<template>
  <div class="auth">
    <button class="theme-fab" :aria-label="theme === 'dark' ? '浅色' : '深色'" @click="toggleTheme">
      {{ theme === 'dark' ? '☀️' : '🌙' }}
    </button>

    <div class="auth-card card">
      <div class="brand">
        <span class="logo">📷</span>
        <div>
          <h1>摄影师网盘</h1>
          <p class="muted">Pan for Photographer</p>
        </div>
      </div>

      <div class="tabs" role="tablist">
        <button
          role="tab"
          :class="{ active: tab === 'login' }"
          @click="tab = 'login'"
        >
          登录
        </button>
        <button
          role="tab"
          :class="{ active: tab === 'register' }"
          @click="tab = 'register'"
        >
          注册
        </button>
      </div>

      <form class="form" @submit.prevent="submit" @keydown="onKeydown">
        <div class="field">
          <label for="username">用户名</label>
          <input
            id="username"
            v-model.trim="username"
            class="input"
            type="text"
            autocomplete="username"
            placeholder="请输入用户名"
          />
          <span v-if="errors.username" class="err">{{ errors.username }}</span>
        </div>

        <div class="field">
          <label for="password">密码</label>
          <input
            id="password"
            v-model="password"
            class="input"
            type="password"
            :autocomplete="isRegister ? 'new-password' : 'current-password'"
            placeholder="至少 6 位"
          />
          <span v-if="errors.password" class="err">{{ errors.password }}</span>
        </div>

        <button class="btn btn-primary submit" type="submit" :disabled="loading">
          {{ loading ? '请稍候…' : isRegister ? '注册' : '登录' }}
        </button>
      </form>

      <p class="switch muted">
        {{ isRegister ? '已有账号？' : '还没有账号？' }}
        <a href="#" @click.prevent="tab = isRegister ? 'login' : 'register'">
          {{ isRegister ? '去登录' : '去注册' }}
        </a>
      </p>
    </div>
  </div>
</template>

<style scoped>
.auth {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px 16px;
  background: radial-gradient(
      circle at 20% 0%,
      var(--primary-soft),
      transparent 55%
    ),
    var(--bg);
}
.theme-fab {
  position: fixed;
  top: 16px;
  right: 16px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  box-shadow: var(--shadow);
  font-size: 1.1rem;
}
.auth-card {
  width: min(92vw, 400px);
  padding: 28px 26px 22px;
  box-shadow: var(--shadow-lg);
}
.brand {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 22px;
}
.logo {
  font-size: 2.2rem;
}
.brand h1 {
  font-size: 1.3rem;
}
.tabs {
  display: flex;
  background: var(--bg-hover);
  border-radius: var(--radius-sm);
  padding: 4px;
  margin-bottom: 20px;
}
.tabs button {
  flex: 1;
  padding: 10px;
  border-radius: 6px;
  font-weight: 600;
  color: var(--text-muted);
  transition: all 0.18s ease;
}
.tabs button.active {
  background: var(--bg-elevated);
  color: var(--primary);
  box-shadow: var(--shadow-sm);
}
.form {
  display: flex;
  flex-direction: column;
}
.submit {
  width: 100%;
  margin-top: 6px;
}
.err {
  color: var(--danger);
  font-size: 0.78rem;
}
.switch {
  text-align: center;
  margin-top: 18px;
  font-size: 0.88rem;
}
</style>
