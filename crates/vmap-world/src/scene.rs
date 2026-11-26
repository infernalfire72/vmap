//! A renderer-independent description of something to draw: textures,
//! materials, models and where each model is placed.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use vmap_formats::nitro::anim::{JointAnimation, TexSrtTrack};
use vmap_formats::nitro::{evaluate, GeoVertex, Image, Model};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wrap {
    Clamp,
    Repeat,
    Mirror,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cull {
    None,
    Back,
    Front,
    /// Neither side is drawn.
    Both,
}

#[derive(Debug, Clone)]
pub struct SceneTexture {
    pub name: String,
    pub image: Image,
}

#[derive(Debug, Clone)]
pub struct SceneMaterial {
    pub name: String,
    pub texture: Option<usize>,
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub specular: [f32; 3],
    pub emission: [f32; 3],
    /// Polygon alpha, 0..=1.
    pub alpha: f32,
    /// Enabled hardware lights (bit 0 = light 0).
    pub lights: u32,
    pub cull: Cull,
    pub wrap_s: Wrap,
    pub wrap_t: Wrap,
    /// Rows of a 2x3 matrix applied to texture coordinates.
    pub tex_matrix: [[f32; 3]; 2],
    /// Needs alpha blending (drawn after everything opaque).
    pub translucent: bool,
}

#[derive(Debug, Clone)]
pub struct SceneBatch {
    /// Index into [`Scene::materials`].
    pub material: usize,
    /// Triangle list in model space.
    pub vertices: Vec<GeoVertex>,
}

#[derive(Debug, Clone)]
pub struct SceneModel {
    pub name: String,
    pub batches: Vec<SceneBatch>,
    pub min: Vec3,
    pub max: Vec3,
}

impl SceneModel {
    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.vertices.len() / 3).sum()
    }
}

/// Frames per second the games play field animations at.
pub const ANIMATION_FPS: f32 = 30.0;

/// Frame of a looping animation at a time in seconds.
pub fn loop_frame(time: f32, frames: f32) -> f32 {
    if frames > 0.0 { (time.max(0.0) * ANIMATION_FPS) % frames } else { 0.0 }
}

/// Texture placement matrix in normalised, top-left-origin coordinates.
/// Scale and translation follow the Maya convention the games' exporter used
/// (V measured upwards); rotation turns about the texture centre.
pub fn tex_matrix(scale: [f32; 2], sin: f32, cos: f32, translation: [f32; 2]) -> [[f32; 3]; 2] {
    let ([ss, st], [ts, tt]) = (scale, translation);
    // In V-up coordinates: P' = S * (R * (P - 0.5) + 0.5 - T). Rewritten here
    // for V measured downwards from the top-left corner.
    [
        [ss * cos, ss * sin, ss * (0.5 - 0.5 * cos - 0.5 * sin - ts)],
        [-st * sin, st * cos, 1.0 - st * (0.5 - 0.5 * sin + 0.5 * cos - tt)],
    ]
}

/// A material whose texture slides, turns or stretches over time.
#[derive(Debug, Clone)]
pub struct TexSrtAnim {
    pub material: usize,
    /// Length of the loop in frames.
    pub frames: f32,
    pub track: TexSrtTrack,
}

impl TexSrtAnim {
    pub fn matrix(&self, time: f32) -> [[f32; 3]; 2] {
        let f = loop_frame(time, self.frames);
        let t = &self.track;
        tex_matrix(
            [t.scale_s.sample(f), t.scale_t.sample(f)],
            t.rot_sin.sample(f),
            t.rot_cos.sample(f),
            [t.trans_s.sample(f), t.trans_t.sample(f)],
        )
    }
}

/// A material that switches between textures over time.
#[derive(Debug, Clone)]
pub struct TexPatternAnim {
    pub material: usize,
    /// Length of the loop in frames.
    pub frames: f32,
    /// (first frame, scene texture), in frame order.
    pub keys: Vec<(f32, usize)>,
}

impl TexPatternAnim {
    /// Index into `keys` of the texture shown at a time.
    pub fn key_at(&self, time: f32) -> usize {
        let f = loop_frame(time, self.frames);
        self.keys.iter().rposition(|k| k.0 <= f).unwrap_or(0)
    }
}

