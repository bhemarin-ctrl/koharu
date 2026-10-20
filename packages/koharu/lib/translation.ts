import type {
  LanguageChoice,
  Model,
  ModelSelection,
  Provider,
  ProviderPreference,
} from '@koharu/bridge/protocol'

export function providerName(entries: ProviderPreference[], provider: Provider): string {
  return entries.find((entry) => entry.config.provider === provider)?.name ?? provider
}

export function modelKey(model: Model | ModelSelection): string {
  return `${model.provider}:${model.model ?? ''}`
}

export function modelSelection(model: Model, quantization: string | null): ModelSelection {
  return {
    provider: model.provider,
    model: model.model,
    quantization,
    vision: model.vision,
    reasoning: model.reasoning,
  }
}

export function orderedLanguageChoices(
  languages: readonly LanguageChoice[],
): Array<{ tag: string; name: string }> {
  return languages
    .map((language) => ({ tag: language.tag, name: language.name }))
    .sort((left, right) =>
      left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: 'base' }),
    )
}

/** The listed models, plus the selected one when the provider no longer lists it. */
export function availableModels(
  selected: ModelSelection | null,
  models: Model[],
  providers: ProviderPreference[],
): Model[] {
  if (!selected || models.some((model) => modelKey(model) === modelKey(selected))) return models
  return [
    {
      provider: selected.provider,
      model: selected.model ?? null,
      name: selected.model ?? providerName(providers, selected.provider),
      quantizations: [],
      vision: selected.vision ?? false,
      reasoning: selected.reasoning ?? false,
    },
    ...models,
  ]
}
