//! Controls for a [`RetroStyle`], shared by Scene settings (a scene's own
//! look, fog and background) and Project settings (the project look).
//!
//! The console look (resolution, wobble, textures, shading...) and the
//! scene's atmosphere (fog, background) are separate groups because a
//! scene that uses the project look still owns its fog and background.

use copperframe_core::{RetroStyle, TextureFilter};

/// The console-look controls: presets, low resolution, vertex snapping,
/// affine UVs, colour reduction, dithering, texture filtering, shading and
/// blob shadows. Returns true when a drag or edit finished (to end a
/// merged undo step).
pub fn console_look(ui: &mut egui::Ui, style: &mut RetroStyle) -> bool {
    let mut finished = false;
    ui.horizontal(|ui| {
        ui.label(format!("Preset: {}", style.preset().label()));
        // Presets keep the fog and background.
        let keep = |s: &mut RetroStyle, mut p: RetroStyle| {
            p.fog = s.fog;
            p.atmosphere = s.atmosphere;
            *s = p;
        };
        if ui.button("PS1").clicked() {
            keep(style, RetroStyle::ps1());
            finished = true;
        }
        if ui.button("PS2").clicked() {
            keep(style, RetroStyle::ps2());
            finished = true;
        }
        if ui.button("Clean").clicked() {
            keep(style, RetroStyle::clean());
            finished = true;
        }
    });
    ui.checkbox(&mut style.low_res.enabled, "Low resolution");
    ui.horizontal(|ui| {
        ui.checkbox(&mut style.vertex_snap.enabled, "Vertex snapping")
            .on_hover_text("The PS1 wobble effect, not the toolbar's snapping tool");
        let r = ui.add(egui::Slider::new(&mut style.vertex_snap.grid, 0.25..=8.0).text("grid px"));
        finished |= r.drag_stopped();
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut style.affine_uv.enabled, "Affine UVs")
            .on_hover_text("PS1 texture swim on large polygons");
        let r = ui
            .add_enabled(
                style.affine_uv.enabled,
                egui::Slider::new(&mut style.affine_uv.correction_distance, 0.0..=10.0)
                    .text("close-up fix m"),
            )
            .on_hover_text(
                "Textures within this distance of the camera stay steady, so walking up \
                 to a model no longer warps it. Farther polygons keep the PS1 swim. \
                 0 warps everywhere",
            );
        finished |= r.drag_stopped();
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut style.color_reduction.enabled, "Colour reduction");
        let r = ui.add(egui::Slider::new(&mut style.color_reduction.bits, 1..=8).text("bits"));
        finished |= r.drag_stopped();
    });
    ui.checkbox(&mut style.dither.enabled, "Dithering");
    ui.horizontal(|ui| {
        let linear = style.textures.filter == TextureFilter::Linear;
        let mut l = linear;
        ui.checkbox(&mut l, "Filtered textures");
        if l != linear {
            style.textures.filter = if l {
                TextureFilter::Linear
            } else {
                TextureFilter::Nearest
            };
        }
        ui.checkbox(&mut style.textures.mipmaps, "Mipmaps");
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut style.shading.per_pixel, "Per-pixel lighting");
        ui.checkbox(&mut style.shading.local_lights, "Point lights");
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut style.shadows.enabled, "Blob shadows");
        let r = ui.add(egui::Slider::new(&mut style.shadows.opacity, 0.0..=1.0).text("opacity"));
        finished |= r.drag_stopped();
    });
    finished
}

/// Fog and background colour (always the scene's own).
pub fn atmosphere(ui: &mut egui::Ui, style: &mut RetroStyle) -> bool {
    let mut finished = false;
    ui.horizontal(|ui| {
        ui.checkbox(&mut style.fog.enabled, "Fog");
        ui.color_edit_button_srgb(&mut style.fog.color);
        let r1 = ui.add(
            egui::DragValue::new(&mut style.fog.start)
                .speed(0.1)
                .prefix("start "),
        );
        let r2 = ui.add(
            egui::DragValue::new(&mut style.fog.end)
                .speed(0.1)
                .prefix("end "),
        );
        finished |= r1.drag_stopped() || r2.drag_stopped();
    });
    ui.horizontal(|ui| {
        ui.label("Background");
        ui.color_edit_button_srgb(&mut style.atmosphere.background);
    });
    finished
}

/// `look`'s console settings with `scene`'s fog and atmosphere.
pub fn with_scene_atmosphere(look: &RetroStyle, scene: &RetroStyle) -> RetroStyle {
    let mut s = look.clone();
    s.fog = scene.fog;
    s.atmosphere = scene.atmosphere;
    s
}
