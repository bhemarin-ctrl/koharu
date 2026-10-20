'use client'

import { call } from '@/lib/backend'
import { pipelineStages, useKoharuStore, type PipelineScope } from '@/lib/store'
import {
  commands,
  type EntityId,
  type Operation,
  type Scope,
  type Stage,
} from '@koharu/bridge/protocol'

/**
 * Starts the pipeline. Whenever more than one page can be involved, pages run
 * one at a time, each through every requested stage before the next starts,
 * so a batch gives the same result as processing the pages by hand.
 */
export function runPipeline(selection: PipelineScope, stages: readonly Stage[], page: EntityId) {
  const { selectedPages, pageIntervalSeconds } = useKoharuStore.getState()
  const scope: Scope =
    selection === 'project'
      ? { scope: 'project' }
      : selection === 'selected-pages'
        ? { scope: 'pages', value: selectedPages }
        : { scope: 'pages', value: [page] }
  const singlePage =
    selection === 'page' || (selection === 'selected-pages' && selectedPages.length < 2)
  return call(commands.process, scope, operation(stages), singlePage ? null : pageIntervalSeconds)
}

function operation(stages: readonly Stage[]): Operation {
  const ordered = pipelineStages.filter((stage) => stages.includes(stage))
  if (ordered.length === pipelineStages.length) return { operation: 'full' }
  if (ordered.length === 1) return { operation: 'only', stage: ordered[0]! }
  return { operation: 'stages', stages: ordered }
}
