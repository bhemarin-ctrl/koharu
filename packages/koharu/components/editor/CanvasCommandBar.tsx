'use client'

import { InferenceControl } from '@/components/editor/InferenceControl'
import { runPipeline } from '@/lib/processing'
import { usePage } from '@/lib/queries'
import { useKoharuStore, type PipelineScope } from '@/lib/store'
import type { Stage } from '@koharu/bridge/protocol'

export function CanvasCommandBar() {
  const page = usePage().data
  const jobs = useKoharuStore((state) => state.jobs)
  const running = Object.values(jobs).find((job) => job.state === 'running')

  const run = (selection: PipelineScope, stages: Stage[]) => {
    if (!page) return
    void runPipeline(selection, stages, page.id).catch(() => undefined)
  }

  return (
    <header className='flex h-10 shrink-0 items-center gap-2 border-b border-border/80 bg-[var(--surface-toolbar)] px-2.5'>
      <div className='min-w-0 flex-1' />
      <InferenceControl disabled={!page || Boolean(running)} onRun={run} />
    </header>
  )
}
