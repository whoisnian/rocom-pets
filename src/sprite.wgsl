// 把精灵贴图画成一个四边形。顶点由 vertex_index 生成,不用顶点缓冲。
// 颜色保持预乘 alpha:片元里做任何调整都必须维持 rgb <= a 的不变式。

struct U {
    surface: vec2<f32>,   // 表面尺寸(像素)
    pos: vec2<f32>,       // 精灵左上角(像素)
    size: vec2<f32>,      // 精灵尺寸(像素)
    highlight: f32,       // >0.5 提亮(拖动中)
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var sprite_tex: texture_2d<f32>;
@group(0) @binding(2) var sprite_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    // TriangleStrip 的四个角:(0,0) (1,0) (0,1) (1,1)
    let corner = vec2<f32>(f32(idx & 1u), f32((idx >> 1u) & 1u));
    let px = u.pos + corner * u.size;
    // 像素坐标 → NDC(y 轴翻转)
    let ndc = vec2<f32>(px.x / u.surface.x * 2.0 - 1.0, 1.0 - px.y / u.surface.y * 2.0);

    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(sprite_tex, sprite_smp, in.uv);
    if u.highlight > 0.5 {
        // 提亮但不破坏预乘:上限是 alpha
        c = vec4<f32>(min(c.rgb + vec3<f32>(0.18) * c.a, vec3<f32>(c.a)), c.a);
    }
    return c;
}
