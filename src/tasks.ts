// Ported from OpenCard (Apache-2.0), features/shell/composables/useShellProgressTasks.ts.
import { computed, reactive } from 'vue'

export interface ProgressTask {
  key: string
  title: string
  detail?: string
  /** 0 to 1. */
  progress: number
}

const registry = reactive(new Map<string, ProgressTask>())

export const tasks = computed<readonly ProgressTask[]>(() => [...registry.values()])
/** Overall progress for the title bar, or null when nothing is running. */
export const overallProgress = computed(() => {
  if (!registry.size) return null
  return tasks.value.reduce((sum, t) => sum + t.progress, 0) / registry.size
})

export function setTask(task: ProgressTask) {
  registry.set(task.key, { ...task, progress: Math.min(1, Math.max(0, task.progress)) })
}
export function removeTask(key: string) {
  registry.delete(key)
}
