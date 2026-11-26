//! Orbit camera in engine space: left-handed, Y up, +Z away from a camera at
//! yaw 0. The game's own coordinates go to the GPU untouched.
//!
//! Perspective or orthographic; the Blender numpad views (front / side / top)
//! set yaw and pitch, the orbit keeps working from there.

use glam::{Mat4, Vec3, Vec4};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub fov_y: f32,
    pub ortho: bool,
}

pub const NEAR: f32 = 0.5;
/// How far behind the target an orthographic eye sits: everything on the map
/// is in front of it whatever the zoom.
const ORTHO_BACK: f32 = 3000.0;
const ORTHO_FAR: f32 = 8000.0;
const MAX_PITCH: f32 = 1.5703;

impl Default for Camera {
    fn default() -> Self {
        Camera { target: Vec3::ZERO, yaw: 0.0, pitch: 0.95, dist: 640.0, fov_y: 42f32.to_radians(), ortho: false }
    }
}

impl Camera {
    /// Unit vector from the target toward the eye.
    fn back(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.back() * if self.ortho { ORTHO_BACK } else { self.dist }
    }

    pub fn forward(&self) -> Vec3 {
        -self.back()
    }

    pub fn right(&self) -> Vec3 {
        Vec3::Y.cross(self.forward()).normalize_or(Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin()))
    }

    pub fn up(&self) -> Vec3 {
        self.forward().cross(self.right())
    }

    /// Half the visible height at the target, in metres.
    pub fn half_height(&self) -> f32 {
        self.dist * (self.fov_y * 0.5).tan()
    }

    /// Left-handed view matrix: camera space x right, y up, +z forward.
    pub fn view(&self) -> Mat4 {
        let (f, r, u, e) = (self.forward(), self.right(), self.up(), self.eye());
        Mat4::from_cols(
            Vec4::new(r.x, u.x, f.x, 0.0),
            Vec4::new(r.y, u.y, f.y, 0.0),
            Vec4::new(r.z, u.z, f.z, 0.0),
            Vec4::new(-r.dot(e), -u.dot(e), -f.dot(e), 1.0),
        )
    }

    /// Reversed-Z: depth 1 at the near plane, 0 at infinity (perspective) or
    /// at the far plane (orthographic).
    pub fn proj(&self, aspect: f32) -> Mat4 {
        if self.ortho {
            let hh = self.half_height();
            let hw = hh * aspect;
            let range = ORTHO_FAR - NEAR;
            return Mat4::from_cols(
                Vec4::new(1.0 / hw, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0 / hh, 0.0, 0.0),
                Vec4::new(0.0, 0.0, -1.0 / range, 0.0),
                Vec4::new(0.0, 0.0, ORTHO_FAR / range, 1.0),
            );
        }
        let h = 1.0 / (self.fov_y * 0.5).tan();
        Mat4::from_cols(
            Vec4::new(h / aspect, 0.0, 0.0, 0.0),
            Vec4::new(0.0, h, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 0.0, NEAR, 0.0),
        )
    }

    /// World-space ray through a point in normalised device coords (-1..1, y up).
    pub fn ray(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> (Vec3, Vec3) {
        if self.ortho {
            let hh = self.half_height();
            let o = self.eye() + self.right() * (ndc_x * hh * aspect) + self.up() * (ndc_y * hh);
            return (o, self.forward());
        }
        let th = (self.fov_y * 0.5).tan();
        let d = self.forward() + self.right() * (ndc_x * th * aspect) + self.up() * (ndc_y * th);
        (self.eye(), d.normalize())
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.006;
        self.pitch = (self.pitch + dy * 0.006).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Move the target by a screen-space drag in pixels, in the ground plane.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let per_px = 2.0 * self.half_height() / viewport_h.max(1.0);
        let r = self.right();
        let f = Vec3::new(self.forward().x, 0.0, self.forward().z).normalize_or(Vec3::Z);
        self.target += (-r * dx + f * dy) * per_px;
    }

    /// Move the target straight up / down (Shift + wheel, Shift + middle drag, Q / E).
    pub fn lift(&mut self, metres: f32) {
        self.target.y += metres;
    }

    pub fn zoom(&mut self, steps: f32) {
        self.dist = (self.dist * (1.0 - steps * 0.0015).clamp(0.5, 1.5)).clamp(3.0, 5000.0);
    }

    /// Look at a world sphere from a distance that fits it.
    pub fn frame(&mut self, center: Vec3, radius: f32) {
        self.target = center;
        self.dist = (radius / (self.fov_y * 0.5).sin()).clamp(5.0, 4000.0);
    }

    /// Blender's numpad views.
    pub fn view_front(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.0;
    }

    pub fn view_side(&mut self) {
        self.yaw = -std::f32::consts::FRAC_PI_2;
        self.pitch = 0.0;
    }

    pub fn view_top(&mut self) {
        self.pitch = MAX_PITCH;
    }
}
