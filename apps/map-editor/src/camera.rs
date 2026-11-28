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
        Camera { target: Vec3::ZERO, yaw: 0.0, pitch: 0.95, dist: 640.0, fov_y: EDITOR_FOV.to_radians(), ortho: false }
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

    /// Unity's right-drag: turn the view about the EYE (first-person look),
    /// so the target swings around in front of it.
    pub fn look(&mut self, dx: f32, dy: f32) {
        if self.ortho {
            return self.orbit(dx, dy);
        }
        let eye = self.eye();
        self.yaw += dx * 0.004;
        self.pitch = (self.pitch + dy * 0.004).clamp(-MAX_PITCH, MAX_PITCH);
        self.target = eye - self.back() * self.dist;
    }

    /// Unity's middle-drag: slide the view in the screen plane.
    pub fn pan_screen(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let per_px = 2.0 * self.half_height() / viewport_h.max(1.0);
        self.target += (-self.right() * dx + self.up() * dy) * per_px;
    }

    /// Fly the eye (and the target with it) by `d` metres in world space.
    pub fn fly(&mut self, d: Vec3) {
        self.target += d;
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

    /// The in-run camera (Hero_Common "Default Camera", src/loader/lua/rsmm/camera.lua):
    /// eye = look-at + D (cos X sin Y, sin X, cos X cos Y), so our yaw is the
    /// game's plus half a turn. Yaw 45° and distance 33 are proven in game;
    /// pitch 0.942 and FOV 31° are read from the file but their roles are not.
    pub fn view_game(&mut self) {
        self.yaw = GAME_YAW + std::f32::consts::PI;
        self.pitch = GAME_PITCH;
        self.dist = GAME_DIST;
        self.fov_y = GAME_FOV.to_radians();
        self.ortho = false;
    }

    /// Still looking the way the game does (panning keeps it).
    pub fn is_game(&self) -> bool {
        !self.ortho
            && (self.yaw - GAME_YAW - std::f32::consts::PI).abs() < 1e-4
            && (self.pitch - GAME_PITCH).abs() < 1e-4
            && (self.dist - GAME_DIST).abs() < 1e-3
    }
}

/// 45°, the file holds 0.785398 (π/4).
const GAME_YAW: f32 = std::f32::consts::FRAC_PI_4;
const GAME_PITCH: f32 = 0.942_478;
const GAME_DIST: f32 = 33.0;
const GAME_FOV: f32 = 31.0;
const EDITOR_FOV: f32 = 42.0;

impl Camera {
    /// Back to the editor's own lens after the game view.
    pub fn leave_game(&mut self) {
        self.fov_y = EDITOR_FOV.to_radians();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_view_matches_the_game_formula() {
        let mut c = Camera { target: Vec3::new(5.0, 2.0, -3.0), ..Default::default() };
        c.view_game();
        let (y, x) = (GAME_YAW, GAME_PITCH);
        let want = c.target + GAME_DIST * Vec3::new(x.cos() * y.sin(), x.sin(), x.cos() * y.cos());
        assert!((c.eye() - want).length() < 1e-3, "{:?} vs {want:?}", c.eye());
        assert!(c.is_game());
        c.orbit(10.0, 0.0);
        assert!(!c.is_game());
    }

    #[test]
    fn looking_around_keeps_the_eye_still() {
        let mut c = Camera { target: Vec3::new(10.0, 0.0, 5.0), dist: 50.0, ..Camera::default() };
        let eye = c.eye();
        c.look(120.0, -40.0);
        assert!((c.eye() - eye).length() < 1e-3);
        assert!((c.target - eye).length() - 50.0 < 1e-3);
    }

    #[test]
    fn a_screen_pan_keeps_the_view_direction() {
        let mut c = Camera::default();
        let f = c.forward();
        c.pan_screen(30.0, 20.0, 800.0);
        assert!((c.forward() - f).length() < 1e-6 && c.target != Vec3::ZERO);
    }
}
