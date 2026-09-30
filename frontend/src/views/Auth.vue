<script setup>
import { ref, computed, watch, nextTick, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { login, register, verifyInvite } from '../api'
import { useToast } from '../composables/useToast'
import { useTheme } from '../composables/useTheme'
import AppIcon from '../components/AppIcon.vue'
import PasswordInput from '../components/PasswordInput.vue'

const route = useRoute()
const router = useRouter()
const toast = useToast()
const { theme, toggle: toggleTheme } = useTheme()

const tab = ref(route.name === 'register' ? 'register' : 'login')
const username = ref('')
const password = ref('')
const loading = ref(false)
const errors = ref({ username: '', password: '', invite: '' })

/* 邀请码。注册制下它是第一道门：先验码、再让用户填用户名密码。
   不这么做的话，客户要填完整页才被告知「码无效」。 */
const inviteCode = ref('')
const inviteInput = ref(null)
const inviteOk = ref(false)

// 用于把焦点移到第一个出错的字段（见 submit）
const usernameEl = ref(null)
const passwordEl = ref(null)

/* 登录限流倒计时。
   后端被限流时返回 429 与「还需等待 N 秒」，那个 N 已经是从最后一次失败
   起算的**剩余**秒数。但它只在请求时才有意义——不点重试就没有新响应，
   数字会一直停在原地。所以前端自己按秒递减，让用户看到它真的在走。
   到 0 时自动清空，错误提示消失，登录按钮恢复可用。 */
const cooldown = ref(0)
let cooldownTimer = null

function startCooldown(seconds) {
  stopCooldown()
  cooldown.value = Math.max(1, Number(seconds) || 0)
  cooldownTimer = setInterval(() => {
    cooldown.value -= 1
    if (cooldown.value <= 0) stopCooldown()
  }, 1000)
}

function stopCooldown() {
  if (cooldownTimer) {
    clearInterval(cooldownTimer)
    cooldownTimer = null
  }
  cooldown.value = 0
}

onUnmounted(stopCooldown)

watch(tab, (t) => {
  errors.value = { username: '', password: '', invite: '' }
  inviteOk.value = false
  inviteCode.value = ''
})

const isRegister = computed(() => tab.value === 'register')

/* 错误摘要播报。
   字段级错误用 aria-describedby 关联到各自的输入框（重新聚焦时还能再读一次），
   这里只负责「错误出现的瞬间」播报一次，避免同一句话被读两遍。 */
const errorSummary = computed(() => {
  const n = [errors.value.username, errors.value.password, errors.value.invite].filter(Boolean).length
  return n ? `表单有 ${n} 处需要修改` : ''
})

/* 验码。成功后才允许继续填表——用 disabled 挡提交按钮，
   比提交后再报错少一次往返。码本身不消费，可以放心重复验。 */
async function checkInvite() {
  if (inviteOk.value) return true
  const code = inviteCode.value.trim()
  if (!code) {
    errors.value = { ...errors.value, invite: '请输入邀请码' }
    inviteInput.value?.focus()
    return false
  }
  loading.value = true
  try {
    await verifyInvite(code)
    inviteOk.value = true
    errors.value = { ...errors.value, invite: '' }
    return true
  } catch (err) {
    inviteOk.value = false
    errors.value = { ...errors.value, invite: err.message || '邀请码无效' }
    return false
  } finally {
    loading.value = false
  }
}

function validate() {
  const e = { username: '', password: '', invite: '' }
  if (!username.value.trim()) e.username = '请输入用户名'
  else if (username.value.trim().length < 2) e.username = '用户名至少 2 个字符'
  if (!password.value) e.password = '请输入密码'
  else if (password.value.length < 6) e.password = '密码至少 6 位'
  errors.value = e
  return !e.username && !e.password
}

async function submit() {
  // 注册制：先把码验掉。已验过（inviteOk）时这一步是空操作。
  if (tab.value === 'register' && !inviteOk.value) {
    if (!(await checkInvite())) return
  }
  if (!validate()) {
    await nextTick()
    // 把焦点移到第一个出错的字段：键盘用户不用自己去找哪里填错了，
    // 且聚焦会让该字段的 aria-describedby 错误提示被读出来
    if (errors.value.username) usernameEl.value?.focus()
    else if (errors.value.password) passwordEl.value?.focus()
    return
  }
  loading.value = true
  try {
    const data =
      tab.value === 'login'
        ? await login(username.value.trim(), password.value)
        : await register(username.value.trim(), password.value, inviteCode.value.trim())
    localStorage.setItem('token', data.token)
    localStorage.setItem('user', JSON.stringify(data.user))
    toast.success(isRegister.value ? '注册成功' : '登录成功')
    const redirect = route.query.redirect
    router.push(typeof redirect === 'string' ? redirect : '/')
  } catch (err) {
    // 被限流时不再只弹一条静态 toast：那条提示里的秒数不会自己走，
    // 用户看到「请在 1 秒后重试」永远不变，还会以为程序卡住了。
    // 改成页面内可见的倒计时，归零后自动恢复可提交。
    if (err.status === 429 && err.retryAfter) {
      startCooldown(err.retryAfter)
    } else {
      toast.error(err.message || (isRegister.value ? '注册失败' : '登录失败'))
    }
  } finally {
    loading.value = false
  }
}

function onKeydown(e) {
  if (e.key !== 'Enter') return
  // 焦点在按钮上时交给按钮自己处理。加了密码显示切换之后，
  // 这一条是必需的——否则在「👁 显示密码」上按 Enter 会同时切换可见性并提交表单。
  if (e.target instanceof HTMLButtonElement) return
  submit()
}
</script>

<template>
  <div class="auth">
    <button class="theme-fab" :aria-label="theme === 'dark' ? '浅色' : '深色'" @click="toggleTheme">
      <AppIcon :name="theme === 'dark' ? 'Sun' : 'Moon'" size="sm" />
    </button>

    <div class="auth-card card">
      <div class="brand">
        <AppIcon class="logo" name="Camera" size="xl" />
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
        <p class="sr-only" role="alert">{{ errorSummary }}</p>

        <!-- 登录限流倒计时。放在表单里而不是 toast 里：toast 会自动消失，
             而这里必须一直可见到倒计时结束。role="status" 让屏幕阅读器
             礼貌播报，不会打断用户。aria-live="polite" 也不会每秒都念一遍，
             只有数字变化时才读。 -->
        <p v-if="cooldown > 0" class="cooldown" role="status">
          <AppIcon name="Clock" size="sm" />
          登录失败次数过多，请在 <strong>{{ cooldown }}</strong> 秒后重试
        </p>

        <!-- 邀请码。注册制的入口，所以放在最前面：先验码，再填账号。
             v-model 不加 .trim 是为了保留用户正在输入的空格手感，
             提交前统一 trim（后端也会再归一化一次）。 -->
        <div v-if="isRegister" class="field">
          <label for="invite">邀请码</label>
          <input
            id="invite"
            ref="inviteInput"
            v-model="inviteCode"
            class="input invite-input"
            :class="{ ok: inviteOk }"
            type="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="向摄影师索取"
            :disabled="loading || inviteOk"
            :aria-invalid="errors.invite ? 'true' : undefined"
            :aria-describedby="errors.invite ? 'invite-error' : 'invite-hint'"
          />
          <span v-if="errors.invite" id="invite-error" class="err">
            {{ errors.invite }}
          </span>
          <span v-else-if="inviteOk" id="invite-hint" class="hint ok-hint">
            邀请码有效，请继续填写用户名和密码
          </span>
          <span v-else id="invite-hint" class="hint">
            本网盘为邀请制，账号由摄影师开通
          </span>
        </div>

        <div class="field">
          <label for="username">用户名</label>
          <input
            id="username"
            ref="usernameEl"
            v-model.trim="username"
            class="input"
            type="text"
            autocomplete="username"
            placeholder="请输入用户名"
            :aria-invalid="errors.username ? 'true' : undefined"
            :aria-describedby="errors.username ? 'username-error' : undefined"
          />
          <span v-if="errors.username" id="username-error" class="err">
            {{ errors.username }}
          </span>
        </div>

        <div class="field">
          <label for="password">密码</label>
          <PasswordInput
            id="password"
            ref="passwordEl"
            v-model="password"
            :autocomplete="isRegister ? 'new-password' : 'current-password'"
            placeholder="至少 6 位"
            :aria-invalid="errors.password ? 'true' : undefined"
            :aria-describedby="errors.password ? 'password-error' : undefined"
          />
          <span v-if="errors.password" id="password-error" class="err">
            {{ errors.password }}
          </span>
        </div>

        <button
          class="btn btn-primary submit"
          type="submit"
          :disabled="loading || cooldown > 0"
        >
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
  color: var(--primary);
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
/* 邀请码：等宽字体，因为它是照着抄的，字形宽度一致才看得出漏没漏字符 */
.invite-input {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}
.invite-input.ok {
  border-color: var(--success, #22c55e);
}
.hint {
  font-size: 0.8rem;
  color: var(--text-muted);
  line-height: 1.5;
}
.ok-hint {
  color: var(--success, #22c55e);
}
.submit {
  width: 100%;
  margin-top: 6px;
}
/* .err 已统一到 style.css（原先此处 0.78rem、Admin 0.85rem，已漂移） */
.switch {
  text-align: center;
  margin-top: 18px;
  font-size: 0.88rem;
}
</style>
