//! Icons drawn with lines, so the editor needs no icon font or image files.

use egui::epaint::PathStroke;
use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, vec2};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Move,
    Rotate,
    Scale,
    GridSnap,
    VertexSnap,
    ModelSnap,
    /// Three sliders: settings for the tool beside it.
    Tune,
    FrameSelected,
    Play,
    /// Two bars.
    Pause,
    /// Advance one tick: a triangle against a bar.
    Step,
    Stop,
    Model,
    Texture,
    Sound,
    Folder,
    /// A scene file: a framed landscape.
    Scene,
    /// Project settings: a cog wheel.
    Cog,
    /// Drag handle: two columns of dots.
    Grip,
    ArrowUp,
    ArrowDown,
    Trash,
    Plus,
}

/// Paints `icon` to fill `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    // Points on a -1..1 square around the centre.
    let at = |x: f32, y: f32| c + vec2(x, y) * (s * 0.5);
    let w = (s * 0.08).max(1.25);
    let stroke = Stroke::new(w, color);
    let line = |points: &[(f32, f32)]| {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| at(x, y)).collect();
        painter.add(Shape::line(points, PathStroke::from(stroke)));
    };
    match icon {
        Icon::Move => {
            line(&[(-0.85, 0.0), (0.85, 0.0)]);
            line(&[(0.0, -0.85), (0.0, 0.85)]);
            line(&[(-0.55, -0.3), (-0.85, 0.0), (-0.55, 0.3)]);
            line(&[(0.55, -0.3), (0.85, 0.0), (0.55, 0.3)]);
            line(&[(-0.3, -0.55), (0.0, -0.85), (0.3, -0.55)]);
            line(&[(-0.3, 0.55), (0.0, 0.85), (0.3, 0.55)]);
        }
        Icon::Rotate => {
            let arc: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let a = -0.9 + i as f32 / 20.0 * 5.0;
                    (a.cos() * 0.75, a.sin() * 0.75)
                })
                .collect();
            line(&arc);
            let end = arc[arc.len() - 1];
            line(&[
                (end.0 - 0.3, end.1 - 0.25),
                end,
                (end.0 + 0.32, end.1 - 0.2),
            ]);
        }
        Icon::Scale => {
            line(&[
                (-0.8, 0.8),
                (-0.8, 0.05),
                (-0.05, 0.05),
                (-0.05, 0.8),
                (-0.8, 0.8),
            ]);
            line(&[(-0.05, 0.05), (0.75, -0.75)]);
            line(&[(0.25, -0.75), (0.75, -0.75), (0.75, -0.25)]);
        }
        Icon::GridSnap => {
            for k in [-0.8, -0.27, 0.27, 0.8] {
                line(&[(k, -0.8), (k, 0.8)]);
                line(&[(-0.8, k), (0.8, k)]);
            }
        }
        Icon::VertexSnap => {
            line(&[(-0.75, 0.75), (0.0, -0.65), (0.75, 0.75), (-0.75, 0.75)]);
            for (x, y) in [(-0.75, 0.75), (0.75, 0.75)] {
                painter.circle_filled(at(x, y), w * 1.4, color);
            }
            painter.circle_filled(at(0.0, -0.65), w * 2.0, theme::ACCENT);
        }
        Icon::ModelSnap => {
            line(&[
                (-0.9, -0.5),
                (-0.1, -0.5),
                (-0.1, 0.5),
                (-0.9, 0.5),
                (-0.9, -0.5),
            ]);
            line(&[
                (0.15, -0.5),
                (0.9, -0.5),
                (0.9, 0.5),
                (0.15, 0.5),
                (0.15, -0.5),
            ]);
        }
        Icon::Tune => {
            for (y, knob) in [(-0.6, 0.35), (0.0, -0.4), (0.6, 0.1)] {
                line(&[(-0.85, y), (0.85, y)]);
                painter.circle_filled(at(knob, y), w * 2.0, color);
            }
        }
        Icon::FrameSelected => {
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let (x, y) = (x * 0.8, y * 0.8);
                line(&[(x * 0.45, y), (x, y), (x, y * 0.45)]);
            }
            painter.circle_filled(c, s * 0.12, color);
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![at(-0.55, -0.75), at(0.75, 0.0), at(-0.55, 0.75)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for x in [-0.4, 0.4] {
                painter.rect_filled(
                    Rect::from_center_size(at(x, 0.0), vec2(s * 0.24, s * 0.72)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Step => {
            painter.add(Shape::convex_polygon(
                vec![at(-0.7, -0.7), at(0.35, 0.0), at(-0.7, 0.7)],
                color,
                Stroke::NONE,
            ));
            painter.rect_filled(
                Rect::from_center_size(at(0.6, 0.0), vec2(s * 0.2, s * 0.72)),
                1.0,
                color,
            );
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, vec2(s, s) * 0.68), 1.0, color);
        }
        Icon::Model => {
            // A cube: the top rhombus and the two visible sides.
            let (top, left, right, front, bl, br, bottom) = (
                (0.0, -0.85),
                (-0.75, -0.42),
                (0.75, -0.42),
                (0.0, 0.0),
                (-0.75, 0.42),
                (0.75, 0.42),
                (0.0, 0.85),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    at(top.0, top.1),
                    at(right.0, right.1),
                    at(front.0, front.1),
                    at(left.0, left.1),
                ],
                color.gamma_multiply(0.45),
                Stroke::NONE,
            ));
            line(&[top, right, br, bottom, bl, left, top]);
            line(&[left, front, right]);
            line(&[front, bottom]);
        }
        Icon::Texture => {
            line(&[
                (-0.85, -0.7),
                (0.85, -0.7),
                (0.85, 0.7),
                (-0.85, 0.7),
                (-0.85, -0.7),
            ]);
            line(&[
                (-0.85, 0.45),
                (-0.3, -0.1),
                (0.15, 0.35),
                (0.45, 0.05),
                (0.85, 0.45),
            ]);
            painter.circle_filled(at(0.4, -0.35), s * 0.09, color);
        }
        Icon::Sound => {
            for (x, h) in [
                (-0.8, 0.2),
                (-0.48, 0.55),
                (-0.16, 0.9),
                (0.16, 0.45),
                (0.48, 0.7),
                (0.8, 0.25),
            ] {
                line(&[(x, -h), (x, h)]);
            }
        }
        Icon::Cog => {
            // Eight teeth around a ring with a hole.
            let teeth = 8;
            let outline: Vec<(f32, f32)> = (0..=teeth * 4)
                .map(|i| {
                    let a = i as f32 / (teeth * 4) as f32 * std::f32::consts::TAU;
                    // Two points out on the tooth, two in at its root.
                    let r = if i % 4 < 2 { 0.88 } else { 0.64 };
                    (a.cos() * r, a.sin() * r)
                })
                .collect();
            line(&outline);
            painter.circle_stroke(c, s * 0.5 * 0.28, stroke);
        }
        Icon::Scene => {
            line(&[
                (-0.9, -0.7),
                (0.9, -0.7),
                (0.9, 0.7),
                (-0.9, 0.7),
                (-0.9, -0.7),
            ]);
            line(&[
                (-0.9, 0.55),
                (-0.35, -0.1),
                (0.05, 0.3),
                (0.35, 0.0),
                (0.9, 0.55),
            ]);
            painter.circle_filled(at(0.45, -0.35), s * 0.09, color);
        }
        Icon::Folder => {
            let body = [at(-0.9, -0.35), at(0.9, -0.35), at(0.9, 0.7), at(-0.9, 0.7)];
            painter.add(Shape::convex_polygon(
                body.to_vec(),
                color.gamma_multiply(0.45),
                Stroke::NONE,
            ));
            line(&[
                (-0.9, 0.7),
                (-0.9, -0.7),
                (-0.25, -0.7),
                (-0.05, -0.35),
                (0.9, -0.35),
                (0.9, 0.7),
                (-0.9, 0.7),
            ]);
        }
        Icon::Grip => {
            for x in [-0.3, 0.3] {
                for y in [-0.6, 0.0, 0.6] {
                    painter.circle_filled(at(x, y), (s * 0.1).max(1.2), color);
                }
            }
        }
        Icon::ArrowUp => {
            line(&[(0.0, 0.8), (0.0, -0.75)]);
            line(&[(-0.55, -0.2), (0.0, -0.75), (0.55, -0.2)]);
        }
        Icon::ArrowDown => {
            line(&[(0.0, -0.8), (0.0, 0.75)]);
            line(&[(-0.55, 0.2), (0.0, 0.75), (0.55, 0.2)]);
        }
        Icon::Trash => {
            line(&[(-0.8, -0.55), (0.8, -0.55)]);
            line(&[(-0.25, -0.55), (-0.2, -0.82), (0.2, -0.82), (0.25, -0.55)]);
            line(&[(-0.58, -0.55), (-0.45, 0.85), (0.45, 0.85), (0.58, -0.55)]);
            line(&[(-0.15, -0.2), (-0.12, 0.55)]);
            line(&[(0.15, -0.2), (0.12, 0.55)]);
        }
        Icon::Plus => {
            painter.circle_stroke(c, s * 0.42, stroke);
            line(&[(-0.42, 0.0), (0.42, 0.0)]);
            line(&[(0.0, -0.42), (0.0, 0.42)]);
        }
    }
}

/// A small borderless icon button for rows and card headers.
pub fn small_button(ui: &mut Ui, icon: Icon, enabled: bool, tip: &str) -> Response {
    let size = vec2(20.0, 20.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        if enabled && response.hovered() {
            ui.painter()
                .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
        }
        let color = if !enabled {
            theme::TEXT_DIM.gamma_multiply(0.4)
        } else if response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        paint(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(13.0, 13.0)),
            icon,
            color,
        );
    }
    response.on_hover_text(tip)
}

/// A square toolbar button holding an icon. `on` lights it in the accent.
pub fn button(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let size = vec2(26.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if on {
            Color32::from_rgb(132, 78, 38)
        } else if response.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 3.0, fill);
        if on {
            ui.painter().hline(
                rect.x_range().shrink(3.0),
                rect.bottom() - 0.5,
                Stroke::new(1.5, theme::ACCENT),
            );
        }
        let color = if on { theme::TEXT } else { theme::TEXT_DIM };
        paint(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(15.0, 15.0)),
            icon,
            color,
        );
    }
    response.on_hover_text(tip)
}

/// Left-aligned square for an icon at the start of a row.
pub fn row_rect(ui: &mut Ui, side: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    rect
}
