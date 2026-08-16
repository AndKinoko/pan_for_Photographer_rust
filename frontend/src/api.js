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

const instance = axios.create({
  baseURL: '',
  timeout: 120000,
})

// Attach JWT to every request when available.
instance.interceptors.request.use((config) => {
  const token = localStorage.getItem(TOKEN_KEY)
  if (token) {
    config.headers = config.headers || {}
    config.headers.Authorization = `Bearer ${token}`
  }
  return config
})

// Unwrap the unified envelope and centralise error handling.
instance.interceptors.response.use(
  (response) => {
    const body = response.data
    if (body && typeof body === 'object' && 'success' in body) {
      if (body.success) return body.data
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
 * Append the current JWT as a query param. Required for resources loaded by
 * <img>/<a download> which cannot send an Authorization header
 * (backend supports ?token= for download/media endpoints).
 */
export function authUrl(url) {
  if (!url) return url
  const token = localStorage.getItem(TOKEN_KEY)
  if (!token) return url
  const sep = url.includes('?') ? '&' : '?'
  return `${url}${sep}token=${encodeURIComponent(token)}`
}

/* ===========================================================================
   Auth API
   =========================================================================== */

export const register = (username, password) =>
  instance.post('/api/auth/register', { username, password })

export const login = (username, password) =>
  instance.post('/api/auth/login', { username, password })

export const getMe = () => instance.get('/api/auth/me')

/* ===========================================================================
   Files API
   =========================================================================== */

export const listFiles = (folderId) =>
  instance.get('/api/files', {
    params: folderId != null ? { folder_id: folderId } : {},
  })

/**
 * Upload files with per-file progress reporting.
 * @param {File[]} files
 * @param {number|null} folderId
 * @param {(loaded:number,total:number,fileIndex:number)=>void} onProgress
 */
export function uploadFiles(files, folderId, onProgress) {
  const form = new FormData()
  if (folderId != null && folderId !== '') {
    form.append('folder_id', String(folderId))
  }
  for (const f of files) form.append('file', f, f.name)

  return instance.post('/api/files/upload', form, {
    headers: { 'Content-Type': 'multipart/form-data' },
    onUploadProgress: (e) => {
      if (onProgress && e.total) onProgress(e.loaded, e.total, 0)
    },
  })
}

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

export const listTrash = () => instance.get('/api/trash')

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

export const getPublicShare = (id) => instance.get(`/api/public/shares/${id}`)

export const verifySharePassword = (id, password) =>
  instance.post(`/api/public/shares/${id}/verify`, { password })

export const publicShareDownloadUrl = (id) =>
  `/api/public/shares/${id}/download`

export const publicShareMediaUrl = (id, { thumb = false, preview = false } = {}) => {
  const params = new URLSearchParams()
  if (thumb) params.set('thumb', '1')
  if (preview) params.set('preview', '1')
  const q = params.toString()
  return `/api/public/shares/${id}/media${q ? `?${q}` : ''}`
}

/* ===========================================================================
   Search API
   =========================================================================== */

export const searchFiles = (params) =>
  instance.get('/api/search', { params })

/* ===========================================================================
   Batch API
   =========================================================================== */

export const batchMove = (payload) => instance.post('/api/batch/move', payload)

export const batchCopy = (payload) => instance.post('/api/batch/copy', payload)

export const batchDelete = (payload) =>
  instance.post('/api/batch/delete', payload)

export const batchShare = (payload) => instance.post('/api/batch/share', payload)

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

/** Returns an emoji icon for a file based on its type/name. */
export function fileIcon(type = '', name = '') {
  const ext = extOf(name) || type.toLowerCase()
  if (ext === 'folder') return '📁'
  if (IMAGE_EXT.includes(ext) || RAW_EXT.includes(ext)) return '🖼️'
  if (VIDEO_EXT.includes(ext)) return '🎬'
  if (AUDIO_EXT.includes(ext)) return '🎵'
  if (ext === 'pdf') return '📕'
  if (DOC_EXT.includes(ext)) return '📘'
  if (SHEET_EXT.includes(ext)) return '📊'
  if (SLIDE_EXT.includes(ext)) return '📙'
  if (ARCHIVE_EXT.includes(ext)) return '📦'
  if (['txt', 'md', 'rtf'].includes(ext)) return '📃'
  if (['json', 'js', 'ts', 'py', 'rs', 'go', 'java', 'c', 'cpp', 'html', 'css'].includes(ext))
    return '💻'
  return '📄'
}

/** Human readable file size. */
export function formatSize(bytes) {
  const n = Number(bytes) || 0
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

/** Format an ISO date string into a localised short form. */
export function formatDate(input) {
  if (!input) return ''
  const d = new Date(input)
  if (Number.isNaN(d.getTime())) return input
  const p = (x) => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`
}
