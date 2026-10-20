// Line-for-line WGSL port of src/native/pill/pill.metal (same passes, structs and constants).
// Differences: `textureSampleLevel` everywhere (WGSL needs uniform control flow for implicit-LOD
// sampling), and the glass bottle itself is a CSS backdrop-filter layer driven by the same
// silhouette, where the Mac uses NSGlassEffectView.

// Shared by every pass; the passes below are split into separate modules (they reuse bindings).

struct TileUniforms {
  atlas_size: vec2f,
  canvas_size: vec2f,
  scale: f32,
  _pad0: f32,
  _pad1: vec2f,
}

const CORNERS = array<vec2f, 6>(
  vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
  vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
);

// ─── Pass 1: Render elements into dedicated tiles in offscreen_a (960 × 256 pt) ────────────

// kind == 0: analytic rounded rect (`uv_rect.x` = corner radius); kind == 1: atlas sprite.
struct TilePrim {
  rect: vec4f,
  uv_rect: vec4f,
  color: vec4f,
  kind: u32,
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
}

struct TileVertexOut {
  @builtin(position) position: vec4f,
  @location(0) pt: vec2f,
  @location(1) uv: vec2f,
  @location(2) @interpolate(flat) rect: vec4f,
  @location(3) @interpolate(flat) color: vec4f,
  @location(4) @interpolate(flat) radius: f32,
  @location(5) @interpolate(flat) kind: u32,
}

@group(0) @binding(0) var<uniform> tu: TileUniforms;
@group(0) @binding(1) var<storage, read> prims: array<TilePrim>;
@group(0) @binding(2) var tile_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

@vertex
fn pill_tile_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> TileVertexOut {
  let prim = prims[iid];
  let c = CORNERS[vid];
  // For analytic shapes, expand quad by 1.5 pt so anti-aliased edges never clip.
  let pad = select(0.0, 1.5, prim.kind == 0u);
  let p = prim.rect.xy - vec2f(pad) + c * (prim.rect.zw + vec2f(2.0 * pad));
  let ndc = vec2f((p.x / tu.canvas_size.x) * 2.0 - 1.0, 1.0 - (p.y / tu.canvas_size.y) * 2.0);
  var out: TileVertexOut;
  out.position = vec4f(ndc, 0.0, 1.0);
  out.pt = p;
  out.uv = mix(prim.uv_rect.xy, prim.uv_rect.zw, c);
  out.rect = prim.rect;
  out.color = prim.color;
  out.radius = prim.uv_rect.x;
  out.kind = prim.kind;
  return out;
}

@fragment
fn pill_tile_fragment(in: TileVertexOut) -> @location(0) vec4f {
  if (in.kind == 0u) {
    let center = in.rect.xy + in.rect.zw * 0.5;
    let half_sz = in.rect.zw * 0.5;
    let r = min(in.radius, min(half_sz.x, half_sz.y));
    let q = abs(in.pt - center) - (half_sz - vec2f(r));
    let sdf = length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
    let cov = clamp(0.5 - sdf * tu.scale, 0.0, 1.0);
    let a = in.color.a * cov;
    return vec4f(in.color.rgb * a, a);
  }
  let tex = textureSampleLevel(tile_tex, samp, in.uv, 0.0);
  let a = in.color.a * tex.a;
  return vec4f(in.color.rgb * a, a);
}

// ─── Pass 2: 1D Horizontal Gaussian Blur (offscreen_a -> offscreen_b) ──────────────────────

struct BlurBox {
  tile_rect: vec4f,
  sigma_pt: f32,
  _pad0: f32,
  _pad1: vec2f,
}

struct BlurVertexOut {
  @builtin(position) position: vec4f,
  @location(0) pt: vec2f,
  @location(1) @interpolate(flat) tile_rect: vec4f,
  @location(2) @interpolate(flat) sigma_pt: f32,
}

@group(0) @binding(0) var<uniform> bu: TileUniforms;
@group(0) @binding(1) var<storage, read> boxes: array<BlurBox>;
@group(0) @binding(2) var blur_src: texture_2d<f32>;
@group(0) @binding(3) var blur_samp: sampler;

