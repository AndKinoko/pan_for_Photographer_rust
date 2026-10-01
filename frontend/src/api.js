import axios from 'axios'
import router from './router'

/* ===========================================================================
   Axios instance + auth interceptor + response unwrapping.
   Backend contract: every JSON response is { success, data, error }.

   Note: `router` is statically imported here. This is safe because
   `router.js` only lazily imports the views, so it never triggers
   evaluation of `api.js` during module init (no circular init problem).
   =========================================================================== */

const TOKEN_KEY = 'token'

/* ===========================================================================
   Media ticket
   ---------------------------------------------------------------------------
   Thumbnails, previews and download links are loaded by <img> / <a download>,
   which cannot send an Authorization header. Those URLs therefore have to
   carry a credential in the query string.

   They used to carry the **JWT itself** — a 7-day, full-permission bearer
   token — into every thumbnail URL. Query strings are recorded verbatim by
   Cloudflare's edge logs, the browser history and DevTools, so one thumbnail
   load copied the account key into three logs.

   The backend now issues a narrow, short-lived `media_ticket` (2h, media and
   download endpoints only). It answers "who are you"; file ownership is still
   enforced by `owner_id` in SQL on the server side, so this widens nothing.

   The ticket is returned by /auth/login, /auth/register and /auth/me, and can
   be refreshed on demand via /auth/media-ticket. The request interceptor
   below refreshes it *before* any other API call goes out, which guarantees a
   fresh ticket is in hand by the time a list response produces thumbnail
   URLs — no flash of broken images after the ticket expires.
   =========================================================================== */

const MEDIA_TICKET_TTL_MS = 2 * 60 * 60 * 1000
/** Refresh this long before the real expiry, so a request never races it. */
const MEDIA_TICKET_SKEW_MS = 60 * 1000

let mediaTicket = null
let mediaTicketExpiresAt = 0
let mediaTicketPending = null

export function setMediaTicket(ticket, ttlSeconds) {
  if (!ticket) {
    clearMediaTicket()
    return
  }
  mediaTicket = ticket
  const ttl = ttlSeconds ? Number(ttlSeconds) * 1000 : MEDIA_TICKET_TTL_MS
  mediaTicketExpiresAt = Date.now() + Math.max(0, ttl - MEDIA_TICKET_SKEW_MS)
}

export function clearMediaTicket() {
  mediaTicket = null
  mediaTicketExpiresAt = 0
  mediaTicketPending = null
}

const mediaTicketFresh = () => !!mediaTicket && Date.now() < mediaTicketExpiresAt

/**
 * Ensure a usable media ticket exists, fetching one if needed.
 * Concurrent callers share a single in-flight request.
 */
function ensureMediaTicket() {
  if (mediaTicketFresh()) return Promise.resolve(mediaTicket)
  if (!localStorage.getItem(TOKEN_KEY)) return Promise.resolve(null)
  if (!mediaTicketPending) {
    mediaTicketPending = instance
      .post('/api/auth/media-ticket')
      .then((data) => {
        setMediaTicket(data?.ticket, data?.expires_in)
        return mediaTicket
      })
      .catch(() => null)
      .finally(() => {
        mediaTicketPending = null
      })
  }
  return mediaTicketPending
}

const instance = axios.create({
  baseURL: '',
  timeout: 120000,
})

// Attach JWT to every request when available.
instance.interceptors.request.use(async (config) => {
  const token = localStorage.getItem(TOKEN_KEY)
  if (token) {
    config.headers = config.headers || {}
    config.headers.Authorization = `Bearer ${token}`
  }
  // Every authenticated API call is a chance to refresh the media ticket.
  // Doing it here (rather than lazily at render time) is what keeps
  // `authUrl()` synchronous and free of broken-image races.
  const url = config.url || ''
  if (token && url.startsWith('/api/') && !url.startsWith('/api/auth/')) {
    await ensureMediaTicket()
  }
  return config
})

