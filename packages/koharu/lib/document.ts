import type { Layer } from '@koharu/bridge/protocol'

export function isTextLayer(layer: Layer): layer is Extract<Layer, { type: 'text' }> {
  return layer.type === 'text'
}

export function isGroupLayer(layer: Layer): layer is Extract<Layer, { type: 'group' }> {
  return layer.type === 'group'
}

export function isLockedLayer(layer: Layer): boolean {
  return layer.type === 'artwork'
}

export function layerChildren(layers: Layer[], parent: string): Layer[] {
  return layers.filter((layer) => layer.parent === parent)
}

export function expandLayerSelection(layers: Layer[], selected: string[]): string[] {
  const result: string[] = []
  const visit = (id: string) => {
    const layer = layers.find((candidate) => candidate.id === id)
    if (!layer) return
    if (isGroupLayer(layer)) {
      for (const child of layerChildren(layers, id)) visit(child.id)
    } else if (!result.includes(id)) {
      result.push(id)
    }
  }
  for (const id of selected) visit(id)
  return result
}

export function effectiveLayerVisibility(layers: Layer[], layer: Layer) {
  let visible = layer.visibility.visible
  let opacity = layer.visibility.opacity
  let parent = layer.parent
  while (parent) {
    const group = layers.find((candidate) => candidate.id === parent)
    if (!group || !isGroupLayer(group)) break
    visible &&= group.visibility.visible
    opacity *= group.visibility.opacity
    parent = group.parent
  }
  return { visible, opacity }
}

/**
 * Reading position of each text layer, starting at 1. The order of a text
 * group is the order its text is read and sent to translation.
 */
export function readingOrder(layers: Layer[]): Map<string, number> {
  const order = new Map<string, number>()
  for (const group of layers) {
    if (!isGroupLayer(group) || group.role !== 'text') continue
    layerChildren(layers, group.id)
      .filter(isTextLayer)
      .forEach((layer, index) => order.set(layer.id, index + 1))
  }
  return order
}
