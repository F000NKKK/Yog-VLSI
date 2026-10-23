//! The view of the chips: where the graph's world shows on the screen.

/// The view of the chips: their place `(x, y)` is drawn at
/// `pan + (x, y) · zoom` (local to the graph).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Camera {
    pub(super) zoom: f32,
    pub(super) pan: (f32, f32),
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            zoom: 1.0,
            pan: (0.0, 0.0),
        }
    }
}

impl Camera {
    pub(super) const MIN: f32 = 0.4;
    pub(super) const MAX: f32 = 3.0;

    pub(super) fn to_local(self, (x, y): (f32, f32)) -> (f32, f32) {
        (self.pan.0 + x * self.zoom, self.pan.1 + y * self.zoom)
    }

    pub(super) fn to_world(self, (x, y): (f32, f32)) -> (f32, f32) {
        ((x - self.pan.0) / self.zoom, (y - self.pan.1) / self.zoom)
    }

    /// Zoomed by `factor` keeping the point under `at` (local) where it is.
    pub(super) fn zoomed(self, factor: f32, at: (f32, f32)) -> Camera {
        let zoom = (self.zoom * factor).clamp(Self::MIN, Self::MAX);
        let world = self.to_world(at);
        Camera {
            zoom,
            pan: (at.0 - world.0 * zoom, at.1 - world.1 * zoom),
        }
    }
}