// Unwrap the unified envelope and centralise error handling.
instance.interceptors.response.use(
  (response) => {
    const body = response.data
    if (body && typeof body === 'object' && 'success' in body) {
      if (body.success) {
        // Login / register / me all hand back a fresh ticket.
        if (body.data && body.data.media_ticket) {
          setMediaTicket(body.data.media_ticket, body.data.media_ticket_ttl)
        }
        return body.data
      }
      return Promise.reject(new Error(body.error || '请求失败'))
    }
    return body
  },
  (error) => {
    if (error.response) {
      const { status, data } = error.response
      const msg =
        (data && (data.error || data.message)) ||
        error.message ||
        `请求失败 (${status})`

      if (status === 401) {
        localStorage.removeItem(TOKEN_KEY)
        localStorage.removeItem('user')
        clearMediaTicket()
        const current = router.currentRoute
          ? router.currentRoute.value
          : null
        const path = current ? current.path : ''
        // Only redirect away from protected pages; public share stays.
        if (!path.startsWith('/share/') && path !== '/login') {
          router.push('/login').catch(() => {})
        }
      }
      const wrapped = new Error(msg)
      wrapped.status = status
        // 429 时后端会算好「还需等待 N 秒」并写进提示语。
        // 这里再解析出数字字段，让调用方能做**倒计时**——
        // 只把那句话弹出来的话，数字会一直停在原地不动。
        if (status === 429) {
          const m = /(\d+)\s*秒/.exec(msg)
          if (m) wrapped.retryAfter = Number(m[1])
        }
      return Promise.reject(wrapped)
    }
    return Promise.reject(error)
  }
)

export default instance

/* ===========================================================================
   URL helpers
   =========================================================================== */

/**
 * Append the current media ticket as a query param. Required for resources
 * loaded by <img>/<a download> which cannot send an Authorization header
 * (backend accepts ?ticket= on the download/media endpoints).
 *
 * Uses the short-lived media ticket, never the JWT.
 */
export function authUrl(url) {
  if (!url) return url
  if (!mediaTicket) return url
  const sep = url.includes('?') ? '&' : '?'
  return `${url}${sep}ticket=${encodeURIComponent(mediaTicket)}`
}

/* ===========================================================================
   Auth API
   =========================================================================== */

export const register = (username, password, inviteCode) =>
  instance.post('/api/auth/register', { username, password, invite_code: inviteCode })

/** 校验邀请码是否可用（不消费）。注册页在填用户名之前先验一次。 */
export const verifyInvite = (inviteCode) =>
  instance.post('/api/auth/invite/verify', { invite_code: inviteCode })

export const login = (username, password) =>
  instance.post('/api/auth/login', { username, password })

export const getMe = () => instance.get('/api/auth/me')

/* ===========================================================================
   Files API
   =========================================================================== */

/**
 * 把列表接口的分页响应归一化成同一种形状，供 `useProgressiveList` 消费。
 *
 * 三个接口的原始字段名有差异（`/api/files` 用 total，`/api/trash` 与
 * `/api/search` 用 total_files），归一化只在这一处做，视图层就不必各自记住
 * 「哪个接口叫什么」。这也是前端唯一需要了解分页字段的地方。
 */
function normalizePage(data, { withFolders = false } = {}) {
  const d = data || {}
  const page = {
    files: d.files || [],
    total: d.total ?? d.total_files ?? 0,
    hasMore: !!d.has_more,
    nextCursor: d.next_cursor || null,
  }
  if (withFolders) {
    page.folders = d.folders || []
    page.totalFolders = d.total_folders ?? page.folders.length
  }
  if (d.file_types) page.fileTypes = d.file_types
  return page
}

/** 分页参数：只在有值时才带上，避免出现 `limit=undefined` 这种请求 */
function pageParams({ limit, cursor } = {}) {
  const p = {}
  if (limit) p.limit = limit
  if (cursor) p.cursor = cursor
  return p
}

/**
 * GET /api/files（游标分页）
 * @param {number|null} folderId
 * @param {{limit?: number, cursor?: string|null}} [page]
 */
