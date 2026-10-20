'use client'

import { Minus, Play, Plus } from 'lucide-react'
import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'

import { runPipeline } from '@/lib/processing'
import { usePage } from '@/lib/queries'
import { pipelineStages, useKoharuStore, type PipelineScope } from '@/lib/store'
import type { Stage } from '@koharu/bridge/protocol'
import { Button } from '@koharu/ui/components/button'
import { Checkbox } from '@koharu/ui/components/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@koharu/ui/components/dialog'
import {
  NumberField,
  NumberFieldDecrement,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
} from '@koharu/ui/components/number-field'
import { RadioGroup, RadioGroupItem } from '@koharu/ui/components/radio-group'

const scopes = ['page', 'selected-pages', 'project'] as const satisfies readonly PipelineScope[]

/**
 * Runs the chosen steps over many pages, one page at a time: every step
 * finishes on a page before the next page starts, with an optional pause
 * between pages, so each page is treated as if it were processed by hand.
 */
export function BatchDialog() {
  const { t } = useTranslation()
  const page = usePage().data
  const open = useKoharuStore((state) => state.batchOpen)
  const setOpen = useKoharuStore((state) => state.setBatchOpen)
  const scope = useKoharuStore((state) => state.processingScope)
  const setScope = useKoharuStore((state) => state.setProcessingScope)
  const stages = useKoharuStore((state) => state.processingStages)
  const setStages = useKoharuStore((state) => state.setProcessingStages)
  const interval = useKoharuStore((state) => state.pageIntervalSeconds)
  const setPageInterval = useKoharuStore((state) => state.setPageIntervalSeconds)
  const selectedPages = useKoharuStore((state) => state.selectedPages)
  const jobs = useKoharuStore((state) => state.jobs)
  const running = Object.values(jobs).some((job) => job.state === 'running')
  const unavailable =
    !page || stages.length === 0 || (scope === 'selected-pages' && selectedPages.length === 0)

  // A batch is for many pages: with one page or none selected, start from the
  // whole project, as Neko does, rather than quietly running a single page.
  useEffect(() => {
    if (open && scope !== 'project' && selectedPages.length < 2) setScope('project')
    // Only when the dialog opens; later choices stay as the user makes them.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const toggle = (stage: Stage, checked: boolean) =>
    setStages(
      pipelineStages.filter((candidate) =>
        candidate === stage ? checked : stages.includes(candidate),
      ),
    )

  const start = () => {
    if (!page || unavailable || running) return
    setOpen(false)
    void runPipeline(scope, stages, page.id).catch(() => undefined)
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className='sm:max-w-md'>
        <DialogHeader>
          <DialogTitle>{t('batch.title')}</DialogTitle>
          <DialogDescription>{t('batch.description')}</DialogDescription>
        </DialogHeader>

        <section className='grid gap-2'>
          <h3 className='text-[11px] font-semibold'>{t('batch.pages')}</h3>
          <RadioGroup
            value={scope}
            onValueChange={(value) => setScope(value as PipelineScope)}
            className='gap-1.5'
          >
            {scopes.map((value) => (
              <label
                key={value}
                className='flex cursor-pointer items-start gap-2.5 rounded-lg px-1 py-1 hover:bg-foreground/[0.04] has-disabled:cursor-not-allowed has-disabled:opacity-50'
              >
                <RadioGroupItem
                  value={value}
                  disabled={value === 'selected-pages' && selectedPages.length === 0}
                  className='mt-0.5'
                />
                <span className='grid gap-0.5'>
                  <span className='text-[12px]'>{t(scopeLabel[value])}</span>
                  <span className='text-[10px] text-muted-foreground'>
                    {value === 'selected-pages'
                      ? selectedPages.length
                        ? t('inference.selectedCount', { count: selectedPages.length })
                        : t('inference.selectPagesFirst')
                      : t(scopeDetail[value])}
                  </span>
                </span>
              </label>
            ))}
          </RadioGroup>
        </section>

        <section className='grid gap-2'>
          <h3 className='text-[11px] font-semibold'>{t('batch.steps')}</h3>
          <div className='grid gap-1.5'>
            {pipelineStages.map((stage) => (
              <label
                key={stage}
                className='flex cursor-pointer items-start gap-2.5 rounded-lg px-1 py-1 hover:bg-foreground/[0.04]'
              >
                <Checkbox
                  checked={stages.includes(stage)}
                  className='mt-0.5'
                  onCheckedChange={(checked) => toggle(stage, checked)}
                />
                <span className='grid gap-0.5'>
                  <span className='text-[12px]'>{t(`phase.${stage}`)}</span>
                  <span className='text-[10px] text-muted-foreground'>
                    {t(`phaseDescription.${stage}`)}
                  </span>
                </span>
              </label>
            ))}
          </div>
        </section>

        <section className='grid gap-1.5'>
          <div className='flex items-center justify-between gap-3'>
            <h3 className='text-[11px] font-semibold'>{t('batch.pause')}</h3>
            <div className='flex items-center gap-2'>
              <NumberField
                min={0}
                max={600}
                step={5}
                value={interval}
                className='w-24'
                onValueChange={(next) => {
                  if (next !== null) setPageInterval(next)
                }}
              >
                <NumberFieldGroup className='h-7'>
                  <NumberFieldDecrement aria-label={`- ${t('batch.pause')}`}>
                    <Minus />
                  </NumberFieldDecrement>
                  <NumberFieldInput aria-label={t('batch.pause')} />
                  <NumberFieldIncrement aria-label={`+ ${t('batch.pause')}`}>
                    <Plus />
                  </NumberFieldIncrement>
                </NumberFieldGroup>
              </NumberField>
              <span className='text-[11px] text-muted-foreground'>{t('batch.seconds')}</span>
            </div>
          </div>
          <p className='text-[10px] leading-4 text-muted-foreground'>{t('batch.pauseDetail')}</p>
        </section>

        <DialogFooter className='items-center'>
          {running && (
            <span className='mr-auto text-[11px] text-muted-foreground'>{t('batch.busy')}</span>
          )}
          <Button type='button' variant='ghost' onClick={() => setOpen(false)}>
            {t('common.cancel')}
          </Button>
          <Button type='button' disabled={unavailable || running} onClick={start}>
            <Play className='size-3.5' /> {t('batch.start')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

const scopeLabel = {
  page: 'inference.currentPage',
  'selected-pages': 'inference.selectedPages',
  project: 'inference.entireProject',
} as const satisfies Record<PipelineScope, string>

const scopeDetail = {
  page: 'inference.currentPageDescription',
  'selected-pages': 'inference.selectPagesFirst',
  project: 'inference.entireProjectDescription',
} as const satisfies Record<PipelineScope, string>
