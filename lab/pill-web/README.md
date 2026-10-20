# Pill playground (WebGPU)

The Metal recording pill in a browser tab: click between its states, play transitions back
slowly, scrub them frame by frame with the springs plotted, tune them with the same keys as
`scrawl tune`, and send the values back to the running app.

```sh
bun install
bun run dev            # builds public/pill.wasm, then Vite on 127.0.0.1:4466 ($PORT)
bun parity/parity.ts   # wasm vs native model.rs, frame by frame
```

## What is shared with the app, and what is a port

| Piece | Here | In the app |
|---|---|---|
| Motion model (springs, layout, gating, look-ahead cap, highlight edges, punch) | `src/native/pill/model.rs` **compiled unchanged** to wasm (`wasm/src/lib.rs` includes it by path) | the same file |
| Shaders | `src/pill.wgsl`, a line-for-line port of `pill.metal` (same three passes, structs, constants) | `src/native/pill/pill.metal` |
| Draw list | `src/renderer.ts`, a port of `render_frame` / `emit_elem_prims` (tile packing, colours, levels, Esc hint) | `src/native/pill/mod.rs` |
| Glyph atlas | `src/atlas.ts`, Canvas 2D at the render scale (13 pt system font, 500/600 weights) | `atlas.rs`, CoreText |
| Glass bottle | a CSS `backdrop-filter` div moved to the silhouette each frame | `NSGlassEffectView` moved in the same `CATransaction` |
| Coordinator | `src/coordinator.ts`: phases, auto-fold after 3.5 s, processing 0.8 s, the lab scenes | `src/ui.rs`, `src/ui/lab.rs` |
| Tick | `requestAnimationFrame`, `dt = min(frame, 25 ms) × speed`, and (toggle) the same gating: the model only advances while it animates, processes or something changed | `SharedNativePill::tick_main` on `CVDisplayLink` |

`parity/parity.ts` runs the five lab scenes under four option sets through both builds: 13,200
frames agree to 1.3e-5 (libm rounding), so what you see here is the app's motion. The stage's
**Real pill** buttons put screenshots of the real Metal pill (`scrawl lab pill`, default tune,
`public/reference/`) under the web one in the same state; paste any screenshot to compare with it.

## Translating back to Metal

- **Tuned values:** **Apply to my Scrawl** runs `scrawl tune set` for every key, and the running
  dev build reloads `~/.config/scrawl/tune.json` within 250 ms. **Copy Rust** gives the whole
  `Tune::DEFAULT` literal (pill keys included) to ship them.
- **Model changes:** edit `src/native/pill/model.rs` itself. The dev server watches it, rebuilds
  the wasm and reloads the page, so the change is the app's change; `cargo test` in `wasm/` also
  runs model.rs's own tests.
- **Look changes:** a change to `pill.wgsl` or `renderer.ts` maps one to one onto `pill.metal` or
  `render_frame` (same names, same order). Only the WGSL-specific bits differ:
  `textureSampleLevel` instead of `sample`, `select()` for ternaries, and the passes split into
  three modules because they reuse binding slots.
