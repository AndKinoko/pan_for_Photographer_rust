<script setup>
import { ref, watch, computed } from 'vue'
import { createShare } from '../api'
import { useToast } from '../composables/useToast'
import { useModal } from '../composables/useModal'
import AppIcon from './AppIcon.vue'

const props = defineProps({
  visible: { type: Boolean, default: false },
  /**
   * 要装进这一批的条目：`[{ type: 'file'|'folder', id }]`。
   *
   * 一个分享就是一个**批次**，里面能同时装文件和文件夹——所以这里收的是
   * 一个混合列表，而不是 `fileIds`（那表达不了文件夹，也没法混装）。
   */
  items: { type: Array, default: () => [] },
})

const emit = defineEmits(['update:visible', 'created'])
const toast = useToast()

const dialogEl = ref(null)
// 焦点陷阱 / Esc / 焦点归还 / 背景 inert
useModal(() => props.visible, { container: dialogEl, onClose: close })

const expiresHours = ref(0)
const enablePassword = ref(false)
const password = ref('')
const enableMaxDownloads = ref(false)
const maxDownloads = ref(10)
const customCode = ref('')
const submitting = ref(false)

const fileCount = computed(
  () => props.items.filter((i) => i.type === 'file').length
)
const folderCount = computed(
  () => props.items.filter((i) => i.type === 'folder').length
)

/** 「3 个文件夹 · 12 个文件」——把批次里装了什么说清楚 */
const contentSummary = computed(() => {
  const parts = []
  if (folderCount.value) parts.push(`${folderCount.value} 个文件夹`)
  if (fileCount.value) parts.push(`${fileCount.value} 个文件`)
  return parts.join(' · ') || '空'
})

const title = computed(() =>
  props.items.length > 1
    ? `创建分享（${props.items.length} 项）`
    : '创建分享链接'
)

watch(
  () => props.visible,
  (open) => {
    if (open) {
      expiresHours.value = 0
      enablePassword.value = false
      password.value = ''
      enableMaxDownloads.value = false
      maxDownloads.value = 10
      customCode.value = ''
      submitting.value = false
    }
  }
)

function close() {
  emit('update:visible', false)
}

async function submit() {
  if (props.items.length === 0) {
    toast.warning('请先选择要分享的文件或文件夹')
    return
  }
  if (enablePassword.value && !password.value) {
    toast.warning('请填写分享密码')
    return
  }
  submitting.value = true
  try {
    // 多选与单选走**同一条路**：批次模型下一个分享本来就装一批东西，
    // 「只选了一个」只是批次大小为 1，不需要第二条分支。
    // （从前批量分享会给每个文件各建一条链接，那是另一种产品语义，
    //   现在不再提供——要发多条链接就依次建几个批次。）
    const result = await createShare({
      items: props.items.map((i) => ({ type: i.type, id: i.id })),
      expires_hours: expiresHours.value || null,
      password: enablePassword.value ? password.value : null,
      max_downloads: enableMaxDownloads.value ? Number(maxDownloads.value) || null : null,
      custom_code: customCode.value.trim() || null,
    })
    toast.success(`已创建分享（${props.items.length} 项）`)
    emit('created', result)
    close()
  } catch (err) {
    toast.error(err.message || '创建分享失败')
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <Transition name="fade">
    <div v-if="visible" class="overlay" @mousedown.self="close">
      <div ref="dialogEl" class="dialog" role="dialog" aria-modal="true" tabindex="-1">
        <header class="head">
          <h3>{{ title }}</h3>
          <button class="btn-icon btn-ghost" aria-label="关闭" @click="close">
            <AppIcon name="X" size="sm" />
          </button>
        </header>

        <div class="body">
          <!-- 批次里装了什么，先说清楚。用户刚在多选里挑完，这里再确认一次
               比只写个「创建分享链接」有用得多。 -->
          <div class="summary">
            <AppIcon name="Package" size="sm" />
            <span>本次分享包含 <strong>{{ contentSummary }}</strong></span>
          </div>

          <div class="field">
            <label>有效期</label>
            <select v-model.number="expiresHours" class="select">
              <option :value="0">永久有效</option>
              <option :value="1">1 小时</option>
              <option :value="24">1 天</option>
              <option :value="168">7 天</option>
              <option :value="720">30 天</option>
            </select>
          </div>

          <div class="toggle-row">
            <label class="switch">
              <input v-model="enablePassword" type="checkbox" />
              <span>访问密码</span>
            </label>
            <input
              v-if="enablePassword"
              v-model="password"
              class="input"
              type="text"
              placeholder="留空则不设密码"
              autocomplete="new-password"
            />
          </div>

          <!-- 这两项过去只在「单个文件分享」时出现，因为批量分享走的是另一个
               端点、不支持它们。现在只有一个批次端点，限制没有了，统一显示。 -->
          <div class="toggle-row">
            <label class="switch">
              <input v-model="enableMaxDownloads" type="checkbox" />
              <span>最大下载次数</span>
            </label>
            <input
              v-if="enableMaxDownloads"
              v-model.number="maxDownloads"
              class="input"
              type="number"
              min="1"
              placeholder="如 10"
            />
          </div>

          <div class="field">
            <label>自定义分享码（可选）</label>
            <input
              v-model="customCode"
              class="input"
              type="text"
              maxlength="32"
              placeholder="留空则自动生成"
            />
          </div>
        </div>

        <footer class="foot">
          <button class="btn btn-ghost" @click="close">取消</button>
          <button class="btn btn-primary" :disabled="submitting" @click="submit">
            {{ submitting ? '创建中…' : '创建分享' }}
          </button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.summary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px;
  margin-bottom: 14px;
  border-radius: var(--radius-sm);
  background: var(--bg-hover);
  color: var(--text-body);
  font-size: 0.88rem;
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
  z-index: var(--z-modal);
}
.dialog {
  width: min(92vw, 460px);
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  display: flex;
  flex-direction: column;
  max-height: 90vh;
}
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 18px 20px 12px;
}
.head h3 {
  font-size: 1.1rem;
}
.body {
  padding: 8px 20px 16px;
  overflow-y: auto;
}
.foot {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding: 14px 20px 18px;
  border-top: 1px solid var(--border);
}
.toggle-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
  flex-wrap: wrap;
}
.switch {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 0.88rem;
  color: var(--text-heading);
  font-weight: 500;
  white-space: nowrap;
}
.switch input {
  width: 16px;
  height: 16px;
  accent-color: var(--primary);
}
.toggle-row .input {
  flex: 1 1 160px;
}
</style>
