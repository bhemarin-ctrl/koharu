'use client'

import {
  BookOpenText,
  Check,
  ChevronDown,
  Eraser,
  Languages,
  LoaderCircle,
  Scan,
  ScanText,
  Sparkles,
  type LucideIcon,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { ModelPicker } from '@/components/controls/ModelPicker'
import { BatchDialog } from '@/components/editor/BatchDialog'
import {
  defaultModel,
  modelNames,
  modelOptions,
  replaceStage,
  stageModel,
  type ModelName,
  type ModelStage,
  type PipelineModel,
} from '@/components/preferences/models'
import { call, refreshTranslationModels, savePreferences } from '@/lib/backend'
import { runPipeline } from '@/lib/processing'
import { pageKey, refresh, usePage } from '@/lib/queries'
import { receivePreferences, useKoharuStore } from '@/lib/store'
import { availableModels, modelKey } from '@/lib/translation'
import {
  commands,
  type Model,
  type ModelSelection,
  type Page,
  type PipelineConfig,
  type ProviderPreference,
  type Stage,
} from '@koharu/bridge/protocol'
import { Button } from '@koharu/ui/components/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@koharu/ui/components/dialog'
import { Popover, PopoverContent, PopoverTrigger } from '@koharu/ui/components/popover'
import { Textarea } from '@koharu/ui/components/textarea'
import { Tooltip, TooltipContent, TooltipTrigger } from '@koharu/ui/components/tooltip'
import { cn } from '@koharu/ui/lib/utils'

const stageIcons = {
  detection: Scan,
  ocr: ScanText,
  summary: BookOpenText,
  translation: Languages,
  inpainting: Eraser,
} as const satisfies Record<Stage, LucideIcon>

const railStages = ['detection', 'ocr', 'summary', 'translation', 'inpainting'] as const

/**
 * One button per pipeline step, run on the page on screen, each with its own
 * model menu, and the batch dialog on top. Mirrors processing a page by hand.
 */
export function StageRail() {
  const { t } = useTranslation()
  const page = usePage().data
  const jobs = useKoharuStore((state) => state.jobs)
  const setBatchOpen = useKoharuStore((state) => state.setBatchOpen)
  const running = Object.values(jobs).find((job) => job.state === 'running') ?? null
  const [summaryOpen, setSummaryOpen] = useState(false)

  const run = (stage: Stage) => {
    if (!page) return
    if (stage === 'summary' && page.scene_summary) {
      setSummaryOpen(true)
      return
    }
    void runPipeline('page', [stage], page.id).catch(() => undefined)
  }

  return (
    <aside
      aria-label={t('stageRail.title')}
      className='flex w-14 shrink-0 flex-col items-center gap-1.5 border-r border-border/60 bg-[var(--surface-toolbar)] px-1 py-3'
    >
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              type='button'
              variant='secondary'
              size='icon'
              aria-label={t('stageRail.batch')}
              className='h-8 w-11 rounded-xl text-primary hover:bg-primary/10'
              onClick={() => setBatchOpen(true)}
            />
          }
        >
          <Sparkles className='size-4' />
        </TooltipTrigger>
        <TooltipContent side='right'>{t('stageRail.batch')}</TooltipContent>
      </Tooltip>

      <span className='my-1 h-px w-8 bg-border' />

      {railStages.map((stage) => (
        <StageButton
          key={stage}
          stage={stage}
          page={page ?? null}
          busy={Boolean(running)}
          active={
            running !== null && running.stage === stage && (!page || running.page === page.id)
          }
          done={stage === 'summary' && Boolean(page?.scene_summary)}
          onRun={() => run(stage)}
        />
      ))}

      <BatchDialog />
      {page && (
        <SummaryDialog
          page={page}
          open={summaryOpen}
          busy={Boolean(running)}
          onOpenChange={setSummaryOpen}
        />
      )}
    </aside>
  )
}

