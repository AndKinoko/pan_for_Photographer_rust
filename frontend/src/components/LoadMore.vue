<script setup>
/* ===========================================================================
   LoadMore —— 服务端分页的「加载更多」
   ---------------------------------------------------------------------------
   与 useProgressiveList 配套。抽成组件而不是在三个视图里各写一遍，
   是因为这类"每页都要有、容易被漏掉一个"的东西正是本项目漂移的重灾区
   （四套对话框、三份扩展名白名单、两份 .err 字号）。

   显示「已显示 N / 共 M 项」而不是只写「加载更多」：分页之后列表里不再是全部
   内容，用户需要知道「还有多少没看到」，否则会以为目录里只有 100 张。
   `hasMore` 为 false 时自身不渲染。
   =========================================================================== */

import AppIcon from './AppIcon.vue'

defineProps({
  /** 服务端是否还有下一页 */
  hasMore: { type: Boolean, default: false },
  /** 已经加载并渲染出来的条数 */
  loaded: { type: Number, default: 0 },
  /** 服务端报告的该列表总条数 */
  total: { type: Number, default: 0 },
  /** 正在取下一页 */
  loading: { type: Boolean, default: false },
})

const emit = defineEmits(['more'])
</script>

<template>
  <div v-if="hasMore" class="load-more">
    <button class="btn btn-sm" :disabled="loading" @click="emit('more')">
      <AppIcon name="ChevronDown" size="sm" />
      {{
        loading
          ? '加载中…'
          : `加载更多（已显示 ${loaded} / 共 ${total} 项）`
      }}
    </button>
  </div>
</template>

<style scoped>
.load-more {
  display: flex;
  justify-content: center;
  padding: 4px 0 8px;
}
</style>