@vertex
fn pill_blur_h_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> BlurVertexOut {
  let b = boxes[iid];
  let c = CORNERS[vid];
  let p = b.tile_rect.xy + c * b.tile_rect.zw;
  let ndc = vec2f((p.x / bu.canvas_size.x) * 2.0 - 1.0, 1.0 - (p.y / bu.canvas_size.y) * 2.0);
  var out: BlurVertexOut;
  out.position = vec4f(ndc, 0.0, 1.0);
  out.pt = p;
  out.tile_rect = b.tile_rect;
  out.sigma_pt = b.sigma_pt;
  return out;
}

@fragment
fn pill_blur_h_fragment(in: BlurVertexOut) -> @location(0) vec4f {
  let inv_canvas = 1.0 / bu.canvas_size;
  let sigma = in.sigma_pt;
  if (sigma < 0.05) {
    return textureSampleLevel(blur_src, blur_samp, in.pt * inv_canvas, 0.0);
  }
  let min_x = in.tile_rect.x + 0.5;
  let max_x = in.tile_rect.x + in.tile_rect.z - 0.5;
  let radius = min(24, i32(ceil(sigma * 3.0 * bu.scale)));
  let step_pt = 1.0 / bu.scale;
  let inv_2s2 = 0.5 / max(sigma * sigma, 1e-4);
  var acc = vec4f(0.0);
  var w_sum = 0.0;
  for (var k = -radius; k <= radius; k++) {
    let dx = f32(k) * step_pt;
    let w = exp(-(dx * dx) * inv_2s2);
    let sx = clamp(in.pt.x + dx, min_x, max_x);
    acc += textureSampleLevel(blur_src, blur_samp, vec2f(sx, in.pt.y) * inv_canvas, 0.0) * w;
    w_sum += w;
  }
  return acc / max(w_sum, 1e-5);
}

// ─── Pass 3: Analytic SDF Capsule + 1D Vertical Blurred Element Composite + Hairline Border ─

struct CompositeElem {
  screen_rect: vec4f,
  tile_rect: vec4f,
  base_size: vec2f,
  scale: f32,
  alpha: f32,
  sigma_pt: f32,
  pad_pt: f32,
  levels: vec2f,
}

struct PillUniforms {
  panel_size: vec2f,
  canvas_size: vec2f,
  scale: f32,
  elem_count: u32,
  is_completed: u32,
  has_ghost: u32,
  shape_rect: vec4f,
  shape_meta: vec4f,
  ghost_rect: vec4f,
  ghost_meta: vec4f,
  hint_rect: vec4f,
  hint_uv: vec4f,
  hl_rect: vec4f,
  hl_meta: vec4f,
}

struct ScreenVertexOut {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
}

@group(0) @binding(0) var<uniform> u: PillUniforms;
@group(0) @binding(1) var<storage, read> elems: array<CompositeElem>;
@group(0) @binding(2) var offscreen_b: texture_2d<f32>;
@group(0) @binding(3) var atlas: texture_2d<f32>;
@group(0) @binding(4) var s: sampler;

@vertex
fn pill_screen_vertex(@builtin(vertex_index) vid: u32) -> ScreenVertexOut {
  let pos = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  var out: ScreenVertexOut;
  out.position = vec4f(pos[vid], 0.0, 1.0);
  out.uv = pos[vid] * vec2f(0.5, -0.5) + 0.5;
  return out;
}

fn rrect_sdf(p: vec2f, rect: vec4f, r: f32) -> f32 {
  let center = (rect.xy + rect.zw) * 0.5;
  let half_sz = max((rect.zw - rect.xy) * 0.5, vec2f(0.0));
  let cr = min(r, min(half_sz.x, half_sz.y));
  let q = abs(p - center) - (half_sz - vec2f(cr));
  return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - cr;
}

fn blend_over(dst: vec4f, src: vec4f) -> vec4f {
  return src + dst * (1.0 - src.a);
}

