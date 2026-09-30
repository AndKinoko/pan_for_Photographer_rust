<script setup>
/* ===========================================================================
   PasswordInput —— 带显示/隐藏切换的密码框
   ---------------------------------------------------------------------------
   原先全站 4 个 `type="password"` 输入框（Auth 登录、Admin 新建用户初始密码、
   Admin 重置密码、PublicShare 分享密码）都没有回显开关。
   管理员给客户设初始密码、或客户输入分享密码时只能盲打重试。

   为什么抽成组件而不是抄 4 遍：这四处将来都会各自演化（本项目已经有
   「四套对话框各写各的、其中一套 z-index 低于侧边栏」的先例），
   而且切换按钮的 aria 状态、tab 顺序、与 label 的关联都属于容易漏的细节。

   `inheritAttrs: false` + `v-bind="$attrs"` 是有意的：调用方传进来的
   id / autocomplete / placeholder / autofocus / aria-describedby / aria-invalid
   必须落到真正的 <input> 上，而不是外面这层 div——否则 label 的 for、
   错误提示的 aria-describedby 全部失效。
   =========================================================================== */

import { ref } from 'vue'
import AppIcon from './AppIcon.vue'

defineOptions({ inheritAttrs: false })

const props = defineProps({
  modelValue: { type: String, default: '' },
})

const emit = defineEmits(['update:modelValue'])

const show = ref(false)
const inputEl = ref(null)

// 父组件需要把焦点移到出错字段上，所以要暴露 focus/select
defineExpose({
  focus: () => inputEl.value?.focus(),
  select: () => inputEl.value?.select(),
})
</script>

<template>
  <div class="pwd-wrap">
    <input
      ref="inputEl"
      v-bind="$attrs"
      class="input"
      :value="modelValue"
      :type="show ? 'text' : 'password'"
      @input="emit('update:modelValue', $event.target.value)"
    />
    <button
      type="button"
      class="pwd-toggle"
      :aria-label="show ? '隐藏密码' : '显示密码'"
      :aria-pressed="show ? 'true' : 'false'"
      :title="show ? '隐藏密码' : '显示密码'"
      @click="show = !show"
    >
      <AppIcon :name="show ? 'EyeOff' : 'Eye'" size="sm" />
    </button>
  </div>
</template>

<style scoped>
.pwd-wrap {
  position: relative;
  display: flex;
  align-items: center;
}
/* 给切换按钮留出位置，否则长密码会顶到按钮底下 */
.pwd-wrap .input {
  padding-right: 44px;
}
.pwd-toggle {
  position: absolute;
  right: 4px;
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  transition: background-color 0.15s ease, color 0.15s ease;
}
.pwd-toggle:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
}
/* 触屏没有 hover，按钮又只有 36px，用伪元素把可点区域扩到 44×44 */
@media (hover: none) {
  .pwd-toggle::after {
    content: '';
    position: absolute;
    inset: -4px;
  }
}
</style>
