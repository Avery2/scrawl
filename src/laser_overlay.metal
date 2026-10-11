#include <metal_stdlib>
using namespace metal;

struct LaserUniforms {
    float2 resolution;
    float scale;
    float now_sec;
    float tail_duration;
    float tip_active;
    float2 tip_pos;
    uint segment_count;
    uint pin_count;
    float constrain_rect;
    float _pad1;
    float4 target_rect;
};

// Each segment connects A -> B in logical screen points.
// kind == 0.0: tapered laser segment (w0..w1 = combined arc + time weight 0..1)
// kind == 1.0: persistent ink segment (w0..w1 = simulated pressure scale)
struct PathSegment {
    float2 a;
    float2 b;
    float w0;
    float w1;
    float kind;
    float width;
};

struct PinMarker {
    float2 pos;
    float radius;
    float _pad;
};

struct VertexOutput {
    float4 position [[position]];
    float2 uv;
};

vertex VertexOutput laser_vertex(uint vertex_id [[vertex_id]]) {
    const float2 positions[] = {
        float2(-1.0, -1.0),
        float2( 3.0, -1.0),
        float2(-1.0,  3.0),
    };
    VertexOutput out;
    out.position = float4(positions[vertex_id], 0.0, 1.0);
    out.uv = positions[vertex_id] * float2(0.5, -0.5) + 0.5;
    return out;
}

fragment float4 laser_fragment(
    VertexOutput in [[stage_in]],
    constant LaserUniforms &u [[buffer(0)]],
    const device PathSegment *segments [[buffer(1)]],
    const device PinMarker *pins [[buffer(2)]]
) {
    // Pixel position in logical screen points (top-left origin).
    float2 p = (in.uv * u.resolution) / max(u.scale, 1.0);

    float3 laser_red = float3(1.0, 0.16, 0.19);
    float3 laser_hot = float3(1.0, 0.97, 0.97);

    float laser_core_cov = 0.0;
    float laser_body_cov = 0.0;
    float laser_glow_cov = 0.0;
    float ink_halo_cov = 0.0;
    float ink_red_cov = 0.0;

    uint count = min(u.segment_count, 1536u);
    for (uint i = 0; i < count; ++i) {
        PathSegment seg = segments[i];
        float2 ab = seg.b - seg.a;
        float len2 = max(dot(ab, ab), 0.0001);
        float h = clamp(dot(p - seg.a, ab) / len2, 0.0, 1.0);
        float2 closest = seg.a + ab * h;
        float d = length(p - closest);
        float w = clamp(mix(seg.w0, seg.w1, h), 0.0, 1.25);

        if (seg.kind < 0.5) {
            if (w <= 0.002) {
                continue;
            }
            float glow_sigma = max(1.4, 6.2 * (0.28 + 0.72 * w));
            if (d < glow_sigma * 3.0) {
                float body_r = 2.45 * pow(w, 0.68);
                float core_r = 0.90 * pow(w, 0.82);
                float body = smoothstep(body_r + 0.85, max(0.0, body_r - 0.85), d) * pow(w, 0.40);
                float core = smoothstep(core_r + 0.75, max(0.0, core_r - 0.75), d) * pow(w, 0.72);
                float glow = exp(-(d * d) / (glow_sigma * glow_sigma)) * pow(w, 0.92);
                laser_body_cov = max(laser_body_cov, body);
                laser_core_cov = max(laser_core_cov, core);
                laser_glow_cov = max(laser_glow_cov, glow);
            }
        } else {
            // Simple tldraw-style thin red marker stroke (~4.5px wide, smooth anti-aliased edge).
            float r = 2.25 * clamp(w, 0.55, 1.15);
            if (d < r + 2.0) {
                float fill = smoothstep(r + 0.70, max(0.0, r - 0.70), d);
                ink_red_cov = max(ink_red_cov, fill);
            }
        }
    }

    // Pin markers
    uint pcount = min(u.pin_count, 64u);
    for (uint i = 0; i < pcount; ++i) {
        PinMarker pin = pins[i];
        float d = length(p - pin.pos);
        if (d < pin.radius + 3.5) {
            float halo = smoothstep(pin.radius + 2.0, pin.radius + 0.8, d);
            float fill = smoothstep(pin.radius + 0.75, pin.radius - 0.75, d);
            ink_halo_cov = max(ink_halo_cov, halo);
            ink_red_cov = max(ink_red_cov, fill);
        }
    }

    // Subtle red marker dot at the cursor tip while Shift is held.
    if (u.tip_active > 0.01) {
        float d_tip = length(p - u.tip_pos);
        if (d_tip < 5.0) {
            float tip_fill = smoothstep(2.6, 1.4, d_tip) * u.tip_active;
            ink_red_cov = max(ink_red_cov, tip_fill);
        }
    }

    float4 out = float4(0.0);

    // 0. If constrained to a window or area, softly dim outside the capture box and draw a 1.5px border.
    if (u.constrain_rect > 0.5) {
        float2 box_min = u.target_rect.xy;
        float2 box_max = u.target_rect.xy + u.target_rect.zw;
        float2 box_center = (box_min + box_max) * 0.5;
        float2 half_size = (box_max - box_min) * 0.5;
        float2 q = abs(p - box_center) - half_size;
        float sdf = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
        if (sdf > 0.0) {
            float dim_a = smoothstep(0.0, 1.5, sdf) * 0.28;
            out.rgb = float3(0.0);
            out.a = dim_a;
            float border_a = (1.0 - smoothstep(0.2, 1.8, abs(sdf - 1.0))) * 0.55;
            if (border_a > 0.001) {
                float3 border_col = float3(0.35, 0.52, 1.0);
                out.rgb = border_col * border_a + out.rgb * (1.0 - border_a);
                out.a = border_a + out.a * (1.0 - border_a);
            }
        }
    }

    // 1. Composite persistent ink (white halo + red fill)
    if (ink_halo_cov > 0.001) {
        float a = ink_halo_cov * 0.82;
        out.rgb = float3(1.0) * a + out.rgb * (1.0 - a);
        out.a = a + out.a * (1.0 - a);
    }
    if (ink_red_cov > 0.001) {
        float3 ink_col = float3(1.0, 0.23, 0.19);
        float a = ink_red_cov;
        out.rgb = ink_col * a + out.rgb * (1.0 - a);
        out.a = a + out.a * (1.0 - a);
    }

    // 2. Composite optical laser (soft atmospheric bloom + anti-aliased ruby ribbon + white-hot core)
    float laser_alpha = clamp(laser_glow_cov * 0.42 + laser_body_cov * 0.88 + laser_core_cov * 0.95, 0.0, 1.0);
    if (laser_alpha > 0.002) {
        float3 col = mix(laser_red, laser_hot, clamp(laser_core_cov * 0.92, 0.0, 1.0));
        out.rgb = col * laser_alpha + out.rgb * (1.0 - laser_alpha);
        out.a = laser_alpha + out.a * (1.0 - laser_alpha);
    }

    return float4(out.rgb * out.a, out.a);
}