function StageButton({
  stage,
  page,
  busy,
  active,
  done,
  onRun,
}: {
  stage: Stage
  page: Page | null
  busy: boolean
  active: boolean
  done: boolean
  onRun: () => void
}) {
  const { t } = useTranslation()
  const Icon = stageIcons[stage]
  const name = t(`phase.${stage}`)
  const label = done ? t('summary.view') : t('stageRail.run', { stage: name })

  return (
    <div className='flex h-8 w-11 items-stretch overflow-hidden rounded-xl bg-foreground/[0.05]'>
      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type='button'
              aria-label={label}
              disabled={!page || (busy && !(done && !active))}
              className='relative grid flex-1 place-items-center text-muted-foreground transition-colors hover:bg-primary/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25 disabled:opacity-35'
              onClick={onRun}
            />
          }
        >
          {active ? (
            <LoaderCircle className='size-4 animate-spin text-primary motion-reduce:animate-none' />
          ) : (
            <Icon className='size-4' />
          )}
          {done && !active && (
            <span className='absolute right-0.5 bottom-0.5 grid size-3 place-items-center rounded-full bg-primary text-primary-foreground'>
              <Check className='size-2' strokeWidth={3} />
            </span>
          )}
        </TooltipTrigger>
        <TooltipContent side='right'>
          {active ? t('stageRail.running', { stage: name }) : label}
        </TooltipContent>
      </Tooltip>
      <StageModelMenu stage={stage} name={name} disabled={busy} />
    </div>
  )
}

function StageModelMenu({
  stage,
  name,
  disabled,
}: {
  stage: Stage
  name: string
  disabled: boolean
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState<string | null>(null)
  const preferences = useKoharuStore((state) => state.preferences)
  const translationModels = useKoharuStore((state) => state.translationModels)

  const save = (pipeline: PipelineConfig, key: string) => {
    if (!preferences || saving) return
    setSaving(key)
    void savePreferences(pipeline, preferences.providers, preferences.typesetting)
      .then((saved) => {
        receivePreferences(saved)
        setOpen(false)
      })
      .catch(() => undefined)
      .finally(() => setSaving(null))
  }

  const handleOpenChange = (next: boolean) => {
    setOpen(next)
    if (!next || (stage !== 'translation' && stage !== 'summary')) return
    setLoading(true)
    void refreshTranslationModels(true)
      .catch(() => undefined)
      .finally(() => setLoading(false))
  }

  const pipeline = preferences?.pipeline ?? null
  const providers = preferences?.providers.entries ?? []

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger
        type='button'
        aria-label={t('stageRail.chooseModel', { stage: name })}
        disabled={disabled || !pipeline}
        className='grid w-3.5 shrink-0 place-items-center text-muted-foreground transition-colors outline-none hover:bg-primary/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25 disabled:opacity-35 data-open:bg-primary/10 data-open:text-foreground'
      >
        <ChevronDown className='size-3' />
      </PopoverTrigger>
      <PopoverContent
        side='right'
        align='start'
        sideOffset={6}
        className='w-64 min-w-0 gap-0 overflow-hidden rounded-xl border border-border/50 p-1 shadow-sm ring-0'
      >
        {pipeline && stage === 'translation' && (
          <ModelPicker
            value={pipeline.translation.model}
            models={availableModels(pipeline.translation.model, translationModels, providers)}
            providers={providers}
            loading={loading}
            disabled={disabled}
            busyModel={saving}
            onBack={() => setOpen(false)}
            onSelect={(model) =>
              save(
                { ...pipeline, translation: { ...pipeline.translation, model } },
                modelKey(model),
              )
            }
          />
        )}
        {pipeline && stage === 'summary' && (
          <SummaryModelMenu
            pipeline={pipeline}
            models={translationModels}
            providers={providers}
            loading={loading}
            saving={saving}
            onClose={() => setOpen(false)}
            onSelect={(model, key) => save({ ...pipeline, summary: { model } }, key)}
          />
        )}
        {pipeline && stage !== 'translation' && stage !== 'summary' && (
          <ProcessorMenu
            stage={stage}
            pipeline={pipeline}
            name={name}
            saving={saving}
            onSelect={(next) => save(next, stage)}
          />
        )}
      </PopoverContent>
    </Popover>
  )
}

function SummaryModelMenu({
  pipeline,
  models,
  providers,
  loading,
  saving,
  onClose,
  onSelect,
}: {
  pipeline: PipelineConfig
  models: Model[]
  providers: ProviderPreference[]
  loading: boolean
  saving: string | null
  onClose: () => void
  onSelect: (model: ModelSelection | null, key: string) => void
}) {
  const { t } = useTranslation()
  const selected = pipeline.summary.model ?? null
  const visionModels = availableModels(selected, models, providers).filter((model) => model.vision)

  return (
    <div className='grid min-w-0 gap-1'>
      <Button
        type='button'
        variant='ghost'
        aria-pressed={selected === null}
        disabled={Boolean(saving)}
        className='h-auto min-h-9 justify-start gap-2 rounded-lg px-2 py-1 text-left font-normal hover:bg-primary/10'
        onClick={() => onSelect(null, 'translation')}
      >
        <span className='min-w-0 flex-1'>
          <span className='block text-[11px]'>{t('stageRail.sameAsTranslation')}</span>
          <span className='block truncate text-[9px] text-muted-foreground'>
            {t('stageRail.sameAsTranslationDetail')}
          </span>
        </span>
        {selected === null && <Check className='size-3.5 shrink-0 text-primary' />}
      </Button>
      <p className='px-2 text-[9px] leading-3 text-muted-foreground'>
        {t('stageRail.needsVision')}
      </p>
      <ModelPicker
        value={selected}
        models={visionModels}
        providers={providers}
        loading={loading}
        busyModel={saving}
        onBack={onClose}
        onSelect={(model) => onSelect(model, modelKey(model))}
      />
    </div>
  )
}

