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
    uint _pad0;
    uint _pad1;
};

// Each segment connects A -> B in logical screen points.
// kind == 0.0: time-decaying laser segment (t0_sec..t1_sec)
// kind == 1.0: persistent ink segment
struct PathSegment {
    float2 a;
    float2 b;
    float t0_sec;
    float t1_sec;
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

    float3 laser_red = float3(1.0, 0.14, 0.18);
    float3 laser_hot = float3(1.0, 0.96, 0.96);

    float laser_core_energy = 0.0;
    float laser_glow_energy = 0.0;
    float ink_halo_cov = 0.0;
    float ink_red_cov = 0.0;

    uint count = min(u.segment_count, 1024u);
    for (uint i = 0; i < count; ++i) {
        PathSegment seg = segments[i];
        float2 ab = seg.b - seg.a;
        float len2 = max(dot(ab, ab), 0.0001);
        float h = clamp(dot(p - seg.a, ab) / len2, 0.0, 1.0);
        float2 closest = seg.a + ab * h;
        float d = length(p - closest);

        if (seg.kind < 0.5) {
            // Time-based laser segment: each point along the segment has its own birth time.
            // As `now_sec` advances, the tail smoothly retracts into the tip.
            float t_pt = mix(seg.t0_sec, seg.t1_sec, h);
            float age = max(0.0, u.now_sec - t_pt);
            if (age >= u.tail_duration) {
                continue;
            }
            float life = smoothstep(0.0, 1.0, 1.0 - age / max(u.tail_duration, 0.01));
            // Taper both width and intensity smoothly toward the vanishing tail.
            float core_sigma = max(0.45, 1.25 * pow(life, 0.65));
            float glow_sigma = max(1.8, 6.2 * (0.35 + 0.65 * life));

            if (d < glow_sigma * 3.2) {
                float core = exp(-(d * d) / (core_sigma * core_sigma)) * pow(life, 1.25);
                float glow = exp(-(d * d) / (glow_sigma * glow_sigma)) * pow(life, 1.05);
                laser_core_energy = max(laser_core_energy, core);
                laser_glow_energy = max(laser_glow_energy, glow);
            }
        } else {
            // Persistent ink stroke: crisp white halo + red stroke.
            float r = seg.width * 0.5;
            if (d < r + 3.5) {
                float halo = smoothstep(r + 2.2, r + 1.0, d);
                float fill = smoothstep(r + 0.6, r - 0.6, d);
                ink_halo_cov = max(ink_halo_cov, halo);
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
            float halo = smoothstep(pin.radius + 2.4, pin.radius + 1.2, d);
            float fill = smoothstep(pin.radius + 0.6, pin.radius - 0.6, d);
            ink_halo_cov = max(ink_halo_cov, halo);
            ink_red_cov = max(ink_red_cov, fill);
        }
    }

    // Active laser diode pinpoint at the cursor tip.
    if (u.tip_active > 0.01) {
        float d_tip = length(p - u.tip_pos);
        if (d_tip < 24.0) {
            float pulse = 0.95 + 0.05 * sin(u.now_sec * 28.0);
            float tip_core = exp(-(d_tip * d_tip) / (1.75 * 1.75)) * u.tip_active;
            float tip_glow = exp(-(d_tip * d_tip) / (6.8 * 6.8 * pulse)) * u.tip_active * 1.15;
            laser_core_energy = max(laser_core_energy, tip_core);
            laser_glow_energy = max(laser_glow_energy, tip_glow);
        }
    }

    float4 out = float4(0.0);

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

    // 2. Composite optical laser (subtle ruby glow + incandescent white-pink core)
    float laser_alpha = clamp(laser_glow_energy * 0.58 + laser_core_energy * 0.92, 0.0, 1.0);
    if (laser_alpha > 0.002) {
        float3 col = mix(laser_red, laser_hot, clamp(pow(laser_core_energy, 1.35), 0.0, 1.0));
        // Premultiplied output for CAMetalLayer with non-opaque background
        out.rgb = col * laser_alpha + out.rgb * (1.0 - laser_alpha);
        out.a = laser_alpha + out.a * (1.0 - laser_alpha);
    }

    return float4(out.rgb * out.a, out.a);
}
