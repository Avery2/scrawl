// Outlines of assets/icon.svg, simplified from its potrace paths to their true corners
// (icon pixels, 1024 viewBox, y down). Brush3D fillets and resamples them before extruding.
export type Outline = readonly (readonly [number, number])[]

export const HANDLE: Outline = [
  [774, 183],
  [715, 193],
  [528, 418],
  [413, 391],
  [334, 480],
  [569, 691],
  [653, 600],
  [615, 493],
  [827, 284],
  [829, 229],
]

export const BRISTLES: Outline = [
  [323, 490],
  [195, 623],
  [230, 702],
  [282, 755],
  [342, 734],
  [313, 782],
  [371, 821],
  [442, 842],
  [559, 700],
]
