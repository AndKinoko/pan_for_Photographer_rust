<script setup>
/* ===========================================================================
   AppIcon —— 全站唯一的图标出口
   ---------------------------------------------------------------------------
   用法：
     <AppIcon name="Trash2" />                      装饰性，默认从无障碍树隐藏
     <AppIcon name="Upload" size="sm" />            16px
     <AppIcon name="CircleAlert" :decorative="false" />  有语义，交给外部补名称

   尺寸用 sm/md/lg 三档而不是任意数字，是为了让图标有"尺度感"：
   原来 emoji 混用 2.6rem / 3rem / 1.15rem / 1.1rem 四种字号，且随字体缩放漂移。

   `decorative` 默认 true 是刻意的默认值：绝大多数图标旁边就写着它的含义
   （"🗑️ 删除"、"🔗 分享"），重复朗读只会让屏幕阅读器更啰嗦。
   纯图标按钮请保持 decorative 并在按钮上写 aria-label。
   =========================================================================== */

import { computed } from 'vue'
import { ICONS } from '../icons'

const props = defineProps({
  /** 图标名，取自 src/icons.js 的 ICONS 注册表 */
  name: { type: String, required: true },
  /** 视觉尺寸档位 */
  size: { type: String, default: 'md' },
  /** 是否纯装饰（默认是）。false 时图标会暴露给无障碍树 */
  decorative: { type: Boolean, default: true },
})

// 16 与正文 0.78rem 字号齐平、20 与 0.92rem 齐平、24 用于标题与行内按钮，
// 48 用于全屏预览的空态（原 emoji 是 4rem，纯图标排到 64px 会显得笨重）
const SIZES = { sm: 16, md: 20, lg: 24, xl: 48 }

// 未注册的名字回退成通用文件图标而不是渲染空白——静默失败会让人以为图标丢了
const component = computed(() => ICONS[props.name] || ICONS.File)
const px = computed(() => SIZES[props.size] || SIZES.md)
</script>

<template>
  <component
    :is="component"
    class="app-icon"
    :size="px"
    :stroke-width="2"
    :aria-hidden="decorative ? 'true' : undefined"
    :focusable="false"
  />
</template>

<style scoped>
.app-icon {
  /* 不参与 flex 收缩，否则窄按钮里的图标会被压扁成椭圆 */
  flex: 0 0 auto;
  /* 与相邻文字做基线对齐。默认 inline 会让图标沉在基线上，
     视觉重心比文字低约 2px；-0.125em 是图标与文字混排的常用补偿量。 */
  vertical-align: -0.125em;
}
</style>
