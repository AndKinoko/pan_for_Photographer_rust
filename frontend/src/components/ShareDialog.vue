<script setup>
import { ref, watch, computed } from 'vue'
import { createShare, batchShare } from '../api'
import { useToast } from '../composables/useToast'

const props = defineProps({
  visible: { type: Boolean, default: false },
  /** Array of file ids to share. */
  fileIds: { type: Array, default: () => [] },
})

const emit = defineEmits(['update:visible', 'created'])
const toast = useToast()

const expiresHours = ref(0)
const enablePassword = ref(false)
const password = ref('')
const enableMaxDownloads = ref(false)
const maxDownloads = ref(10)
const customCode = ref('')
const submitting = ref(false)

const isBatch = computed(() => props.fileIds.length > 1)
const title = computed(() =>
  isBatch.value
    ? `批量分享 ${props.fileIds.length} 个文件`
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
  if (props.fileIds.length === 0) {
    toast.warning('请先选择要分享的文件')
    return
  }
  if (enablePassword.value && !password.value) {
    toast.warning('请填写分享密码')
    return
  }
  submitting.value = true
  try {
    let result
    if (isBatch.value) {
      result = await batchShare({
        file_ids: props.fileIds,
        expires_hours: expiresHours.value || null,
        password: enablePassword.value ? password.value : null,
      })
      toast.success(`已为 ${props.fileIds.length} 个文件创建分享`)
    } else {
      result = await createShare({
        file_id: props.fileIds[0],
        expires_hours: expiresHours.value || null,
        password: enablePassword.value ? password.value : null,
        max_downloads: enableMaxDownloads.value ? Number(maxDownloads.value) || null : null,
        custom_code: customCode.value.trim() || null,
      })
      toast.success('分享链接已创建')
    }
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
      <div class="dialog" role="dialog" aria-modal="true">
        <header class="head">
          <h3>{{ title }}</h3>
          <button class="btn-icon btn-ghost" aria-label="关闭" @click="close">
            ✕
          </button>
        </header>

        <div class="body">
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

          <template v-if="!isBatch">
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
          </template>
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
.overlay {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
  backdrop-filter: blur(2px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  z-index: 8500;
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