export const listFiles = (folderId, page) =>
  instance
    .get('/api/files', {
      params: {
        ...(folderId != null ? { folder_id: folderId } : {}),
        ...pageParams(page),
      },
    })
    .then((d) => normalizePage(d))

export const renameFile = (id, name) =>
  instance.put(`/api/files/${id}/rename`, { name })

export const deleteFile = (id) => instance.delete(`/api/files/${id}`)

export const restoreFile = (id) => instance.post(`/api/files/${id}/restore`)

export const permanentDeleteFile = (id) =>
  instance.delete(`/api/files/${id}/permanent`)

/* ===========================================================================
   Folders API
   =========================================================================== */

export const listFolders = (parentId) =>
  instance.get('/api/folders', {
    params: parentId != null ? { parent_id: parentId } : {},
  })

export const createFolder = (name, parentId) =>
  instance.post('/api/folders', { name, parent_id: parentId })

export const renameFolder = (id, name) =>
  instance.put(`/api/folders/${id}/rename`, { name })

export const deleteFolder = (id) => instance.delete(`/api/folders/${id}`)

export const restoreFolder = (id) => instance.post(`/api/folders/${id}/restore`)

export const permanentDeleteFolder = (id) =>
  instance.delete(`/api/folders/${id}/permanent`)

/* ===========================================================================
   Trash API
   =========================================================================== */

export const listTrash = (page) =>
  instance
    .get('/api/trash', { params: pageParams(page) })
    .then((d) => normalizePage(d, { withFolders: true }))

export const emptyTrash = () => instance.delete('/api/trash')

/* ===========================================================================
   Shares API (authenticated)
   =========================================================================== */

export const listShares = () => instance.get('/api/shares')

export const createShare = (payload) => instance.post('/api/shares', payload)

export const getShare = (id) => instance.get(`/api/shares/${id}`)

export const deleteShare = (id) => instance.delete(`/api/shares/${id}`)

/* ===========================================================================
   Public share API (no auth)
   =========================================================================== */

/**
 * Public share detail.
 *
 * For password-protected shares the backend only returns preview/thumb URLs
 * when the request carries a valid access ticket, so `ticket` must be passed
 * once the visitor has verified the password (PublicShare.vue re-loads after
 * verifying). Without it the response is still valid — just without media URLs.
 */
export const getPublicShare = (id, ticket) =>
  instance.get(`/api/public/shares/${id}`, {
    params: ticket ? { ticket } : {},
  })

export const verifySharePassword = (id, password) =>
  instance.post(`/api/public/shares/${id}/verify`, { password })

/**
 * 浏览批次内容。
 *
 * 不传 folderId → 列出批次顶层（里面直接包含的文件与文件夹）。
 * 传 folderId → 列出该文件夹的直接子级（后端会先校验它落在批次范围内）。
 *
 * 批次模型下一个分享指向的是一批条目，不再是单个文件，所以「这个分享里有
 * 什么」必须单独问一次这个接口。
 */
export const getPublicShareItems = (id, { folderId = null, cursor = null, limit = null, ticket = null } = {}) => {
  const params = {}
  if (folderId != null) params.folder_id = folderId
  if (cursor) params.cursor = cursor
  if (limit) params.limit = limit
  if (ticket) params.ticket = ticket
  return instance
    .get(`/api/public/shares/${id}/items`, { params })
    .then((d) => ({
      folderId: d.folder_id ?? null,
      breadcrumbs: d.breadcrumbs || [],
      items: d.items || [],
      total: d.total ?? 0,
      hasMore: !!d.has_more,
      nextCursor: d.next_cursor || null,
    }))
}

/**
 * 给后端下发的公开资源地址补上访问凭证。
 *
 * 批次里的每个条目，后端都已经生成了带 `file_id` 的完整地址
 * （`preview_url` / `thumb_url` / `download_url`），前端只负责追加 ticket——
 * 地址拼装规则只有一处（后端 side），这里不重复实现一遍，
 * 免得两边对「哪个参数叫什么」产生分歧。
 */
