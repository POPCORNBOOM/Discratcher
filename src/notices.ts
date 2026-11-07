// Ported from OpenCard (Apache-2.0), features/notifications/titlebarNotices.ts.
import { mdiAlertCircleOutline, mdiAlertOutline, mdiCheck } from '@mdi/js'
import { readonly, ref } from 'vue'

export type NoticeTone = 'success' | 'warning' | 'danger'
export interface Notice {
  id: number
  tone: NoticeTone
  icon: string
  message: string
}

const HISTORY_LIMIT = 50
const list = ref<Notice[]>([])
let nextId = 1

function add(tone: NoticeTone, icon: string, message: string) {
  list.value = [...list.value.slice(-(HISTORY_LIMIT - 1)), { id: nextId++, tone, icon, message }]
}

export const notifySuccess = (message: string) => add('success', mdiCheck, message)
export const notifyWarning = (message: string) => add('warning', mdiAlertOutline, message)
export const notifyError = (message: string) => add('danger', mdiAlertCircleOutline, message)
export const notices = readonly(list)
