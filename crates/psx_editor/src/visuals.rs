//! Inspectors for the visual components: point light, blob shadow and
//! animator. Edits go through scene commands (undoable, merged while a
//! value is dragged).

use glam::Vec3;
use psx_runtime::scene_gpu::SceneResources;
use psx_scene::{AnimatorComponent, Command, Entity, LightComponent, MeshSource, ShadowComponent};

use crate::session::EditorSession;

fn finished(r: &egui::Response) -> bool {
    r.drag_stopped() || r.lost_focus()
}

pub fn light_editor(ui: &mut egui::Ui, session: &mut EditorSession, entity: &Entity) {
    let id = entity.id;
    ui.label(egui::RichText::new("Point light").strong());
    let Some(original) = entity.light else {
        if ui.button("+ Point light").clicked() {
            session.execute(Command::SetLight {
                id,
                light: Some(LightComponent::default()),
            });
        }
        return;
    };
    let mut l = original;
    let mut done = false;
    ui.horizontal(|ui| {
        ui.label("Colour");
        let mut rgb = l.color.to_array();
        // egui edits linear RGB here, which is what the component stores.
        if ui.color_edit_button_rgb(&mut rgb).changed() {
            l.color = Vec3::from(rgb).clamp(Vec3::ZERO, Vec3::ONE);
            done = true;
        }
        let r1 = ui.add(
            egui::DragValue::new(&mut l.intensity)
                .speed(0.02)
                .range(0.0..=16.0)
                .prefix("intensity "),
        );
        let r2 = ui.add(
            egui::DragValue::new(&mut l.range)
                .speed(0.05)
                .range(0.05..=100.0)
                .prefix("range ")
                .suffix(" m"),
        );
        done |= finished(&r1) || finished(&r2);
    });
    if session.scene.settings.style.shading.local_lights {
        ui.label(egui::RichText::new("At most 8 lights nearest the camera are used.").small());
    } else {
        ui.label(
            egui::RichText::new("The scene style has local lights off; this light is not drawn.")
                .small(),
        );
    }
    let key = format!("light:{id}");
    if l != original && l.problem().is_none() {
        session.execute_merging(Command::SetLight { id, light: Some(l) }, Some(&key));
    }
    if done {
        session.end_merge();
    }
    if ui.button("Remove light").clicked() {
        session.execute(Command::SetLight { id, light: None });
    }
}

pub fn shadow_editor(ui: &mut egui::Ui, session: &mut EditorSession, entity: &Entity) {
    let id = entity.id;
    ui.label(egui::RichText::new("Blob shadow").strong());
    let Some(original) = entity.shadow else {
        if ui.button("+ Blob shadow").clicked() {
            session.execute(Command::SetShadow {
                id,
                shadow: Some(ShadowComponent::default()),
            });
        }
        return;
    };
    let mut s = original;
    let mut done = false;
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::DragValue::new(&mut s.radius)
                .speed(0.01)
                .range(0.05..=20.0)
                .prefix("radius "),
        );
        done |= finished(&r);
        ui.label("offset");
        for c in [&mut s.offset.x, &mut s.offset.y, &mut s.offset.z] {
            let r = ui.add(
                egui::DragValue::new(c)
                    .speed(0.01)
                    .range(-1000.0..=1000.0)
                    .max_decimals(2),
            );
            done |= finished(&r);
        }
    });
    if !session.scene.settings.style.shadows.enabled {
        ui.label(
            egui::RichText::new("The scene style has blob shadows off; this shadow is not drawn.")
                .small(),
        );
    }
    let key = format!("shadow:{id}");
    if s != original && s.problem().is_none() {
        session.execute_merging(
            Command::SetShadow {
                id,
                shadow: Some(s),
            },
            Some(&key),
        );
    }
    if done {
        session.end_merge();
    }
    if ui.button("Remove shadow").clicked() {
        session.execute(Command::SetShadow { id, shadow: None });
    }
}

/// Clip names of the entity's model when it is loaded and skinned.
fn clips_of(resources: &SceneResources, entity: &Entity) -> Option<Vec<String>> {
    let Some(MeshSource::Model { asset }) = entity.mesh.as_ref().map(|m| &m.source) else {
        return None;
    };
    let anim = resources.animation(*asset)?;
    Some(anim.clips.iter().map(|c| c.name.clone()).collect())
}

pub fn animator_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    resources: &SceneResources,
    entity: &Entity,
) {
    let id = entity.id;
    ui.label(egui::RichText::new("Animator").strong());
    let clips = clips_of(resources, entity);
    let Some(original) = entity.animator.clone() else {
        match &clips {
            Some(clips) if !clips.is_empty() => {
                egui::ComboBox::from_id_salt("add-animator")
                    .selected_text("+ Animator…")
                    .show_ui(ui, |ui| {
                        for c in clips {
                            if ui.selectable_label(false, c).clicked() {
                                session.execute(Command::SetAnimator {
                                    id,
                                    animator: Some(AnimatorComponent::new(c.clone())),
                                });
                            }
                        }
                    });
            }
            _ => {
                ui.label(
                    egui::RichText::new("needs a model with a skin and animation clips").small(),
                );
            }
        }
        return;
    };
    let mut a = original.clone();
    let mut done = false;
    let known = clips.as_ref().is_some_and(|c| c.contains(&a.clip));
    egui::ComboBox::from_id_salt("animator-clip")
        .selected_text(if known {
            a.clip.clone()
        } else {
            format!("missing \"{}\"", a.clip)
        })
        .show_ui(ui, |ui| {
            for c in clips.iter().flatten() {
                if ui.selectable_label(a.clip == *c, c).clicked() {
                    a.clip = c.clone();
                    done = true;
                }
            }
        });
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::DragValue::new(&mut a.speed)
                .speed(0.01)
                .range(-10.0..=10.0)
                .prefix("speed "),
        );
        done |= finished(&r);
        done |= ui.checkbox(&mut a.looping, "loop").changed();
        done |= ui.checkbox(&mut a.autoplay, "autoplay").changed();
    });
    ui.label(egui::RichText::new("The viewport previews autoplaying clips.").small());
    let key = format!("animator:{id}");
    if a != original && a.problem().is_none() {
        session.execute_merging(
            Command::SetAnimator {
                id,
                animator: Some(a),
            },
            Some(&key),
        );
    }
    if done {
        session.end_merge();
    }
    if ui.button("Remove animator").clicked() {
        session.execute(Command::SetAnimator { id, animator: None });
    }
}
