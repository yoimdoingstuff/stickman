//! Draws the solved pose into a small transparent pixmap.

use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::math::Vec2;
use crate::rig::Pose;
use crate::stickman::Dims;

/// Colours derived from the one body colour the user picks.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    body: [u8; 4],
    /// Far-side arm and leg, slightly shifted so the limbs read as separate.
    back: [u8; 4],
    /// Thin rim so the stickman is visible on any background.
    outline: [u8; 4],
}

impl Style {
    pub fn from_color(c: [u8; 3]) -> Self {
        let lum = (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0;
        let dark = lum < 0.45;
        let mix = |v: u8| {
            let toward = if dark { 90.0 } else { 0.0 };
            (v as f32 + (toward - v as f32) * 0.3).round() as u8
        };
        Style {
            body: [c[0], c[1], c[2], 255],
            back: [mix(c[0]), mix(c[1]), mix(c[2]), 255],
            outline: if dark { [240, 240, 240, 200] } else { [22, 18, 18, 240] },
        }
    }
}

/// Nearly invisible, but makes the area around the body clickable
/// (fully transparent pixels of a layered window pass clicks through).
const HIT: [u8; 4] = [0, 0, 0, 2];

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

fn polyline(pm: &mut Pixmap, pts: &[Vec2], width: f32, c: [u8; 4]) {
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].x, pts[0].y);
    for q in &pts[1..] {
        pb.line_to(q.x, q.y);
    }
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
    }
}

fn circle(pm: &mut Pixmap, c: Vec2, r: f32, col: [u8; 4]) {
    if let Some(path) = PathBuilder::from_circle(c.x, c.y, r) {
        pm.fill_path(&path, &paint(col), FillRule::Winding, Transform::identity(), None);
    }
}

pub fn draw(pm: &mut Pixmap, pose: &Pose, origin: Vec2, d: &Dims, style: &Style) {
    pm.fill(Color::TRANSPARENT);
    let p = |v: Vec2| v - origin;
    let s = d.s;
    let w = d.stroke;
    let ow = w + 2.4 * s;

    let feet_mid = (pose.feet[0] + pose.feet[1]) * 0.5;
    polyline(pm, &[p(feet_mid), p(pose.head)], 24.0 * s, HIT);

    let limb = |pm: &mut Pixmap, pts: &[Vec2], col: [u8; 4]| {
        let pts: Vec<Vec2> = pts.iter().map(|&v| p(v)).collect();
        polyline(pm, &pts, ow, style.outline);
        polyline(pm, &pts, w, col);
    };

    // Back limbs first, then torso and head, then front limbs.
    limb(pm, &[pose.hip, pose.knees[0], pose.feet[0]], style.back);
    limb(pm, &[pose.chest, pose.elbows[0], pose.hands[0]], style.back);
    limb(pm, &[pose.hip, pose.chest, pose.neck], style.body);
    circle(pm, p(pose.head), d.head_r + 1.2 * s, style.outline);
    circle(pm, p(pose.head), d.head_r, style.body);
    limb(pm, &[pose.hip, pose.knees[1], pose.feet[1]], style.body);
    limb(pm, &[pose.chest, pose.elbows[1], pose.hands[1]], style.body);
}
