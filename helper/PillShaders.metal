// SwiftUI shaders for the SwiftUI capsule (`helper/Pill.swift`), applied with `.colorEffect` and
// `.layerEffect` and driven by the same animation values as the shape. `build.rs` compiles this
// into a metallib and embeds it in scrawl-helper (`PillShaderData.swift`).

#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

/// A soft glow inside the capsule's edge, a little stronger at the top, that swells with
/// `energy` while the shape moves and is gone at rest (the hairline itself is a SwiftUI stroke).
[[ stitchable ]] half4 pillEdgeGlow(float2 pos, half4 color, float2 size, float energy) {
    if (color.a < 0.001h || energy < 0.001) { return color; }
    float r = size.y * 0.5;
    float2 c = float2(clamp(pos.x, r, max(size.x - r, r)), r);
    float inside = max(r - length(pos - c), 0.0);
    float top = 1.0 - clamp(pos.y / max(size.y, 1.0), 0.0, 1.0);
    float g = inside / 6.0;
    half add = half(exp(-g * g) * 0.07 * energy * (0.55 + 0.45 * top)) * color.a;
    return half4(min(color.rgb + add, half3(color.a)), color.a);
}

/// The recording dot, as the Metal capsule draws it, plus a soft glow: one 1.6 s `phase` drives
/// a halo disc that grows from 4 to 10 pt as it fades, and the 8 pt core breathing .78 → 1
/// with a warm glow around it that swells as it brightens.
[[ stitchable ]] half4 pillDot(float2 pos, half4 color, float2 size, float phase) {
    float d = length(pos - size * 0.5);
    float breath = 0.5 + 0.5 * cos(6.2831853 * phase);
    float core = smoothstep(4.3, 3.7, d);
    float haloR = 4.0 + 6.0 * phase;
    float halo = smoothstep(haloR + 0.4, haloR - 0.4, d) * 0.34 * (1.0 - phase) * (1.0 - phase);
    float g = max(d - 3.5, 0.0) / (2.6 + 1.0 * breath);
    float glow = exp(-g * g) * (0.20 + 0.16 * breath);
    float around = 1.0 - (1.0 - halo) * (1.0 - glow);
    float a = clamp(core * (0.78 + 0.22 * breath) + (1.0 - core) * around, 0.0, 1.0);
    half3 red = half3(0.941h, 0.267h, 0.220h);
    return half4(red * half(a), half(a)) * color.a;
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
