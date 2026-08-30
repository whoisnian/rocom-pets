//! 相机与取景:正交视图、取景半径、轨道旋转/视图,以及俯仰的上限。
//!
//! 桌宠(正交、铺满画布)与网页预览(轨道相机)共用这一组;
//! 离屏渲染的取景盒是「各动作姿势的并集」,判据见 `orthographic_view` 的注释。

use super::*;

/// 正交相机:桌宠是贴在桌面上的小人,透视没有意义,正交还免了远近缩放的麻烦。
///
/// `bounds` 是**绑定姿势**的包围盒,`yaw` 是绕 Y 轴的观察角(0 = 从 +Z 看;宠物朝 +Z,故 0 是正面)。
/// `padding` 要留出余量:跳跃/伸展类动作会超出绑定姿势的包围盒(实测 Happy 会高出一截)。
///
/// 桌宠只绕 Y 转、画布也一定是正方的,所以这里没有俯仰与宽高比;
/// 网页预览要拖着看,走 [`orbit_view`]。
pub fn orthographic_view(bounds: (Vec3, Vec3), yaw: f32, padding: f32) -> Mat4 {
    orbit_view(bounds, yaw, 0.0, padding, 1.0, Vec3::ZERO)
}

/// 取景半径:包围盒最长边的一半,乘上余量。
///
/// 取最长边而不是对角线:对角线会把瘦高的模型框得过松,宠物在画面里缩成一小团。
/// 单独提出来是因为网页预览要拿它换算「拖一像素等于世界里多远」——**正交投影下
/// 画面高度正好是 `2 * radius`**,两处各写一遍迟早对不上。
pub fn framing_radius(bounds: (Vec3, Vec3), padding: f32) -> f32 {
    let extent = bounds.1 - bounds.0;
    extent.x.max(extent.y).max(extent.z) * 0.5 * padding
}

/// 观察角 → 相机朝向。`pitch` 在这里夹紧,调用方不必自己管。
pub fn orbit_rotation(yaw: f32, pitch: f32) -> glam::Quat {
    glam::Quat::from_rotation_y(yaw)
        * glam::Quat::from_rotation_x(pitch.clamp(-MAX_PITCH, MAX_PITCH))
}

/// 同上,外加**俯仰**与**画布宽高比** —— 网页预览那块 canvas 可以拖、也不一定是正方的。
///
/// `pitch` 正值是从上往下看。**夹在 ±80° 内**:到极点时 `look_at` 的上方向会和视线共线,
/// 矩阵直接退化成一片空白。宽高比只放宽横向,竖向那半径不动,于是不论画布多宽,
/// 宠物在画面里的**高度**是一样的 —— 拖窗口大小时它不会跟着忽大忽小。
///
/// `target` 是**世界坐标里的**轨道中心偏移(网页预览的平移)。存世界坐标而不是屏幕偏移,
/// 是因为平移完再转视角时,被推到一边的宠物应当待在原地,而不是跟着镜头甩。
pub fn orbit_view(
    bounds: (Vec3, Vec3),
    yaw: f32,
    pitch: f32,
    padding: f32,
    aspect: f32,
    target: Vec3,
) -> Mat4 {
    let (min, max) = bounds;
    let center = (min + max) * 0.5 + target;
    let radius = framing_radius(bounds, padding);
    let rotation = orbit_rotation(yaw, pitch);
    let eye = center + rotation * Vec3::new(0.0, 0.0, radius * 2.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, center, Vec3::Y);
    let half_w = radius * aspect.max(0.01);
    // 深度范围用 wgpu 的 0..1(DirectX 约定),与管线的 Depth32Float + CompareFunction::Less 匹配
    let proj = glam::camera::rh::proj::directx::orthographic(
        -half_w,
        half_w,
        -radius,
        radius,
        0.01,
        // 俯仰会把相机推到包围盒的角上,近/远平面要按对角线留够,不然会削掉一块
        radius * 6.0,
    );
    proj * view
}