/// A model whose joints move over time.
#[derive(Debug, Clone)]
pub struct JointAnim {
    pub model: usize,
    /// Length of the loop in frames.
    pub frames: f32,
    pub source: Arc<Model>,
    pub animation: Arc<JointAnimation>,
}

impl JointAnim {
    /// Vertices of every batch of the scene model at a time, in the order of
    /// [`SceneModel::batches`].
    pub fn vertices(&self, time: f32) -> Vec<Vec<GeoVertex>> {
        let pose = self.animation.pose(&self.source, loop_frame(time, self.frames));
        evaluate(&self.source, Some(&pose))
            .batches
            .into_iter()
            .filter(|b| !b.vertices.is_empty())
            .map(|b| b.vertices)
            .collect()
    }
}

/// Everything in a scene that moves by itself.
#[derive(Debug, Clone, Default)]
pub struct Animations {
    pub tex_srt: Vec<TexSrtAnim>,
    pub tex_pattern: Vec<TexPatternAnim>,
    pub joints: Vec<JointAnim>,
}

impl Animations {
    pub fn is_empty(&self) -> bool {
        self.tex_srt.is_empty() && self.tex_pattern.is_empty() && self.joints.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tex_srt.len() + self.tex_pattern.len() + self.joints.len()
    }
}

/// What a placed model represents, for picking and labels.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Chunk { x: usize, y: usize, chunk_id: u32, zone: Option<u32> },
    Building { uid: u16, zone: Option<u32> },
    Door { uid: u16 },
    Model,
}

#[derive(Debug, Clone)]
pub struct SceneNode {
    pub model: usize,
    pub transform: Mat4,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkerKind {
    Npc,
    Warp,
    Trigger,
    Furniture,
}

impl MarkerKind {
    pub const ALL: [MarkerKind; 4] = [Self::Npc, Self::Warp, Self::Trigger, Self::Furniture];

    /// Position in [`Self::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Npc => "NPC",
            Self::Warp => "Warp",
            Self::Trigger => "Trigger",
            Self::Furniture => "Interactable",
        }
    }
}

/// A point of interest placed in the world (an NPC, a warp, ...).
#[derive(Debug, Clone)]
pub struct Marker {
    pub kind: MarkerKind,
    pub pos: Vec3,
    /// Footprint in world units (x, z).
    pub size: [f32; 2],
    pub zone: u32,
    /// Index within its list in the zone's entity file.
    pub index: usize,
    pub label: String,
    /// For warps: the zone and warp they lead to.
    pub target: Option<(u16, u16)>,
}