export const withShareTicket = (url, ticket) => {
  if (!url || !ticket) return url
  const sep = url.includes('?') ? '&' : '?'
  return `${url}${sep}ticket=${encodeURIComponent(ticket)}`
}

/* ===========================================================================
   Search API
   =========================================================================== */

/** GET /api/search（游标分页）。`params` 里直接带 `limit` / `cursor`。 */
export const searchFiles = (params) =>
  instance
    .get('/api/search', { params })
    .then((d) => normalizePage(d, { withFolders: true }))

/* ===========================================================================
   Batch API
   =========================================================================== */

export const batchMove = (payload) => instance.post('/api/batch/move', payload)

export const batchCopy = (payload) => instance.post('/api/batch/copy', payload)

export const batchDelete = (payload) =>
  instance.post('/api/batch/delete', payload)

/* 没有 batchShare：批次模型下「分享选中的这批东西」就是 createShare，
   后端已删掉 /api/batch/share（两个端点会完全同义）。 */

export const batchUnshare = (payload) =>
  instance.post('/api/batch/unshare', payload)

/* ===========================================================================
   Admin API
   =========================================================================== */

export const adminListUsers = () => instance.get('/api/admin/users')

export const adminUpdateUserRole = (id, role) =>
  instance.put(`/api/admin/users/${id}/role`, { role })

export const adminDeleteUser = (id) =>
  instance.delete(`/api/admin/users/${id}`)

export const adminGetStats = () => instance.get('/api/admin/stats')

/* ── 注册邀请码 ────────────────────────────────────────────────
   注册制下，摄影师靠发码决定「谁能进这个网盘」。管理端需要能
   批量生成（一次发给几个客户）、查看哪些已被谁用掉、以及删除。 */
export const adminListInviteCodes = () => instance.get('/api/admin/invite-codes')

export const adminCreateInviteCodes = (payload) =>
  instance.post('/api/admin/invite-codes', payload)

export const adminDeleteInviteCode = (id) =>
  instance.delete(`/api/admin/invite-codes/${id}`)

/* 孤儿文件清理。后台每 24 小时自动跑一次，这里是手动入口——
   运维发现磁盘占用异常时可以立刻处理。 */
export const adminRunGcCleanup = () => instance.post('/api/admin/gc/cleanup')

export const adminGetGcStatus = () => instance.get('/api/admin/gc/status')

/* ----- 管理端：用户增改 / 代管文件夹 / 代为上传 ----- */

/** 新建用户；payload: { username, password, role?, expires_at? } */
export const adminCreateUser = (payload) =>
  instance.post('/api/admin/users', payload)

/** 更新用户；payload: { username?, password?, role?, expires_at?|null }。
 *  expires_at 传 null 表示清除有效期；不传则保持不变。 */
export const adminUpdateUser = (id, payload) =>
  instance.put(`/api/admin/users/${id}`, payload)

/** 列出某普通用户的文件夹（管理端代查） */
export const adminListUserFolders = (id, parentId) =>
  instance.get(`/api/admin/users/${id}/folders`, {
    params: parentId != null ? { parent_id: parentId } : {},
  })

/** 为某普通用户新建文件夹 */
export const adminCreateUserFolder = (id, name, parentId) =>
  instance.post(`/api/admin/users/${id}/folders`, {
    name,
    parent_id: parentId,
  })

/** 以管理员身份为指定用户上传文件；返回 axios 原始响应以暴露进度。 */
export const adminUploadToUser = (userId, folderId, file, onUploadProgress) =>
  instance.post(
    '/api/files/upload',
    (() => {
      const form = new FormData()
      if (folderId != null && folderId !== '') form.append('folder_id', String(folderId))
      form.append('user_id', String(userId))
      form.append('file', file, file.name)
      return form
    })(),
    {
      headers: { 'Content-Type': 'multipart/form-data' },
      onUploadProgress,
    }
  )

/* ===========================================================================
   Health API
   =========================================================================== */

export const checkHealth = () => instance.get('/api/health')

/* ===========================================================================
   Formatting utilities
   =========================================================================== */

