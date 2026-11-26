use glam::{Mat4, Vec3, Vec4};

/// View and projection for one frame.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub view: Mat4,
    pub proj: Mat4,
    pub eye: Vec3,
}

impl Camera {
    pub fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }
}

/// A camera that orbits a target point. With `ortho` set it becomes a
/// parallel projection, which gives a distortion-free top-down map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Heading in radians; 0 looks north (towards -Z).
    pub yaw: f32,
    /// Angle below the horizon in radians; PI/2 looks straight down.
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    pub ortho: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.9,
            distance: 600.0,
            fov_y: 35f32.to_radians(),
            ortho: false,
        }
    }
}

impl OrbitCamera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, -sp, -cy * cp)
    }

    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, 0.0, sy)
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize()
    }

    pub fn eye(&self) -> Vec3 {
        self.target - self.forward() * self.distance
    }

    /// Height of the visible area at the target, in world units.
    pub fn view_height(&self) -> f32 {
        2.0 * self.distance * (self.fov_y * 0.5).tan()
    }

    pub fn camera(&self, aspect: f32) -> Camera {
        let eye = self.eye();
        let view = look_to(eye, self.forward(), self.up());
        let proj = if self.ortho {
            let h = self.view_height() * 0.5;
            let w = h * aspect;
            // Generous depth range: the eye may sit inside tall scenery.
            let range = self.distance.max(1000.0) * 4.0;
            orthographic(-w, w, -h, h, -range, range)
        } else {
            let near = (self.distance * 0.02).clamp(0.5, 50.0);
            let far = (self.distance * 20.0).max(20_000.0);
            perspective(self.fov_y, aspect, near, far)
        };
        Camera { view, proj, eye }
    }

    /// Places the camera so a bounding box fills the view.
    pub fn frame(&mut self, min: Vec3, max: Vec3, aspect: f32) {
        self.target = (min + max) * 0.5;
        let size = max - min;
        let half_h = (self.fov_y * 0.5).tan();
        if self.pitch > 1.5 {
            // Straight down: fit the footprint.
            let need_h = (size.z * 0.5).max(size.x * 0.5 / aspect.max(0.01));
            self.distance = (need_h / half_h).max(10.0) * 1.03;
        } else {
            let radius = (size.length() * 0.5).max(1.0);
            let fit = half_h.min(half_h * aspect);
            self.distance = radius / fit.max(0.01) * 0.9;
        }
    }

    /// Looks straight down with north at the top of the image.
    pub fn top_down(min: Vec3, max: Vec3, aspect: f32) -> Self {
        let mut c = Self { yaw: 0.0, pitch: std::f32::consts::FRAC_PI_2, ortho: true, ..Default::default() };
        c.frame(min, max, aspect);
        c.target.y = max.y;
        c
    }

    /// World-space ray through a point of the view (`ndc` in -1..=1, Y up).
    pub fn ray(&self, ndc: [f32; 2], aspect: f32) -> (Vec3, Vec3) {
        let cam = self.camera(aspect);
        let inv = cam.view_proj().inverse();
        let a = inv.project_point3(Vec3::new(ndc[0], ndc[1], 0.0));
        let b = inv.project_point3(Vec3::new(ndc[0], ndc[1], 1.0));
        (a, (b - a).normalize_or_zero())
    }

    /// Where the ray through a view point meets the horizontal plane `y`.
    pub fn pick_ground(&self, ndc: [f32; 2], aspect: f32, y: f32) -> Option<Vec3> {
        let (o, d) = self.ray(ndc, aspect);
        if d.y.abs() < 1e-6 {
            return None;
        }
        let t = (y - o.y) / d.y;
        Some(o + d * t)
    }
}

// Right-handed view and projection matrices for wgpu clip space (Y up,
// depth 0..1). Written out here so they do not depend on the maths crate's
// naming of graphics-API conventions.

fn look_to(eye: Vec3, dir: Vec3, up: Vec3) -> Mat4 {
    let f = dir.normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    Mat4::from_cols(
        Vec4::new(s.x, u.x, -f.x, 0.0),
        Vec4::new(s.y, u.y, -f.y, 0.0),
        Vec4::new(s.z, u.z, -f.z, 0.0),
        Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let t = 1.0 / (fov_y * 0.5).tan();
    let r = far / (near - far);
    Mat4::from_cols(
        Vec4::new(t / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, t, 0.0, 0.0),
        Vec4::new(0.0, 0.0, r, -1.0),
        Vec4::new(0.0, 0.0, r * near, 0.0),
    )
}

fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    let rw = 1.0 / (right - left);
    let rh = 1.0 / (top - bottom);
    let r = 1.0 / (near - far);
    Mat4::from_cols(
        Vec4::new(2.0 * rw, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * rh, 0.0, 0.0),
        Vec4::new(0.0, 0.0, r, 0.0),
        Vec4::new(-(left + right) * rw, -(top + bottom) * rh, r * near, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ray cast through the screen position of a point must pass that point.
    fn rays_return_to(cam: OrbitCamera) {
        let aspect = 1.5;
        let c = cam.camera(aspect);
        for p in [Vec3::new(10.0, 0.0, -20.0), Vec3::new(-150.0, 30.0, 80.0), cam.target] {
            let clip = c.view_proj() * p.extend(1.0);
            let (origin, dir) = cam.ray([clip.x / clip.w, clip.y / clip.w], aspect);
            let miss = (p - origin) - dir * (p - origin).dot(dir);
            assert!(miss.length() < 0.05, "ray misses {p:?} by {}", miss.length());
        }
    }

    #[test]
    fn rays_pass_through_the_points_they_were_cast_at() {
        rays_return_to(OrbitCamera {
            target: Vec3::new(5.0, 0.0, 7.0),
            yaw: 0.7,
            pitch: 0.8,
            distance: 400.0,
            ..Default::default()
        });
        rays_return_to(OrbitCamera::top_down(Vec3::new(-200.0, 0.0, -200.0), Vec3::new(200.0, 40.0, 200.0), 1.5));
    }

    #[test]
    fn the_ground_under_the_screen_centre_is_the_target() {
        let cam = OrbitCamera {
            target: Vec3::new(100.0, 0.0, 50.0),
            yaw: 0.3,
            pitch: 1.0,
            distance: 300.0,
            ..Default::default()
        };
        let hit = cam.pick_ground([0.0, 0.0], 1.5, 0.0).unwrap();
        assert!((hit - cam.target).length() < 0.05);
    }

    #[test]
    fn the_top_down_view_has_north_up_and_east_right() {
        let cam = OrbitCamera::top_down(Vec3::ZERO, Vec3::new(100.0, 0.0, 100.0), 1.0);
        let c = cam.camera(1.0);
        let screen = |p: Vec3| {
            let v = c.view_proj() * p.extend(1.0);
            (v.x / v.w, v.y / v.w)
        };
        let (cx, cy) = screen(Vec3::new(50.0, 0.0, 50.0));
        // Smaller Z is north, larger X is east.
        assert!(screen(Vec3::new(50.0, 0.0, 10.0)).1 > cy);
        assert!(screen(Vec3::new(90.0, 0.0, 50.0)).0 > cx);
    }
}