impl Marker {
    /// Box drawn (and picked) for this marker: (minimum corner, size).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (sx, sz) = (self.size[0].max(4.0), self.size[1].max(4.0));
        let (height, shrink, centred) = match self.kind {
            MarkerKind::Npc => (24.0, 0.7, true),
            MarkerKind::Furniture => (12.0, 0.7, true),
            MarkerKind::Warp => (20.0, 1.0, false),
            MarkerKind::Trigger => (6.0, 1.0, false),
        };
        // NPCs and interactables sit at the centre of their tile; warps and
        // triggers are anchored at the corner of their footprint.
        let centre = if centred { self.pos } else { self.pos + Vec3::new(sx * 0.5, 0.0, sz * 0.5) };
        let size = Vec3::new(sx * shrink, height, sz * shrink);
        (centre - Vec3::new(size.x * 0.5, 0.0, size.z * 0.5), size)
    }

    /// Distance along a ray to this marker's box, if the ray hits it.
    pub fn ray_hit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let (min, size) = self.bounds();
        let max = min + size;
        let (mut t0, mut t1) = (0.0f32, f32::MAX);
        for a in 0..3 {
            if dir[a].abs() < 1e-8 {
                if origin[a] < min[a] || origin[a] > max[a] {
                    return None;
                }
                continue;
            }
            let (mut n, mut f) = ((min[a] - origin[a]) / dir[a], (max[a] - origin[a]) / dir[a]);
            if n > f {
                std::mem::swap(&mut n, &mut f);
            }
            t0 = t0.max(n);
            t1 = t1.min(f);
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Light {
    pub enabled: bool,
    /// Direction the light travels in.
    pub direction: Vec3,
    pub color: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct Lighting {
    pub lights: [Light; 4],
    pub clear_color: [f32; 3],
}

impl Default for Lighting {
    fn default() -> Self {
        let off = Light { enabled: false, direction: Vec3::NEG_Y, color: [0.0; 3] };
        Self {
            lights: [
                Light { enabled: true, direction: Vec3::new(-0.35, -0.85, -0.4).normalize(), color: [1.0; 3] },
                Light { enabled: true, direction: Vec3::new(0.4, -0.6, 0.7).normalize(), color: [0.45; 3] },
                off,
                off,
            ],
            clear_color: [0.42, 0.62, 0.85],
        }
    }
}

/// One cell of the map grid a scene was built from.
#[derive(Debug, Clone, Copy)]
pub struct MapCell {
    pub x: usize,
    pub y: usize,
    pub chunk_id: u32,
    pub zone: Option<u32>,
    /// The cell's zone was hidden, so nothing was built for it.
    pub hidden: bool,
}

/// Layout of a stitched map, for the 2D overview.
#[derive(Debug, Clone, Default)]
pub struct MapLayout {
    pub matrix_id: usize,
    pub width: usize,
    pub height: usize,
    pub chunk_span: f32,
    pub cells: Vec<MapCell>,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub textures: Vec<SceneTexture>,
    pub materials: Vec<SceneMaterial>,
    pub models: Vec<SceneModel>,
    pub nodes: Vec<SceneNode>,
    pub markers: Vec<Marker>,
    pub lighting: Lighting,
    pub animations: Animations,
    pub layout: Option<MapLayout>,
    pub min: Vec3,
    pub max: Vec3,
    /// Problems met while building (missing files, unparsable models).
    pub warnings: Vec<String>,
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.nodes.iter().map(|n| self.models[n.model].triangle_count()).sum()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn radius(&self) -> f32 {
        ((self.max - self.min).length() * 0.5).max(1.0)
    }

    /// Recomputes the bounding box from the placed models.
    pub fn update_bounds(&mut self) {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for n in &self.nodes {
            let m = &self.models[n.model];
            if m.batches.is_empty() {
                continue;
            }
            for i in 0..8 {
                let c = Vec3::new(
                    if i & 1 == 0 { m.min.x } else { m.max.x },
                    if i & 2 == 0 { m.min.y } else { m.max.y },
                    if i & 4 == 0 { m.min.z } else { m.max.z },
                );
                let p = n.transform.transform_point3(c);
                min = min.min(p);
                max = max.max(p);
            }
        }
        if min.x > max.x {
            min = Vec3::ZERO;
            max = Vec3::ZERO;
        }
        self.min = min;
        self.max = max;
    }
}

/// Where a ray first meets a placed model.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    /// Distance from the ray's origin, in units of the direction's length.
    pub distance: f32,
    pub point: Vec3,
    /// Index into [`Scene::nodes`].
    pub node: usize,
}

impl Scene {
    /// The first surface of a placed model that a ray meets, as far as it
    /// is drawn solid: faces turned away are skipped where the material
    /// culls them, and so are see-through materials and the transparent
    /// texels of cut-out textures. Models are taken in their rest pose.
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        for (index, node) in self.nodes.iter().enumerate() {
            let model = &self.models[node.model];
            if model.batches.is_empty() {
                continue;
            }
            // The transform is affine, so distances along the ray carry over.
            let inverse = node.transform.inverse();
            let o = inverse.transform_point3(origin);
            let d = inverse.transform_vector3(dir);
            let limit = best.map_or(f32::MAX, |b| b.distance);
            if !ray_meets_box(o, d, model.min, model.max, limit) {
                continue;
            }
            for batch in &model.batches {
                let material = self.materials.get(batch.material);
                if material.is_some_and(|m| m.translucent || m.cull == Cull::Both) {
                    continue;
                }
                let cull = material.map_or(Cull::None, |m| m.cull);
                for tri in batch.vertices.chunks_exact(3) {
                    let Some(hit) = ray_triangle(o, d, tri[0].pos.into(), tri[1].pos.into(), tri[2].pos.into()) else {
                        continue;
                    };
                    if hit.t >= best.map_or(f32::MAX, |b| b.distance)
                        || (cull == Cull::Back && !hit.front)
                        || (cull == Cull::Front && hit.front)
                    {
                        continue;
                    }
                    if let Some(material) = material {
                        let w = 1.0 - hit.u - hit.v;
                        let uv = [
                            tri[0].uv[0] * w + tri[1].uv[0] * hit.u + tri[2].uv[0] * hit.v,
                            tri[0].uv[1] * w + tri[1].uv[1] * hit.u + tri[2].uv[1] * hit.v,
                        ];
                        if self.texel_alpha(material, uv) < 128 {
                            continue;
                        }
                    }
                    best = Some(RayHit { distance: hit.t, point: origin + dir * hit.t, node: index });
                }
            }
        }
        best
    }

    /// Opacity (0 to 255) of a material's texture at a texture coordinate.
    fn texel_alpha(&self, material: &SceneMaterial, uv: [f32; 2]) -> u8 {
        let Some(image) = material.texture.and_then(|t| self.textures.get(t)).map(|t| &t.image) else {
            return 255;
        };
        let m = material.tex_matrix;
        let s = m[0][0] * uv[0] + m[0][1] * uv[1] + m[0][2];
        let t = m[1][0] * uv[0] + m[1][1] * uv[1] + m[1][2];
        let texel = |v: f32, wrap: Wrap, size: u32| -> u32 {
            let v = match wrap {
                Wrap::Clamp => v.clamp(0.0, 1.0),
                Wrap::Repeat => v.rem_euclid(1.0),
                Wrap::Mirror => {
                    let m = v.rem_euclid(2.0);
                    if m > 1.0 { 2.0 - m } else { m }
                }
            };
            ((v * size as f32) as u32).min(size.saturating_sub(1))
        };
        let (x, y) = (texel(s, material.wrap_s, image.width), texel(t, material.wrap_t, image.height));
        image.rgba.get(((y * image.width + x) * 4 + 3) as usize).copied().unwrap_or(255)
    }
}

