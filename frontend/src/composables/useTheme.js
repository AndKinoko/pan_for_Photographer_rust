import { ref } from 'vue'

/**
 * Theme management composable.
 * - Persists choice to localStorage ("pan_theme")
 * - Falls back to system prefers-color-scheme on first visit
 * - Applies `data-theme` attribute on <html>
 */

const STORAGE_KEY = 'pan_theme'

function detectInitial() {
  const stored = localStorage.getItem(STORAGE_KEY)
  if (stored === 'light' || stored === 'dark') return stored
  if (
    window.matchMedia &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  ) {
    return 'dark'
  }
  return 'light'
}

const theme = ref(detectInitial())

function apply(value) {
  theme.value = value
  document.documentElement.setAttribute('data-theme', value)
  localStorage.setItem(STORAGE_KEY, value)
}

// Apply immediately on module load so there is no flash of wrong theme.
apply(theme.value)

export function useTheme() {
  function toggle() {
    apply(theme.value === 'dark' ? 'light' : 'dark')
  }
  function setTheme(value) {
    if (value === 'light' || value === 'dark') apply(value)
  }
  return { theme, toggle, setTheme }
}

export default useTheme
