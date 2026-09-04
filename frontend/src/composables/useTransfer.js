import { reactive, computed } from 'vue'
import http, { isImageFile } from '../api'

/**
 * 全局传输状态（模块级单例）。
 * 上传 / 下载队列与抽屉开关在此统一收口，
 * 任何组件 `useTransfer()` 都拿到同一份 reactive state。
 */

const state = reactive({
  drawerOpen: false,
  activeTab: 'upload', // 'upload' | 'download'
  uploads: [], // { id,name,size,status,progress,error,_file,folderId,fileId,abort }
  downloads: [], // { id,name,status,progress,error,abort,url,authed }
})

let seq = 0
const uploadListeners = new Set() // Home 借此感知「某上传完成」→ 插列表 + 缩略图轮询

function nextId() {
  return ++seq
}

/** 上传队列汇总进度（%）—— 只算「uploading / done」两项；pending 不算（还没开始），
 *  这样 10 个文件、第一个跑到 30% 时不会显示 3%（被 9 个 pending 平摊成假进度）。 */
const uploadOverall = computed(() => {
  const active = state.uploads.filter(
    (u) => u.status === 'uploading' || u.status === 'done'
  )
  const total = active.reduce((s, u) => s + u.size, 0)
  const loaded = active.reduce(
    (s, u) => s + (u.size * (u.progress || 0)) / 100,
    0
  )
  return total ? Math.round((loaded / total) * 100) : 0
})

const uploadActiveCount = computed(
  () =>
    state.uploads.filter(
      (u) => u.status === 'pending' || u.status === 'uploading'
    ).length
)

const downloadActiveCount = computed(
  () => state.downloads.filter((d) => d.status === 'downloading').length
)

/* ---------------------------------- 抽屉 ---------------------------------- */
function openDrawer(tab = 'upload') {
  state.activeTab = tab
  state.drawerOpen = true
}
function closeDrawer() {
  state.drawerOpen = false
}
function setTab(tab) {
  state.activeTab = tab
}

/* ---------------------------------- 上传 ---------------------------------- */
function enqueueUpload(fileList, folderId) {
  const files = Array.from(fileList || [])
  if (!files.length) return
  for (const f of files) {
    // 必须先 reactive() 包装，再 push。
    // 直接 push 普通对象后，外部 item 引用仍是普通对象，
    // 后续 item.progress = X 写入的是非响应式对象，UI 不会更新。
    const item = reactive({
      id: nextId(),
      name: f.name,
      size: f.size || 0,
      status: 'pending',
      progress: 0,
      error: '',
      _file: f,
      folderId: folderId ?? null,
      fileId: null,
      abort: null,
    })
    state.uploads.push(item)
  }
  kickUpload()
}

function kickUpload() {
  const next = state.uploads.find((u) => u.status === 'pending' && u._file)
  if (next) runUpload(next)
}

async function runUpload(item) {
  item.status = 'uploading'
  item.progress = 0
  const form = new FormData()
  if (item.folderId != null && item.folderId !== '') {
    form.append('folder_id', String(item.folderId))
  }
  form.append('file', item._file, item.name)
  const controller = new AbortController()
  item.abort = controller
  try {
    // 网络错误/5xx 退避重试（最多 2 次重试）；4xx、超时与主动取消直接落败
    const data = await withRetry(
      () =>
        http.post('/api/files/upload', form, {
          headers: { 'Content-Type': 'multipart/form-data' },
          signal: controller.signal,
          onUploadProgress: (e) => {
            if (e.total) {
              // 封顶 99%，避免最后字节前提前到 100% 与状态切换打架
              item.progress = Math.min(99, Math.round((e.loaded / e.total) * 100))
            }
          },
        }),
      3,
      controller
    )
    const errs = data?.errors || []
    const file = data?.files?.[0]
    if (errs.length && !file) {
      item.status = 'error'
      item.error = errs[0]
    } else {
      item.status = 'done'
      item.progress = 100
      item.fileId = file?.id ?? null
      if (errs.length) item.error = errs[0]
    }
    // 通知完成（文件页插入列表 / 缩略图轮询）
    if (item.status === 'done') {
      const id = item.fileId
      for (const fn of uploadListeners) {
        try {
          fn({
            item,
            fileId: id,
            folderId: item.folderId,
            name: item.name,
            isImage: isImageFile(item.file_type || '', item.name),
          })
        } catch (e) {
          /* 单个监听器异常不影响上传链路 */
        }
      }
    }
  } catch (err) {
    if (err?.code === 'ERR_CANCELED' || controller.signal.aborted) {
      item.status = 'cancelled'
    } else {
      item.status = 'error'
      item.error =
        err?.status === 413 ? '文件超过大小限制' : err?.message || '上传失败'
    }
  } finally {
    item.abort = null
    item._file = null
    kickUpload()
  }
}