fn ray_meets_box(o: Vec3, d: Vec3, min: Vec3, max: Vec3, limit: f32) -> bool {
    let (mut t0, mut t1) = (0.0f32, limit);
    for a in 0..3 {
        if d[a].abs() < 1e-8 {
            if o[a] < min[a] || o[a] > max[a] {
                return false;
            }
            continue;
        }
        let (mut near, mut far) = ((min[a] - o[a]) / d[a], (max[a] - o[a]) / d[a]);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

struct TriangleHit {
    /// Distance along the ray.
    t: f32,
    /// Weights of the second and third corner at the point hit.
    u: f32,
    v: f32,
    /// The ray meets the side that is drawn when back faces are culled
    /// (corners running anticlockwise as seen from the ray's origin).
    front: bool,
}

fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<TriangleHit> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-4).then_some(TriangleHit { t, u, v, front: det > 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_finds_the_nearest_placed_model() {
        let vertex = |x: f32, z: f32| GeoVertex { pos: [x, 0.0, z], normal: [0.0, 1.0, 0.0], color: [1.0; 3], uv: [0.0; 2] };
        // A square on the ground, 10 units wide, as two triangles.
        let vertices = vec![
            vertex(-5.0, -5.0),
            vertex(5.0, -5.0),
            vertex(5.0, 5.0),
            vertex(-5.0, -5.0),
            vertex(5.0, 5.0),
            vertex(-5.0, 5.0),
        ];
        let mut scene = Scene::default();
        scene.models.push(SceneModel {
            name: "square".into(),
            batches: vec![SceneBatch { material: 0, vertices }],
            min: Vec3::new(-5.0, 0.0, -5.0),
            max: Vec3::new(5.0, 0.0, 5.0),
        });
        // One at the origin, one lifted by 20 and moved 100 to the east.
        scene.nodes.push(SceneNode { model: 0, transform: Mat4::IDENTITY, kind: NodeKind::Model });
        scene.nodes.push(SceneNode {
            model: 0,
            transform: Mat4::from_translation(Vec3::new(100.0, 20.0, 0.0)),
            kind: NodeKind::Model,
        });
        let down = Vec3::NEG_Y;
        let hit = scene.ray_cast(Vec3::new(1.0, 50.0, 2.0), down).expect("the first square is below");
        assert_eq!(hit.node, 0);
        assert!((hit.distance - 50.0).abs() < 1e-3);
        assert!((hit.point - Vec3::new(1.0, 0.0, 2.0)).length() < 1e-3);
        let hit = scene.ray_cast(Vec3::new(101.0, 50.0, 0.0), down).expect("the second square is below");
        assert_eq!(hit.node, 1);
        assert!((hit.distance - 30.0).abs() < 1e-3);
        assert!(scene.ray_cast(Vec3::new(50.0, 50.0, 0.0), down).is_none());

        // The square's corners run anticlockwise seen from below. Once its
        // material culls back faces, it is solid from below only.
        scene.materials.push(SceneMaterial {
            name: "one-sided".into(),
            texture: None,
            diffuse: [1.0; 3],
            ambient: [1.0; 3],
            specular: [0.0; 3],
            emission: [0.0; 3],
            alpha: 1.0,
            lights: 0,
            cull: Cull::Back,
            wrap_s: Wrap::Clamp,
            wrap_t: Wrap::Clamp,
            tex_matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            translucent: false,
        });
        assert!(scene.ray_cast(Vec3::new(1.0, 50.0, 2.0), down).is_none());
        let hit = scene.ray_cast(Vec3::new(1.0, -50.0, 2.0), Vec3::Y).expect("solid from below");
        assert_eq!(hit.node, 0);

        // A see-through material is passed through from either side.
        scene.materials[0].cull = Cull::None;
        assert!(scene.ray_cast(Vec3::new(1.0, 50.0, 2.0), down).is_some());
        scene.materials[0].translucent = true;
        assert!(scene.ray_cast(Vec3::new(1.0, 50.0, 2.0), down).is_none());

        // So are the transparent texels of a cut-out texture: here the
        // western half of the square (texture coordinate below 0.5).
        scene.materials[0].translucent = false;
        scene.materials[0].texture = Some(0);
        scene.textures.push(SceneTexture {
            name: "half".into(),
            image: Image { width: 2, height: 1, rgba: vec![0, 0, 0, 0, 255, 255, 255, 255] },
        });
        for model in &mut scene.models {
            for v in &mut model.batches[0].vertices {
                v.uv = [(v.pos[0] + 5.0) / 10.0, 0.5];
            }
        }
        assert!(scene.ray_cast(Vec3::new(-3.0, 50.0, 0.0), down).is_none());
        assert!(scene.ray_cast(Vec3::new(3.0, 50.0, 0.0), down).is_some());
    }

    fn marker(kind: MarkerKind) -> Marker {
        Marker {
            kind,
            pos: Vec3::new(32.0, 0.0, 48.0),
            size: [16.0, 16.0],
            zone: 0,
            index: 0,
            label: String::new(),
            target: None,
        }
    }

    #[test]
    fn a_ray_from_above_hits_a_marker_and_one_beside_it_misses() {
        let m = marker(MarkerKind::Npc);
        // An NPC box is 24 high, so a ray from y = 100 meets its top at 76.
        let t = m.ray_hit(Vec3::new(32.0, 100.0, 48.0), Vec3::NEG_Y).unwrap();
        assert!((t - 76.0).abs() < 1e-3);
        assert!(m.ray_hit(Vec3::new(60.0, 100.0, 48.0), Vec3::NEG_Y).is_none());
    }

    #[test]
    fn warps_cover_their_tiles_from_the_corner() {
        let (min, size) = marker(MarkerKind::Warp).bounds();
        assert_eq!(min, Vec3::new(32.0, 0.0, 48.0));
        assert_eq!(size, Vec3::new(16.0, 20.0, 16.0));
    }

    #[test]
    fn looping_frames_wrap_around() {
        assert_eq!(loop_frame(0.0, 60.0), 0.0);
        assert!((loop_frame(1.0, 60.0) - 30.0).abs() < 1e-4);
        assert!((loop_frame(2.5, 60.0) - 15.0).abs() < 1e-3);
        assert_eq!(loop_frame(5.0, 0.0), 0.0);
    }

    #[test]
    fn an_unmoved_texture_keeps_its_coordinates() {
        assert_eq!(tex_matrix([1.0, 1.0], 0.0, 1.0, [0.0, 0.0]), [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn texture_swaps_show_the_latest_key() {
        let a = TexPatternAnim { material: 0, frames: 30.0, keys: vec![(0.0, 7), (10.0, 8), (20.0, 9)] };
        assert_eq!(a.key_at(0.0), 0);
        assert_eq!(a.key_at(0.5), 1); // frame 15
        assert_eq!(a.key_at(0.9), 2); // frame 27
        assert_eq!(a.key_at(1.1), 0); // wrapped round to frame 3
    }
}