function ProcessorMenu({
  stage,
  pipeline,
  name,
  saving,
  onSelect,
}: {
  stage: ModelStage
  pipeline: PipelineConfig
  name: string
  saving: string | null
  onSelect: (pipeline: PipelineConfig) => void
}) {
  const current = stageModel(pipeline, stage).model
  const choose = (model: ModelName) => {
    if (model === current) return
    // Keep the settings the user already gave this model, as the settings
    // page does when it switches back to a model.
    const profile = pipeline.processor?.[model as keyof NonNullable<PipelineConfig['processor']>]
    const next = { ...defaultModel(model), ...profile } as PipelineModel
    onSelect(replaceStage(pipeline, stage, next))
  }

  return (
    <div className='grid min-w-0 gap-0.5'>
      <div className='mb-0.5 border-b border-border/60 px-2 pt-1 pb-1.5 text-[11px] font-medium'>
        {name}
      </div>
      {modelOptions[stage].map((model) => (
        <Button
          key={model}
          type='button'
          variant='ghost'
          aria-pressed={model === current}
          disabled={Boolean(saving)}
          className='h-8 justify-start gap-2 rounded-lg px-2 text-[11px] font-normal hover:bg-primary/10'
          onClick={() => choose(model)}
        >
          <span className='min-w-0 flex-1 truncate text-left'>{modelNames[model]}</span>
          {model === current && <Check className='size-3.5 shrink-0 text-primary' />}
        </Button>
      ))}
    </div>
  )
}

function SummaryDialog({
  page,
  open,
  busy,
  onOpenChange,
}: {
  page: Page
  open: boolean
  busy: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useTranslation()
  const summary = page.scene_summary
  const [draft, setDraft] = useState(summary?.text ?? '')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (open) setDraft(summary?.text ?? '')
  }, [open, summary?.text])

  const write = (text: string | null) => {
    setSaving(true)
    return call(commands.setSceneSummary, page.id, text)
      .then(() => refresh(pageKey))
      .finally(() => setSaving(false))
  }

  const clear = () =>
    void write(null)
      .then(() => onOpenChange(false))
      .catch(() => undefined)

  const regenerate = () =>
    void write(null)
      .then(() => {
        onOpenChange(false)
        return runPipeline('page', ['summary'], page.id)
      })
      .catch(() => undefined)

  const save = () =>
    void write(draft.trim() ? draft : null)
      .then(() => onOpenChange(false))
      .catch(() => undefined)

  const changed = draft !== (summary?.text ?? '')

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-lg'>
        <DialogHeader>
          <DialogTitle>{t('summary.title')}</DialogTitle>
          <DialogDescription>{t('summary.description')}</DialogDescription>
        </DialogHeader>
        {summary && (
          <span
            className={cn(
              'w-fit rounded-md px-1.5 py-0.5 text-[10px]',
              summary.generated
                ? 'bg-foreground/[0.06] text-muted-foreground'
                : 'bg-primary/10 text-primary',
            )}
          >
            {summary.generated ? t('summary.generated') : t('summary.edited')}
          </span>
        )}
        <Textarea
          aria-label={t('summary.title')}
          value={draft}
          placeholder={t('summary.placeholder')}
          className='max-h-72 min-h-32 text-[12px] leading-5'
          onChange={(event) => setDraft(event.currentTarget.value)}
        />
        <DialogFooter className='sm:justify-between'>
          <div className='flex gap-2'>
            <Button type='button' variant='ghost' disabled={saving || !summary} onClick={clear}>
              {t('summary.clear')}
            </Button>
            <Button type='button' variant='ghost' disabled={saving || busy} onClick={regenerate}>
              {t('summary.regenerate')}
            </Button>
          </div>
          <div className='flex gap-2'>
            <Button type='button' variant='outline' onClick={() => onOpenChange(false)}>
              {t('summary.close')}
            </Button>
            <Button type='button' disabled={saving || !changed} onClick={save}>
              {t('summary.save')}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
