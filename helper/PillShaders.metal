// SwiftUI shaders for the SwiftUI capsule (`helper/Pill.swift`), applied with `.colorEffect` and
// `.layerEffect` and driven by the same animation values as the shape. `build.rs` compiles this
// into a metallib and embeds it in scrawl-helper (`PillShaderData.swift`).

#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

/// The capsule's rim: a hairline lit from above (brightest on the top arc), a faint sheen under
/// it and a soft inner glow along the edge, all brightening with `energy` while the shape moves.
/// Green when the capture is done.
[[ stitchable ]] half4 pillRim(float2 pos, half4 color, float2 size, float energy, float completed) {
    if (color.a < 0.001h) { return color; }
    float r = size.y * 0.5;
    float2 c = float2(clamp(pos.x, r, max(size.x - r, r)), r);
    float inside = r - length(pos - c);
    float rim = smoothstep(1.1, 0.15, inside);
    float top = 1.0 - clamp(pos.y / max(size.y, 1.0), 0.0, 1.0);
    float light = (0.07 + 0.20 * top * top) * (1.0 + 1.4 * energy);
    float sheen = 0.035 * smoothstep(0.55, 1.0, top) * smoothstep(0.0, 6.0, inside);
    // A soft inner glow hugging the whole edge, a breath of light that swells as the shape moves.
    float g = inside / 7.0;
    float glow = exp(-g * g) * (0.018 + 0.05 * energy) * (0.6 + 0.4 * top);
    half3 tint = mix(half3(1.0h), half3(0.412h, 0.847h, 0.624h), half(completed));
    float lightA = completed > 0.5 ? max(light, 0.32) : light;
    half add = half(rim * lightA + sheen + glow) * color.a;
    return half4(min(color.rgb + tint * add, half3(color.a)), color.a);
}

/// The recording dot: an 8 pt core breathing between .78 and 1, a soft glow that swells with
/// it, and a ring that ripples out from 4 to 10 pt and cools as it goes (`ripple` 0 → 1).
[[ stitchable ]] half4 pillDot(float2 pos, half4 color, float2 size, float breath, float ripple) {
    float d = length(pos - size * 0.5);
    float core = smoothstep(4.4, 3.6, d);
    float spread = 2.2 + 1.6 * breath;
    float g = max(d - 3.6, 0.0) / spread;
    float glow = exp(-g * g) * (0.16 + 0.14 * breath);
    float ringR = 4.0 + 6.0 * ripple;
    float rf = (d - ringR) / 1.1;
    float fade = (1.0 - ripple) * (1.0 - ripple);
    float ring = exp(-rf * rf) * 0.34 * fade;
    float a = clamp(core * (0.78 + 0.22 * breath) + (1.0 - core) * (glow + ring), 0.0, 1.0);
    half3 red = half3(0.941h, 0.267h, 0.220h);
    return half4(red * half(a), half(a)) * color.a;
}

/// Motion blur along the shape's travel: a horizontal Gaussian of `radius` pt, only while it
/// morphs (persistent chips never blur; this goes on the shell alone).
[[ stitchable ]] half4 pillMorphBlur(float2 pos, SwiftUI::Layer layer, float radius) {
    if (radius < 0.05) { return layer.sample(pos); }
    half4 acc = half4(0.0h);
    float total = 0.0;
    for (int i = -6; i <= 6; i++) {
        float k = float(i) / 6.0;
        float w = exp(-4.0 * k * k);
        acc += layer.sample(pos + float2(k * radius * 2.0, 0.0)) * half(w);
        total += w;
    }
    return acc / half(total);
}

/// The selection highlight lit like a pressed glass key: a little brighter along its top, a
/// hairline catch on its upper rim, the rest as filled.
[[ stitchable ]] half4 pillHighlight(float2 pos, half4 color, float2 size) {
    if (color.a < 0.001h) { return color; }
    float r = size.y * 0.5;
    float2 c = float2(clamp(pos.x, r, max(size.x - r, r)), r);
    float inside = r - length(pos - c);
    float top = 1.0 - clamp(pos.y / max(size.y, 1.0), 0.0, 1.0);
    float rim = smoothstep(1.0, 0.1, inside) * smoothstep(0.35, 0.9, top);
    float lift = 1.0 + 0.35 * top * top;
    half a = color.a * half(lift) + half(rim * 0.10);
    return half4(color.rgb / max(color.a, 0.001h) * a, a);
}

/// A slow light passing across text while Scrawl processes (`phase` 0 → 1 per pass): a soft band,
/// a quarter of the width, that lifts the glyphs it crosses by at most 35%.
[[ stitchable ]] half4 pillShimmer(float2 pos, half4 color, float2 size, float phase) {
    if (color.a < 0.001h) { return color; }
    float x = pos.x / max(size.x, 1.0);
    float centre = -0.3 + 1.6 * phase;
    float d = (x - centre) / 0.14;
    float band = exp(-d * d);
    half k = half(0.62 + 0.38 * band);
    return half4(color.rgb * k, color.a * k);
}