/// 俯仰的上限(弧度)。差 10° 到极点就停 —— 再上去 `look_at` 就退化了。
pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 * 8.0 / 9.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺卡要退档,而且不能退成「一张都不画」。
    #[test]
    fn a_missing_face_card_falls_back_instead_of_vanishing() {
        let full: Vec<u32> = (1..=8).collect();
        assert_eq!(resolve_face_card(&full, 5), 5, "有就用它");
        // 蝴蝶陶陶三阶缺 5 号(困倦):退回默认那张
        let no_sleepy = [1, 2, 3, 4, 6, 7, 8];
        assert_eq!(resolve_face_card(&no_sleepy, 5), 2);
        // 觅觅蝠一阶连 1 号都没有,但 2 号在,默认脸照样有
        let no_first = [2, 3, 4, 5, 6, 7, 8];
        assert_eq!(resolve_face_card(&no_first, 2), 2);
        // 连 2 号都没有的极端情况:退到最小的一张,而不是什么都不画
        assert_eq!(resolve_face_card(&[3, 6], 5), 3);
        // 不是网格脸:原样返回(着色器不看这个值)
        assert_eq!(resolve_face_card(&[], 2), 2);
    }

    fn skinned_vertex(pos: [f32; 3], joint: u16) -> Vertex {
        Vertex {
            pos,
            normal: [0.0, 1.0, 0.0],
            uv: [0.0; 2],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
            local_pos: pos,
            color: [1.0; 4],
            uv1: [0.0; 2],
            uv2: [0.0; 2],
        }
    }

    /// 拖视角那两条约束:**俯仰要夹住**(到极点 `look_at` 会退化成一片空白),
    /// 而**宽高比只放宽横向** —— 不论画布多宽,宠物在画面里的高度不变。
    #[test]
    fn orbit_clamps_pitch_and_only_widens_horizontally() {
        let bounds = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        // 竖直方向的投影比例不受宽高比影响
        let square = orbit_view(bounds, 0.0, 0.0, 1.0, 1.0, Vec3::ZERO);
        let wide = orbit_view(bounds, 0.0, 0.0, 1.0, 2.0, Vec3::ZERO);
        assert!((square.y_axis.y - wide.y_axis.y).abs() < 1e-6, "高度该一样");
        assert!(wide.x_axis.x.abs() < square.x_axis.x.abs(), "横向该放宽");

        // 俯仰给到超过 90° 也不能让矩阵烂掉(NaN / 全零)
        let over = orbit_view(bounds, 0.3, 3.0, 1.0, 1.5, Vec3::ZERO);
        assert!(over.to_cols_array().iter().all(|v| v.is_finite()));
        assert_eq!(
            over,
            orbit_view(bounds, 0.3, MAX_PITCH, 1.0, 1.5, Vec3::ZERO),
            "该夹到上限"
        );

        // 不给俯仰与宽高比时,就是原来那个正方取景
        assert_eq!(
            orthographic_view(bounds, 0.7, 1.15),
            orbit_view(bounds, 0.7, 0.0, 1.15, 1.0, Vec3::ZERO)
        );
    }

    #[test]
    fn posed_bounds_follow_skin_matrices() {
        let vertices = [
            skinned_vertex([-1.0, -2.0, -3.0], 0),
            skinned_vertex([1.0, 2.0, 3.0], 1),
        ];
        let matrices = [
            Mat4::from_translation(Vec3::new(2.0, 3.0, 4.0)),
            Mat4::from_translation(Vec3::new(-2.0, -1.0, 0.0)),
        ];

        assert_eq!(
            posed_object_bounds(&vertices, &matrices),
            Some([0.0, 1.0, 2.0, 2.0])
        );
        assert_eq!(posed_object_bounds(&[], &matrices), None);
    }

    /// 网页预览的缩放没有动相机,而是把取景余量按比例收紧(`web.rs` 里传的是
    /// `PADDING / zoom`)—— 投影是正交的,这么做和「拉近」等价。这条测试钉住那个比例:
    /// 余量减半,同一个点在裁剪空间里就该走到大约两倍远。
    #[test]
    fn tightening_the_padding_makes_the_pet_fill_more_of_the_frame() {
        let bounds = (Vec3::splat(-1.0), Vec3::splat(1.0));
        let ndc_y = |padding: f32| {
            let clip = orbit_view(bounds, 0.0, 0.0, padding, 1.0, Vec3::ZERO)
                * Vec3::new(0.0, 1.0, 0.0).extend(1.0);
            clip.y / clip.w
        };

        let wide = ndc_y(1.15);
        let tight = ndc_y(1.15 / 2.0);
        assert!(
            (tight / wide - 2.0).abs() < 0.01,
            "余量减半应当正好等于放大两倍,实得 {wide} → {tight}"
        );
    }

    /// 平移要**精确跟手**:把轨道中心沿屏幕上方推「一个画面高」(正交下就是 `2 * radius`),
    /// 原来在正中的那个点就该正好落到画面下边缘 —— NDC 里走 2.0。差一点都会表现成
    /// 「拖得比手快 / 比手慢」,而这正是 web.rs 里 `pan` 那个换算的依据。
    #[test]
    fn panning_one_screen_height_moves_the_subject_exactly_one_screen() {
        let bounds = (Vec3::splat(-1.0), Vec3::splat(1.0));
        let padding = 1.15;
        let radius = framing_radius(bounds, padding);
        let ndc_y = |target: Vec3| {
            let clip = orbit_view(bounds, 0.0, 0.0, padding, 1.0, target) * Vec3::ZERO.extend(1.0);
            clip.y / clip.w
        };

        assert!(ndc_y(Vec3::ZERO).abs() < 1e-6, "没平移时中心就在画面正中");
        let one_screen = ndc_y(Vec3::Y * 2.0 * radius);
        assert!(
            (one_screen + 2.0).abs() < 1e-5,
            "中心上移一个画面高,画面里那个点就该反向走过整整一屏(NDC 满程 2.0),实得 {one_screen}"
        );
    }

    /// **两只身高差 5 倍的宠物,桌面上的描边像素数该一样。**
    ///
    /// 导出器写的 `outline_width` 正比于身高(莫比乌乌 27.6cm → 0.0007 米、
    /// 克莱因龙 138cm → 0.0035 米),而桌宠的窗口也正比于身高 ⇒ 不修正的话描边像素
    /// 差 5 倍(用户实测「宠物体型越大描边越明显」)。乘上这个倍率之后两者应重合。
    #[test]
    fn desktop_outline_is_the_same_width_for_a_small_and_a_large_pet() {
        let pet = |h: f32| (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, h, 1.0));
        // 导出器那一侧:宽度 = ratio × 身高(见 exporter/Materials.cs `OutlineOf`)
        let exported = |h: f32| 0.0196 * 0.13 * h;
        let on_screen = |h: f32| exported(h) * desktop_outline_scale(pet(h));

        let small = on_screen(0.276); // 莫比乌乌
        let large = on_screen(1.380); // 克莱因龙
        assert!(
            (small - large).abs() < 1e-9,
            "修正后两只该等宽,实得 {small} vs {large}"
        );
        // 中位身高那只宽度不变 —— 离屏那批基线数字才不会跟着动
        let median = 1.173;
        assert!((desktop_outline_scale(pet(median)) - 1.0).abs() < 1e-3);
    }

    /// 包围盒退化(资产坏了)时不缩放,免得除出一个巨大的倍率。
    #[test]
    fn desktop_outline_scale_ignores_a_degenerate_bounding_box() {
        let flat = (Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0));
        assert_eq!(desktop_outline_scale(flat), 1.0);
    }

}