async function withRetry(fn, attempts = 3, controller = null) {
  let lastErr
  for (let i = 1; i <= attempts; i++) {
    // 退避前先检查是否已主动取消，避免无意义的等待
    if (controller?.signal?.aborted) {
      const e = new Error('aborted')
      e.code = 'ERR_CANCELED'
      throw e
    }
    try {
      return await fn()
    } catch (err) {
      lastErr = err
      const status = err?.status
      const timedOut = err?.code === 'ECONNABORTED'
      const canceled =
        err?.code === 'ERR_CANCELED' || controller?.signal?.aborted === true
      const retriable = !canceled && (status ? status >= 500 : !timedOut)
      if (i === attempts || !retriable) throw err
      await new Promise((resolve) => setTimeout(resolve, 400 * i))
    }
  }
  throw lastErr
}

function cancelUpload(id) {
  const item = state.uploads.find((u) => u.id === id)
  if (!item) return
  item.abort?.abort()
  if (item.status === 'pending') {
    item.status = 'cancelled'
    kickUpload()
  }
}

// 简化：pending 取消后让 kickUpload 继续；uploading 取消在 catch 里置 cancelled
function removeUpload(id) {
  const i = state.uploads.findIndex((u) => u.id === id)
  if (i !== -1) state.uploads.splice(i, 1)
}

/* ---------------------------------- 下载 ---------------------------------- */

/**
 * 将下载加入全局队列并实时追踪进度。
 * authed=true 走带 token 的下载 URL；authed=false 用于公开分享下载。
 */
function enqueueDownload({ filename, url, authed = true }) {
  if (!url) return
  // 必须先 reactive() 包装，否则 runDownload 内 item.progress = X
  // 写入的是非响应式原始对象，UI 不会实时更新（只在打开抽屉重新挂载时读一次当前值）。
  const item = reactive({
    id: nextId(),
    name: filename || '下载',
    status: 'downloading',
    progress: 0,
    size: 0, // 服务端 Content-Length（字节），0 表示后端没给
    loaded: 0, // 实时已下载字节
    error: '',
    abort: null,
    url,
    authed,
  })
  state.downloads.push(item)
  runDownload(item)
}