const IMAGE_EXT = ['jpg', 'jpeg', 'png', 'gif', 'bmp', 'webp', 'tiff', 'tif', 'svg', 'heic', 'avif']
const VIDEO_EXT = ['mp4', 'mov', 'avi', 'mkv', 'webm', 'flv', 'wmv', 'm4v']
const AUDIO_EXT = ['mp3', 'wav', 'flac', 'ogg', 'aac', 'm4a']
const RAW_EXT = ['nef', 'cr2', 'cr3', 'crw', 'arw', 'sr2', 'srf', 'dng', 'raf', 'orf', 'rw2', 'nrw']
const DOC_EXT = ['pdf', 'doc', 'docx']
const SHEET_EXT = ['xls', 'xlsx', 'csv']
const SLIDE_EXT = ['ppt', 'pptx']
const ARCHIVE_EXT = ['zip', 'rar', '7z', 'tar', 'gz', 'bz2']

function extOf(name = '') {
  const i = String(name).lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
}

export function isImageFile(type = '', name = '') {
  const ext = extOf(name) || type.toLowerCase()
  return IMAGE_EXT.includes(ext) || RAW_EXT.includes(ext)
}

export function isPreviewable(type = '', name = '') {
  const ext = extOf(name) || type.toLowerCase()
  return (
    IMAGE_EXT.includes(ext) ||
    RAW_EXT.includes(ext) ||
    VIDEO_EXT.includes(ext) ||
    AUDIO_EXT.includes(ext) ||
    ext === 'pdf'
  )
}

/**
 * 返回文件对应的**图标名**（`src/icons.js` 注册表里的键），交给 `<AppIcon>` 渲染。
 *
 * 原先这里返回 emoji 字符串，导致整个文件类型图标体系无法主题化：emoji 的颜色由
 * 字体决定，`--primary` 换色、暗色模式提亮都对它无效，且无法 aria-hidden。
 * 改为返回名字后，图标成了真正的组件，跟随 `color` 与主题令牌。
 *
 * 注意：不要在任何 UI 里直接渲染这个返回值，必须经 `<AppIcon :name="...">`。
 */
export function fileIcon(type = '', name = '') {
  const ext = extOf(name) || type.toLowerCase()
  if (ext === 'folder') return 'Folder'
  if (IMAGE_EXT.includes(ext) || RAW_EXT.includes(ext)) return 'Image'
  if (VIDEO_EXT.includes(ext)) return 'Film'
  if (AUDIO_EXT.includes(ext)) return 'Music'
  if (ext === 'pdf') return 'FileText'
  if (DOC_EXT.includes(ext)) return 'FileText'
  if (SHEET_EXT.includes(ext)) return 'FileSpreadsheet'
  if (SLIDE_EXT.includes(ext)) return 'Presentation'
  if (ARCHIVE_EXT.includes(ext)) return 'FileArchive'
  if (['txt', 'md', 'rtf'].includes(ext)) return 'FileText'
  if (['json', 'js', 'ts', 'py', 'rs', 'go', 'java', 'c', 'cpp', 'html', 'css'].includes(ext))
    return 'Code'
  return 'File'
}

/** Human readable file size. */
export function formatSize(bytes) {
  const n = Number(bytes) || 0
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

/**
 * 解析后端时间字符串为 Date。
 * 后端统一使用 UTC 存储（`YYYY-MM-DD HH:MM:SS`，与 SQLite datetime('now') 一致），
 * 该形态不带时区标记，必须显式按 UTC 解析，否则会被浏览器当作本地时间导致偏差。
 * 返回 null 表示无法解析。
 */
export function parseUtcDate(input) {
  if (!input) return null
  let s = String(input).trim()
  if (/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}(:\d{2})?(\.\d+)?$/.test(s)) {
    s = `${s.replace(' ', 'T')}Z`
  }
  const d = new Date(s)
  return Number.isNaN(d.getTime()) ? null : d
}

/** Format a UTC timestamp string into a localised short form. */
export function formatDate(input) {
  const d = parseUtcDate(input)
  if (!d) return input || ''
  const p = (x) => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`
}
