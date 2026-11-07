import { getVersion } from '@tauri-apps/api/app'
import { isTauri } from '@tauri-apps/api/core'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { ref, shallowRef } from 'vue'
import { t } from './i18n'
import { notifyError, notifySuccess } from './notices'
import { useProcess, type StepContext } from './stores/process'

const STARTUP_DELAY_MS = 5000
const DOWNLOAD_SECONDS = 60
const INSTALL_SECONDS = 10

export const appVersion = ref('')
export const checking = ref(false)
/** A newer release, once one has been found. */
export const update = shallowRef<Update | null>(null)

/** The user asking gets an answer either way; the check at startup stays silent. */
export async function checkForUpdate(manual = false) {
  if (!isTauri() || checking.value) return
  checking.value = true
  try {
    update.value = await check()
    if (manual && !update.value) notifySuccess(t('update.latest'))
  } catch (e) {
    if (manual) notifyError(t('update.failed', { e: String(e) }))
  } finally {
    checking.value = false
  }
}

export async function startUpdater() {
  if (!isTauri()) return
  appVersion.value = await getVersion()
  if (!import.meta.env.DEV) window.setTimeout(() => void checkForUpdate(), STARTUP_DELAY_MS)
}

async function download(next: Update, ctx: StepContext) {
  let total = 0
  let received = 0
  await next.download((e) => {
    if (e.event === 'Started') total = e.data.contentLength ?? 0
    if (e.event === 'Progress') {
      received += e.data.chunkLength
      if (total) ctx.report((received / total) * 100)
    }
  })
}

/** Installing is its own step, so stopping during the download leaves this version in place. */
export function installUpdate() {
  const next = update.value
  if (!next) return
  useProcess().open({
    title: t('update.title', { version: next.version }),
    note: t('update.note'),
    writes: false,
    steps: [
      { label: t('update.download'), estimate: () => DOWNLOAD_SECONDS, run: (ctx) => download(next, ctx) },
      { label: t('update.install'), estimate: () => INSTALL_SECONDS, run: () => next.install() },
    ],
    onDone: () => void relaunch(),
  })
}
