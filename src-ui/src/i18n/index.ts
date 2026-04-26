import { createI18n } from 'vue-i18n'
import en from './locales/en'
import zhCN from './locales/zh-CN'
import zhHK from './locales/zh-HK'
import zhTW from './locales/zh-TW'
import ja from './locales/ja'
import ru from './locales/ru'

export type Locale = 'en' | 'zh-CN' | 'zh-HK' | 'zh-TW' | 'ja' | 'ru'

export const LOCALES: { code: Locale; label: string }[] = [
  { code: 'en', label: 'English' },
  { code: 'zh-CN', label: '简体中文' },
  { code: 'zh-TW', label: '繁體中文（台灣）' },
  { code: 'zh-HK', label: '繁體中文（香港）' },
  { code: 'ja', label: '日本語' },
  { code: 'ru', label: 'Русский' },
]

const STORAGE_KEY = 'mousefly:locale'

function detectLocale(): Locale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored && LOCALES.some((l) => l.code === stored)) {
      return stored as Locale
    }
  } catch {
    /* localStorage unavailable */
  }
  const navLang = (navigator.language || 'en').toLowerCase()
  if (navLang.startsWith('zh')) {
    if (navLang.includes('hk')) return 'zh-HK'
    if (navLang.includes('tw') || navLang.includes('hant')) return 'zh-TW'
    return 'zh-CN'
  }
  if (navLang.startsWith('ja')) return 'ja'
  if (navLang.startsWith('ru')) return 'ru'
  return 'en'
}

export const i18n = createI18n({
  legacy: false,
  locale: detectLocale(),
  fallbackLocale: 'en',
  messages: {
    en,
    'zh-CN': zhCN,
    'zh-HK': zhHK,
    'zh-TW': zhTW,
    ja,
    ru,
  },
})

export function setLocale(loc: Locale) {
  i18n.global.locale.value = loc
  try {
    localStorage.setItem(STORAGE_KEY, loc)
  } catch {
    /* ignore */
  }
}