@fragment
fn pill_composite_fragment(in: ScreenVertexOut) -> @location(0) vec4f {
  let p = in.uv * u.panel_size;
  var out = vec4f(0.0);

  let shape_alpha = u.shape_meta.y;
  if (shape_alpha <= 0.001 && u.has_ghost == 0u) {
    return out;
  }

  let r = u.shape_meta.x;
  let is_glass = u.shape_meta.w;
  let sdf = rrect_sdf(p, u.shape_rect, r);

  // 1. Soft drop shadow behind the capsule (black mode only).
  if (is_glass < 0.5) {
    let sdf_shadow = rrect_sdf(p - vec2f(0.0, 4.0), u.shape_rect, r);
    if (sdf_shadow < 22.0) {
      let sh_d = max(sdf_shadow, 0.0);
      let sh_u = sh_d / 7.0;
      let sh_a = 0.16 * shape_alpha * exp(-0.5 * sh_u * sh_u);
      out = blend_over(out, vec4f(0.0, 0.0, 0.0, sh_a));
    }
  }

  // 2. Optional Reduce Motion ghost silhouette.
  let bg_rgb = vec3f(20.0 / 255.0, 20.0 / 255.0, 22.0 / 255.0);
  if (is_glass < 0.5 && u.has_ghost != 0u && u.ghost_meta.y > 0.001) {
    let g_sdf = rrect_sdf(p, u.ghost_rect, u.ghost_meta.x);
    let g_cov = clamp(0.5 - g_sdf * u.scale, 0.0, 1.0);
    let g_a = 0.96 * u.ghost_meta.y * g_cov;
    out = blend_over(out, vec4f(bg_rgb * g_a, g_a));
  }

  // 3. Main capsule fill, feathered over ±1.75σ while the shell is blurred.
  let half_band = max(0.5 / u.scale, u.shape_meta.z * 1.75);
  let in_cov = smoothstep(half_band, -half_band, sdf);
  if (in_cov > 0.001) {
    if (is_glass < 0.5) {
      let fill_a = 0.96 * shape_alpha * in_cov;
      out = blend_over(out, vec4f(bg_rgb * fill_a, fill_a));
    }

    // 4a. The selection highlight.
    var hl_cov = 0.0;
    if (u.hl_meta.y > 0.001) {
      let h_sdf = rrect_sdf(p, u.hl_rect, u.hl_meta.x);
      let h_band = max(0.5 / u.scale, u.hl_meta.z * 1.75);
      hl_cov = smoothstep(h_band, -h_band, h_sdf) * u.hl_meta.y;
      let hl_rgb = select(vec3f(0.06, 0.07, 0.10), vec3f(1.0), is_glass < 0.5);
      let hl_a = select(0.18, 0.16, is_glass < 0.5) * hl_cov * in_cov;
      out = blend_over(out, vec4f(hl_rgb * hl_a, hl_a));
    }

    // 4b. Elements, vertically blurred from offscreen_b and clipped to the capsule.
    let inv_canvas = 1.0 / u.canvas_size;
    let count = min(u.elem_count, 16u);
    for (var i = 0u; i < count; i++) {
      let e = elems[i];
      let exp_pad = e.pad_pt * e.scale;
      let min_p = e.screen_rect.xy - vec2f(exp_pad);
      let max_p = e.screen_rect.xy + e.screen_rect.zw + vec2f(exp_pad);
      if (p.x < min_p.x || p.x > max_p.x || p.y < min_p.y || p.y > max_p.y) {
        continue;
      }
      let local_pt = (p - e.screen_rect.xy) / max(e.scale, 0.05) + vec2f(e.pad_pt);
      let tile_pt = e.tile_rect.xy + local_pt;
      let min_ty = e.tile_rect.y + 0.5;
      let max_ty = e.tile_rect.y + e.tile_rect.w - 0.5;
      if (tile_pt.x < e.tile_rect.x || tile_pt.x > e.tile_rect.x + e.tile_rect.z) {
        continue;
      }

      var sample_col = vec4f(0.0);
      let sigma = e.sigma_pt;
      if (sigma < 0.05) {
        if (tile_pt.y >= min_ty && tile_pt.y <= max_ty) {
          sample_col = textureSampleLevel(offscreen_b, s, tile_pt * inv_canvas, 0.0);
        }
      } else {
        let radius = min(24, i32(ceil(sigma * 3.0 * u.scale)));
        let step_pt = 1.0 / u.scale;
        let inv_2s2 = 0.5 / max(sigma * sigma, 1e-4);
        var w_sum = 0.0;
        for (var k = -radius; k <= radius; k++) {
          let dy = f32(k) * step_pt;
          let w = exp(-(dy * dy) * inv_2s2);
          let sy = clamp(tile_pt.y + dy, min_ty, max_ty);
          sample_col += textureSampleLevel(offscreen_b, s, vec2f(tile_pt.x, sy) * inv_canvas, 0.0) * w;
          w_sum += w;
        }
        sample_col /= max(w_sum, 1e-5);
        if (sample_col.a > 1e-4) {
          let blur_boost = 1.0 + min(sigma, 8.0) * 0.35;
          let boosted_a = min(1.0, sample_col.a * blur_boost);
          sample_col *= (boosted_a / sample_col.a);
        }
      }
      // Glass: a 1 pt dark backing under labels so they hold over bright pages.
      if (is_glass >= 0.5 && sigma < 3.5) {
        let sh_y = clamp(tile_pt.y - 0.85, min_ty, max_ty);
        let sh_a = textureSampleLevel(offscreen_b, s, vec2f(tile_pt.x, sh_y) * inv_canvas, 0.0).a;
        let backing_a = min(1.0, sh_a * 0.64) * e.alpha * in_cov;
        out = blend_over(out, vec4f(0.0, 0.0, 0.0, backing_a));
      }
      let level = mix(e.levels.x, e.levels.y, hl_cov);
      sample_col = vec4f(sample_col.rgb * level, sample_col.a);
      let eff_a = e.alpha * in_cov;
      out = blend_over(out, sample_col * eff_a);
    }
  }

  // 5. 1 px hairline rim (green when Completed; glass draws its own rim otherwise).
  if (is_glass < 0.5 || u.is_completed != 0u) {
    let border_sharp = clamp(1.0 - u.shape_meta.z * 0.22, 0.0, 1.0);
    let border_cov = clamp(1.0 + 0.5 / u.scale + sdf, 0.0, 1.0)
      * clamp(0.5 / u.scale - sdf, 0.0, 1.0)
      * border_sharp;
    if (border_cov > 0.001) {
      let done = u.is_completed != 0u;
      let border_rgb = select(vec3f(1.0), vec3f(105.0 / 255.0, 216.0 / 255.0, 159.0 / 255.0), done);
      let border_max_a = select(0.12, 0.42, done);
      let ba = border_cov * border_max_a * shape_alpha;
      out = blend_over(out, vec4f(border_rgb * ba, ba));
    }
  }

  // 6. "Esc" hint pill below the hovered cancel chip.
  if (u.hint_rect.z > 1.0) {
    let hr = vec4f(u.hint_rect.xy, u.hint_rect.xy + u.hint_rect.zw);
    let h_sdf = rrect_sdf(p, hr, u.hint_rect.w * 0.5);
    let h_cov = clamp(0.5 - h_sdf * u.scale, 0.0, 1.0);
    if (h_cov > 0.001) {
      let ha = 0.92 * shape_alpha * h_cov;
      out = blend_over(out, vec4f(vec3f(0.11, 0.11, 0.12) * ha, ha));
      let t_min = u.hint_rect.xy + vec2f(6.0, 2.5);
      let t_max = u.hint_rect.xy + u.hint_rect.zw - vec2f(6.0, 2.5);
      if (p.x >= t_min.x && p.x <= t_max.x && p.y >= t_min.y && p.y <= t_max.y) {
        let t_uv = (p - t_min) / max(t_max - t_min, vec2f(1.0));
        let uv = mix(u.hint_uv.xy, u.hint_uv.zw, t_uv);
        let txt_a = textureSampleLevel(atlas, s, uv, 0.0).a * shape_alpha * h_cov;
        out = blend_over(out, vec4f(vec3f(0.78, 0.78, 0.80) * txt_a, txt_a));
      }
    }
    let hb_cov = clamp(1.0 + 0.5 / u.scale + h_sdf, 0.0, 1.0) * clamp(0.5 / u.scale - h_sdf, 0.0, 1.0);
    if (hb_cov > 0.001) {
      let hba = hb_cov * 0.16 * shape_alpha;
      out = blend_over(out, vec4f(vec3f(1.0) * hba, hba));
    }
  }

  return out;
}