async function runDownload(item) {
  const controller = new AbortController()
  item.abort = controller
  try {
    // fetch 走流式：能稳定拿到 Content-Length，不受 axios 120s timeout 限制，
    // 也能精确定位 Failed to fetch / CORS / 状态码等真实错误。
    const headers = {}
    if (item.authed) {
      const token = localStorage.getItem('token')
      if (token) headers['Authorization'] = `Bearer ${token}`
    }
    const res = await fetch(item.url, {
      method: 'GET',
      headers,
      signal: controller.signal,
      credentials: 'same-origin',
    })
    if (!res.ok) {
      let msg = `下载失败 (${res.status})`
      try {
        const data = await res.json()
        if (data && (data.error || data.message)) msg = data.error || data.message
      } catch (_) {
        /* 非 JSON 错误体：保留默认 msg */
      }
      throw new Error(msg)
    }
    const total = Number(res.headers.get('Content-Length')) || 0
    item.size = total // 记录到 item 上，UI 能显示 "已下载 X / Y MB"
    const reader = res.body && res.body.getReader ? res.body.getReader() : null
    const chunks = []
    let loaded = 0
    // 进度节流：避免每来一个 chunk 都改一次 reactive，UI 不需要 60fps
    let lastFlush = 0
    function flushProgress(force = false) {
      const now = performance.now()
      if (!force && now - lastFlush < 80) return
      lastFlush = now
      item.loaded = loaded
      if (total > 0) {
        item.progress = Math.min(99, Math.round((loaded / total) * 100))
      } else {
        // 没有 Content-Length：基于已读字节估算。
        // 用对数曲线让小文件也明显推进、大文件不卡在 99% 太早。
        // 1MB≈30%, 4MB≈55%, 16MB≈80%, 64MB≈92%, 256MB+→95%
        const approx = 95 * (1 - Math.exp(-loaded / (4 * 1024 * 1024)))
        item.progress = Math.max(item.progress || 0, Math.round(approx))
      }
    }
    if (reader) {
      // 流式累积：边下边推进度
      // 注意：HTML5 <a download> 需要完整 blob 才能触发保存，因此 chunks 必须全程保留。
      // 浏览器内部 Blob 构造本身有优化（不复制底层 buffer），已是最优实现。
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        if (value) {
          chunks.push(value)
          loaded += value.byteLength
          flushProgress()
        }
      }
    } else {
      // 极少数浏览器不支持 ReadableStream：降级为一次性读
      const buf = await res.arrayBuffer()
      chunks.push(new Uint8Array(buf))
      loaded = buf.byteLength
      flushProgress(true)
    }
    const blob = new Blob(chunks)
    // 触发浏览器保存（同步），随后用 'saving' 过渡态让用户明确知道正在写入磁盘
    const objectUrl = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = objectUrl
    a.download = item.name
    document.body.appendChild(a)
    a.click()
    a.remove()
    setTimeout(() => URL.revokeObjectURL(objectUrl), 5000)
    item.status = 'saving'
    item.progress = 100
    item.loaded = loaded || total
    // 浏览器保存对话框通常是同步/很快的，留一帧让 UI 渲染 saving 状态再切 done
    setTimeout(() => {
      item.status = 'done'
    }, 50)
  } catch (err) {
    if (err?.name === 'AbortError' || controller.signal.aborted) {
      // 已经在 saving 阶段触发的取消：保留 saving 让用户看到"已下载但被取消"
      if (item.status !== 'saving') item.status = 'cancelled'
    } else {
      item.status = 'error'
      // 典型：'Failed to fetch' / 'NetworkError when attempting to fetch resource'
      // 表示请求没到服务器（原样透出，便于排查 CORS / 离线 / 服务端拒连）
      item.error = err?.message || '下载失败'
    }
  } finally {
    item.abort = null
  }
}

function cancelDownload(id) {
  const item = state.downloads.find((d) => d.id === id)
  if (!item) return
  item.abort?.abort()
}

function clearDone(kind) {
  const arr = kind === 'download' ? state.downloads : state.uploads
  for (let i = arr.length - 1; i >= 0; i--) {
    if (
      arr[i].status === 'done' ||
      arr[i].status === 'error' ||
      arr[i].status === 'cancelled'
    ) {
      arr.splice(i, 1)
    }
  }
}

/** 注册「上传完成」回调，返回注销函数 */
function onUploadComplete(fn) {
  uploadListeners.add(fn)
  return () => uploadListeners.delete(fn)
}

export function useTransfer() {
  return {
    state,
    uploadOverall,
    uploadActiveCount,
    downloadActiveCount,
    openDrawer,
    closeDrawer,
    setTab,
    enqueueUpload,
    cancelUpload,
    removeUpload,
    enqueueDownload,
    cancelDownload,
    clearDone,
    onUploadComplete,
  }
}