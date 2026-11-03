//! glTF 2.0 import (`.gltf` with external or embedded buffers, and binary
//! `.glb`).
//!
//! Supported: triangle lists, strips and fans; POSITION, NORMAL (computed
//! when absent), TEXCOORD_0, COLOR_0; u8/u16/u32 indices; node transforms
//! of the default scene (baked into the vertices, mirrored nodes get their
//! winding flipped); base colour factor and texture (PNG/JPEG, embedded or
//! external), alpha mode and cutoff, double-sided, emissive factor, and
//! `KHR_materials_unlit`.
//!
//! Skinning and animation: one skin per model (up to
//! [`MAX_SKIN_JOINTS`] joints) with JOINTS_0/WEIGHTS_0 (weights are
//! renormalised), inverse bind matrices (identity when absent), and
//! animation clips whose channels target the skin's joints (translation,
//! rotation, scale; STEP and LINEAR; CUBICSPLINE keys are imported as
//! LINEAR through their values, with a warning). Skinned primitives keep
//! their bind-space vertices; the node transform of a skinned mesh is
//! ignored, as the glTF specification requires.
//!
//! Reported and ignored (import still succeeds, with a warning): points and
//! lines, morph targets and weight animation, a second skin (those meshes
//! are drawn unskinned in their node pose), channels targeting non-joint
//! nodes, JOINTS_1/WEIGHTS_1, UV1+, metallic/roughness,
//! normal/occlusion/emissive textures, texture transforms on non-required
//! extensions, samplers (filtering is a style setting).
//!
//! Rejected (import fails, previous import is kept by callers): required
//! extensions other than `KHR_materials_unlit` (e.g. Draco or meshopt
//! compression), sparse accessors, out-of-range buffers/views/accessors/
//! indices, non-finite values, references outside the project, network URIs.
//!
//! glTF is right-handed, +Y up, meters, front faces counter-clockwise and
//! UV origin top-left: the same conventions as the engine, so no axis or
//! UV conversion is applied.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine as _;
use glam::{Mat3, Mat4, Quat, Vec3, Vec4};
use gltf::accessor::{DataType, Dimensions};
use gltf::mesh::Mode;
use gltf::{Accessor, Document, Semantic};
use psx_scene::AlphaSetting;

use super::texture::decode_image;
use super::{
    CpuMesh, ImportError, ImportedMaterial, ImportedModel, MeshVertex, ModelPart, guarded,
    read_source, resolve_reference,
};
use crate::anim::{
    AnimationClip, Channel, ChannelProperty, Interpolation, Joint, MAX_SKIN_JOINTS, Skeleton, Trs,
};
use crate::meta::ImportSettings;

/// Required extensions this importer understands.
const SUPPORTED_REQUIRED: &[&str] = &["KHR_materials_unlit"];
const MAX_NODE_DEPTH: usize = 64;
/// Upper bound on vertices or indices in one accessor.
const MAX_ELEMENTS: usize = 16 * 1024 * 1024;

/// Imports `path`. External references must resolve inside `boundary`.
pub fn import_gltf(
    path: &Path,
    boundary: &Path,
    settings: &ImportSettings,
) -> Result<ImportedModel, ImportError> {
    guarded(path, || Importer::run(path, boundary, settings))
}

struct Importer<'a> {
    path: &'a Path,
    document: Document,
    buffers: Vec<Vec<u8>>,
    settings: &'a ImportSettings,
    warnings: std::cell::RefCell<Vec<String>>,
}

impl<'a> Importer<'a> {
    fn run(
        path: &'a Path,
        boundary: &Path,
        settings: &'a ImportSettings,
    ) -> Result<ImportedModel, ImportError> {
        let bytes = read_source(path)?;
        let gltf::Gltf { document, blob } =
            gltf::Gltf::from_slice(&bytes).map_err(|e| ImportError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
        for ext in document.extensions_required() {
            if !SUPPORTED_REQUIRED.contains(&ext) {
                return Err(ImportError::UnsupportedExtension {
                    path: path.to_path_buf(),
                    extension: ext.to_owned(),
                });
            }
        }
        let mut warnings = Vec::new();
        for ext in document.extensions_used() {
            if !SUPPORTED_REQUIRED.contains(&ext) {
                warnings.push(format!("extension `{ext}` is ignored"));
            }
        }
        let buffers = load_buffers(path, boundary, &document, blob)?;
        let importer = Importer {
            path,
            document,
            buffers,
            settings,
            warnings: warnings.into(),
        };
        importer.check_views()?;
        let (mut materials, textures) = importer.materials(boundary)?;
        let skin = importer.skin()?;
        let parts = importer.parts(materials.len(), skin.as_ref())?;
        let clips = match &skin {
            Some(skin) => importer.clips(skin)?,
            None => Vec::new(),
        };
        if parts.iter().any(|p| p.material == materials.len()) {
            materials.push(ImportedMaterial::default());
        }
        if parts.iter().all(|p| p.mesh.indices.is_empty()) {
            return Err(ImportError::Empty {
                path: path.to_path_buf(),
            });
        }
        importer.warn_unsupported_features(skin.is_some());
        let skinned = parts.iter().any(|p| p.skinned);
        Ok(ImportedModel {
            parts,
            materials,
            textures,
            skeleton: skin.filter(|_| skinned).map(|s| s.skeleton),
            clips: if skinned { clips } else { Vec::new() },
            warnings: importer.warnings.into_inner(),
        })
    }

    fn malformed(&self, what: impl Into<String>, message: impl Into<String>) -> ImportError {
        ImportError::Malformed {
            path: self.path.to_path_buf(),
            what: what.into(),
            message: message.into(),
        }
    }

    fn check_views(&self) -> Result<(), ImportError> {
        for view in self.document.views() {
            let buffer = &self.buffers[view.buffer().index()];
            let end = view.offset().checked_add(view.length());
            if end.is_none_or(|end| end > buffer.len()) {
                return Err(self.malformed(
                    format!("bufferView {}", view.index()),
                    format!(
                        "range {}+{} exceeds buffer {} ({} bytes)",
                        view.offset(),
                        view.length(),
                        view.buffer().index(),
                        buffer.len()
                    ),
                ));
            }
            if let Some(stride) = view.stride()
                && !(4..=252).contains(&stride)
            {
                return Err(self.malformed(
                    format!("bufferView {}", view.index()),
                    format!("byteStride {stride} is outside 4..=252"),
                ));
            }
        }
        Ok(())
    }

    fn view_bytes(&self, view: &gltf::buffer::View<'_>) -> &[u8] {
        // Ranges were checked in `check_views`.
        &self.buffers[view.buffer().index()][view.offset()..view.offset() + view.length()]
    }

    /// Reads a float-valued accessor as `dims` components per element.
    /// Integer components are accepted only when `normalized`.
    fn read_floats(
        &self,
        accessor: &Accessor<'_>,
        dims: &[Dimensions],
        what: &str,
    ) -> Result<Vec<[f32; 4]>, ImportError> {
        let label = format!("{what} (accessor {})", accessor.index());
        if !dims.contains(&accessor.dimensions()) {
            return Err(self.malformed(
                label,
                format!("type {:?} is not supported here", accessor.dimensions()),
            ));
        }
        let ty = accessor.data_type();
        if ty != DataType::F32 && !accessor.normalized() {
            return Err(self.malformed(
                label,
                format!("component type {ty:?} must be float or normalized"),
            ));
        }
        let n = accessor.dimensions().multiplicity();
        let raw = self.read_raw(accessor, &label)?;
        let size = ty.size();
        let mut out = Vec::with_capacity(raw.len());
        for element in raw {
            let mut v = [0.0, 0.0, 0.0, 1.0];
            for (c, slot) in v.iter_mut().enumerate().take(n) {
                let b = &element[c * size..(c + 1) * size];
                *slot = match ty {
                    DataType::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    DataType::U8 => f32::from(b[0]) / 255.0,
                    DataType::U16 => f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
                    DataType::I8 => (f32::from(b[0] as i8) / 127.0).max(-1.0),
                    DataType::I16 => {
                        (f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0).max(-1.0)
                    }
                    DataType::U32 => {
                        return Err(self.malformed(&label, "u32 components cannot be normalized"));
                    }
                };
                if !slot.is_finite() {
                    return Err(self.malformed(&label, "contains a non-finite value"));
                }
            }
            out.push(v);
        }
        Ok(out)
    }

    /// Reads JOINTS_n: four unsigned u8/u16 indices per vertex.
    fn read_joints(&self, accessor: &Accessor<'_>) -> Result<Vec<[u16; 4]>, ImportError> {
        let label = format!("JOINTS_0 (accessor {})", accessor.index());
        if accessor.dimensions() != Dimensions::Vec4 {
            return Err(self.malformed(label, "joints must be VEC4"));
        }
        let ty = accessor.data_type();
        if !matches!(ty, DataType::U8 | DataType::U16) || accessor.normalized() {
            return Err(self.malformed(
                label,
                format!("joint component type {ty:?} must be unsigned byte or short"),
            ));
        }
        let raw = self.read_raw(accessor, &label)?;
        Ok(raw
            .into_iter()
            .map(|b| {
                std::array::from_fn(|c| match ty {
                    DataType::U8 => u16::from(b[c]),
                    _ => u16::from_le_bytes([b[c * 2], b[c * 2 + 1]]),
                })
            })
            .collect())
    }

    fn read_indices(&self, accessor: &Accessor<'_>) -> Result<Vec<u32>, ImportError> {
        let label = format!("indices (accessor {})", accessor.index());
        if accessor.dimensions() != Dimensions::Scalar {
            return Err(self.malformed(label, "indices must be scalars"));
        }
        let ty = accessor.data_type();
        if !matches!(ty, DataType::U8 | DataType::U16 | DataType::U32) {
            return Err(self.malformed(label, format!("index type {ty:?} is not unsigned")));
        }
        let raw = self.read_raw(accessor, &label)?;
        Ok(raw
            .into_iter()
            .map(|b| match ty {
                DataType::U8 => u32::from(b[0]),
                DataType::U16 => u32::from(u16::from_le_bytes([b[0], b[1]])),
                _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            })
            .collect())
    }

    /// Each element's bytes, after checking the whole range lies in the view.
    fn read_raw<'b>(
        &'b self,
        accessor: &Accessor<'_>,
        label: &str,
    ) -> Result<Vec<&'b [u8]>, ImportError> {
        if accessor.sparse().is_some() {
            return Err(self.malformed(label, "sparse accessors are not supported"));
        }
        let Some(view) = accessor.view() else {
            return Err(self.malformed(label, "accessor has no bufferView"));
        };
        let count = accessor.count();
        if count == 0 {
            return Ok(Vec::new());
        }
        if count > MAX_ELEMENTS {
            return Err(self.malformed(label, format!("{count} elements exceed the limit")));
        }
        let element = accessor.data_type().size() * accessor.dimensions().multiplicity();
        let stride = view.stride().unwrap_or(element);
        if stride < element {
            return Err(self.malformed(
                label,
                format!("byteStride {stride} is smaller than the element size {element}"),
            ));
        }
        let bytes = self.view_bytes(&view);
        let last_end = (count - 1)
            .checked_mul(stride)
            .and_then(|x| x.checked_add(accessor.offset()))
            .and_then(|x| x.checked_add(element));
        if last_end.is_none_or(|end| end > bytes.len()) {
            return Err(self.malformed(
                label,
                format!(
                    "{count} elements at offset {} with stride {stride} exceed bufferView {} ({} bytes)",
                    accessor.offset(),
                    view.index(),
                    bytes.len()
                ),
            ));
        }
        Ok((0..count)
            .map(|i| {
                let start = accessor.offset() + i * stride;
                &bytes[start..start + element]
            })
            .collect())
    }

    fn materials(
        &self,
        boundary: &Path,
    ) -> Result<(Vec<ImportedMaterial>, Vec<super::CpuTexture>), ImportError> {
        let mut textures = Vec::new();
        let mut texture_for_image: HashMap<usize, usize> = HashMap::new();
        let mut materials = Vec::new();
        let doc_materials: Vec<_> = self.document.materials().collect();
        for m in doc_materials {
            let name = m
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("material {}", m.index().unwrap_or(0)));
            let pbr = m.pbr_metallic_roughness();
            let base_texture = match pbr.base_color_texture() {
                Some(info) => {
                    if info.tex_coord() != 0 {
                        self.warnings.borrow_mut().push(format!(
                            "{name}: base colour texture uses TEXCOORD_{}; only TEXCOORD_0 is supported",
                            info.tex_coord()
                        ));
                    }
                    let image = info.texture().source();
                    let index = image.index();
                    match texture_for_image.get(&index) {
                        Some(&t) => Some(t),
                        None => {
                            let tex = self.decode(&image, boundary)?;
                            textures.push(tex);
                            texture_for_image.insert(index, textures.len() - 1);
                            Some(textures.len() - 1)
                        }
                    }
                }
                None => None,
            };
            let mut ignored = Vec::new();
            if pbr.metallic_factor() != 1.0
                || pbr.roughness_factor() != 1.0
                || pbr.metallic_roughness_texture().is_some()
            {
                ignored.push("metallic/roughness");
            }
            if m.normal_texture().is_some() {
                ignored.push("normal texture");
            }
            if m.occlusion_texture().is_some() {
                ignored.push("occlusion texture");
            }
            if m.emissive_texture().is_some() {
                ignored.push("emissive texture");
            }
            if !ignored.is_empty() {
                self.warnings.borrow_mut().push(format!(
                    "{name}: not supported, ignored: {}",
                    ignored.join(", ")
                ));
            }
            let alpha = match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaSetting::Opaque,
                gltf::material::AlphaMode::Mask => AlphaSetting::Cutout {
                    threshold: m.alpha_cutoff().unwrap_or(0.5).clamp(0.0, 1.0),
                },
                gltf::material::AlphaMode::Blend => AlphaSetting::Blend,
            };
            let base_color = pbr.base_color_factor();
            let emissive = m.emissive_factor();
            if !base_color.iter().chain(&emissive).all(|c| c.is_finite()) {
                return Err(self.malformed(&name, "material factors must be finite"));
            }
            materials.push(ImportedMaterial {
                name,
                base_color,
                base_texture,
                alpha,
                double_sided: m.double_sided(),
                unlit: m.unlit(),
                emissive,
            });
        }
        Ok((materials, textures))
    }

    fn decode(
        &self,
        image: &gltf::image::Image<'_>,
        boundary: &Path,
    ) -> Result<super::CpuTexture, ImportError> {
        let what = image
            .name()
            .map(|n| format!("`{n}`"))
            .unwrap_or_else(|| format!("{}", image.index()));
        let image_err = |message: String| ImportError::Image {
            path: self.path.to_path_buf(),
            what: what.clone(),
            message,
        };
        let (bytes, mime): (std::borrow::Cow<'_, [u8]>, Option<&str>) = match image.source() {
            gltf::image::Source::View { view, mime_type } => {
                (self.view_bytes(&view).into(), Some(mime_type))
            }
            gltf::image::Source::Uri { uri, mime_type } => match decode_data_uri(uri) {
                Some(result) => {
                    let (data, data_mime) = result.map_err(&image_err)?;
                    let mime = mime_type.or(data_mime.filter(|m| m.starts_with("image/")));
                    (data.into(), mime)
                }
                None => {
                    let file = resolve_reference(self.path, boundary, uri)?;
                    (read_source(&file)?.into(), mime_type)
                }
            },
        };
        let (texture, reduced) =
            decode_image(&bytes, mime, self.settings.max_texture_size).map_err(image_err)?;
        if let Some((w, h)) = reduced {
            self.warnings.borrow_mut().push(format!(
                "image {what} reduced from {w}x{h} to {}x{} (max_texture_size {})",
                texture.width, texture.height, self.settings.max_texture_size
            ));
        }
        Ok(texture)
    }

    fn parts(
        &self,
        material_count: usize,
        skin: Option<&SkinImport>,
    ) -> Result<Vec<ModelPart>, ImportError> {
        // Default scene, else the first, else every mesh untransformed.
        let roots: Vec<(gltf::Node<'_>, Mat4)> = match self
            .document
            .default_scene()
            .or_else(|| self.document.scenes().next())
        {
            Some(scene) => scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect(),
            None => Vec::new(),
        };
        let scale = Mat4::from_scale(Vec3::splat(self.settings.scale));
        // (mesh, world, name, skinned)
        let mut instances: Vec<(gltf::Mesh<'_>, Mat4, String, bool)> = Vec::new();
        if roots.is_empty() {
            for mesh in self.document.meshes() {
                let name = mesh.name().unwrap_or("mesh").to_owned();
                instances.push((mesh, scale, name, false));
            }
        } else {
            let mut stack: Vec<(gltf::Node<'_>, Mat4, usize)> =
                roots.into_iter().map(|(n, m)| (n, scale * m, 0)).collect();
            let mut visited = 0usize;
            while let Some((node, parent, depth)) = stack.pop() {
                visited += 1;
                if depth > MAX_NODE_DEPTH || visited > 100_000 {
                    return Err(self.malformed("node hierarchy", "is too deep or cyclic"));
                }
                let local = Mat4::from_cols_array_2d(&node.transform().matrix());
                if !local.is_finite() {
                    return Err(
                        self.malformed(format!("node {}", node.index()), "transform is not finite")
                    );
                }
                let world = parent * local;
                let skinned = match (node.skin(), skin) {
                    (Some(s), Some(used)) if s.index() == used.index => node.mesh().is_some(),
                    (Some(_), _) => {
                        self.warnings.borrow_mut().push(format!(
                            "node {}: uses a second skin; only one skin per model is supported, \
                             so it is drawn unskinned",
                            node.index()
                        ));
                        false
                    }
                    (None, _) => false,
                };
                if let Some(mesh) = node.mesh() {
                    let name = node
                        .name()
                        .or(mesh.name())
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("node {}", node.index()));
                    instances.push((mesh, world, name, skinned));
                }
                for child in node.children() {
                    stack.push((child, world, depth + 1));
                }
            }
            // Stack order is reversed; keep document order for stable output.
            instances.reverse();
        }

        let mut parts = Vec::new();
        for (mesh, world, name, skinned) in instances {
            let remap = skin.filter(|_| skinned).map(|s| s.remap.as_slice());
            for (p, primitive) in mesh.primitives().enumerate() {
                let part_name = if mesh.primitives().len() > 1 {
                    format!("{name}.{p}")
                } else {
                    name.clone()
                };
                if let Some(cpu) = self.primitive(&primitive, world, &part_name, remap)? {
                    let material = match primitive.material().index() {
                        Some(i) if i < material_count => i,
                        Some(_) => {
                            return Err(self.malformed(&part_name, "material index out of range"));
                        }
                        None => material_count, // default material, appended below
                    };
                    parts.push(ModelPart {
                        name: part_name,
                        mesh: cpu,
                        material,
                        skinned: remap.is_some(),
                    });
                }
            }
        }
        Ok(parts)
    }

    /// `remap` (skin joint index → skeleton index) makes this a skinned
    /// primitive: vertices stay in bind space and carry joints/weights.
    fn primitive(
        &self,
        primitive: &gltf::Primitive<'_>,
        world: Mat4,
        name: &str,
        remap: Option<&[u16]>,
    ) -> Result<Option<CpuMesh>, ImportError> {
        // Skinned vertices are positioned by the joints, not the node.
        let world = if remap.is_some() {
            Mat4::IDENTITY
        } else {
            world
        };
        let mode = primitive.mode();
        if matches!(
            mode,
            Mode::Points | Mode::Lines | Mode::LineLoop | Mode::LineStrip
        ) {
            self.warnings.borrow_mut().push(format!(
                "{name}: {mode:?} primitives are not supported; skipped"
            ));
            return Ok(None);
        }
        if primitive.morph_targets().len() > 0 {
            self.warnings.borrow_mut().push(format!(
                "{name}: morph targets are ignored (base shape imported)"
            ));
        }
        for (semantic, _) in primitive.attributes() {
            match semantic {
                Semantic::Positions
                | Semantic::Normals
                | Semantic::TexCoords(0)
                | Semantic::Colors(0) => {}
                Semantic::Tangents => {}
                Semantic::Joints(0) | Semantic::Weights(0) if remap.is_some() => {}
                Semantic::Joints(0) | Semantic::Weights(0) => {}
                other => self
                    .warnings
                    .borrow_mut()
                    .push(format!("{name}: attribute {other:?} is ignored")),
            }
        }
        let Some(positions) = primitive.get(&Semantic::Positions) else {
            return Err(self.malformed(name, "primitive has no POSITION attribute"));
        };
        let positions = self.read_floats(&positions, &[Dimensions::Vec3], "POSITION")?;
        let count = positions.len();
        let check_len = |len: usize, what: &str| {
            if len == count {
                Ok(())
            } else {
                Err(ImportError::Malformed {
                    path: self.path.to_path_buf(),
                    what: format!("{name} {what}"),
                    message: format!("has {len} elements but POSITION has {count}"),
                })
            }
        };
        let normals = match primitive.get(&Semantic::Normals) {
            Some(a) => {
                let n = self.read_floats(&a, &[Dimensions::Vec3], "NORMAL")?;
                check_len(n.len(), "NORMAL")?;
                Some(n)
            }
            None => None,
        };
        let uvs = match primitive.get(&Semantic::TexCoords(0)) {
            Some(a) => {
                let n = self.read_floats(&a, &[Dimensions::Vec2], "TEXCOORD_0")?;
                check_len(n.len(), "TEXCOORD_0")?;
                Some(n)
            }
            None => None,
        };
        let colors = match primitive.get(&Semantic::Colors(0)) {
            Some(a) => {
                let n = self.read_floats(&a, &[Dimensions::Vec3, Dimensions::Vec4], "COLOR_0")?;
                check_len(n.len(), "COLOR_0")?;
                Some(n)
            }
            None => None,
        };
        let skin_data = match remap {
            Some(remap) => {
                let (Some(j), Some(w)) = (
                    primitive.get(&Semantic::Joints(0)),
                    primitive.get(&Semantic::Weights(0)),
                ) else {
                    return Err(self.malformed(
                        name,
                        "skinned primitive needs JOINTS_0 and WEIGHTS_0 attributes",
                    ));
                };
                let joints = self.read_joints(&j)?;
                check_len(joints.len(), "JOINTS_0")?;
                let weights = self.read_floats(&w, &[Dimensions::Vec4], "WEIGHTS_0")?;
                check_len(weights.len(), "WEIGHTS_0")?;
                let mut fixed = 0usize;
                let mut data = Vec::with_capacity(count);
                for (i, (js, ws)) in joints.iter().zip(&weights).enumerate() {
                    let mut out_j = [0u16; 4];
                    let mut out_w = [0.0f32; 4];
                    for c in 0..4 {
                        let w = ws[c].max(0.0);
                        if w == 0.0 {
                            continue;
                        }
                        let Some(&mapped) = remap.get(usize::from(js[c])) else {
                            return Err(self.malformed(
                                name,
                                format!(
                                    "vertex {i} uses joint {} but the skin has {}",
                                    js[c],
                                    remap.len()
                                ),
                            ));
                        };
                        out_j[c] = mapped;
                        out_w[c] = w;
                    }
                    let total: f32 = out_w.iter().sum();
                    if total > 1.0e-6 {
                        out_w = out_w.map(|w| w / total);
                    } else {
                        fixed += 1;
                        out_j = [remap.first().copied().unwrap_or(0), 0, 0, 0];
                        out_w = [1.0, 0.0, 0.0, 0.0];
                    }
                    data.push((out_j, out_w));
                }
                if fixed > 0 {
                    self.warnings.borrow_mut().push(format!(
                        "{name}: {fixed} vertices had no joint weight; bound to the first joint"
                    ));
                }
                if primitive.get(&Semantic::Joints(1)).is_some() {
                    self.warnings.borrow_mut().push(format!(
                        "{name}: JOINTS_1/WEIGHTS_1 are ignored (at most 4 joints per vertex)"
                    ));
                }
                Some(data)
            }
            None => {
                if primitive.get(&Semantic::Joints(0)).is_some() {
                    self.warnings.borrow_mut().push(format!(
                        "{name}: JOINTS_0/WEIGHTS_0 ignored (the mesh is not skinned by the model's skin)"
                    ));
                }
                None
            }
        };
        let raw_indices = match primitive.indices() {
            Some(a) => self.read_indices(&a)?,
            None => (0..count as u32).collect(),
        };
        if let Some((pos, &bad)) = raw_indices
            .iter()
            .enumerate()
            .find(|&(_, &i)| i as usize >= count)
        {
            return Err(self.malformed(
                name,
                format!("index {bad} at position {pos} is out of range for {count} vertices"),
            ));
        }
        let mut indices = triangulate(mode, &raw_indices);

        let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
        let mirrored = world.determinant() < 0.0;
        if mirrored {
            for tri in indices.chunks_exact_mut(3) {
                tri.swap(1, 2);
            }
        }
        let mut vertices: Vec<MeshVertex> = (0..count)
            .map(|i| {
                let p = world.transform_point3(Vec3::from_slice(&positions[i][..3]));
                let n = normals
                    .as_ref()
                    .map(|n| (normal_matrix * Vec3::from_slice(&n[i][..3])).normalize_or_zero())
                    .unwrap_or(Vec3::ZERO);
                let (joints, weights) = skin_data.as_ref().map_or(([0; 4], [0.0; 4]), |d| d[i]);
                MeshVertex {
                    position: p.to_array(),
                    normal: n.to_array(),
                    uv: uvs.as_ref().map_or([0.0, 0.0], |u| [u[i][0], u[i][1]]),
                    color: colors.as_ref().map_or([1.0; 4], |c| c[i]),
                    joints,
                    weights,
                }
            })
            .collect();
        if normals.is_none() {
            compute_normals(&mut vertices, &indices);
        }
        if vertices
            .iter()
            .any(|v| !v.position.iter().chain(&v.normal).all(|f| f.is_finite()))
        {
            return Err(self.malformed(name, "transformed vertices are not finite"));
        }
        if indices.is_empty() {
            self.warnings
                .borrow_mut()
                .push(format!("{name}: no triangles; skipped"));
            return Ok(None);
        }
        Ok(Some(CpuMesh { vertices, indices }))
    }

    fn warn_unsupported_features(&self, has_skin: bool) {
        if !has_skin && self.document.animations().len() > 0 {
            self.warnings.borrow_mut().push(
                "animations are ignored: only skinned (joint) animation is supported".to_owned(),
            );
        }
    }

    /// Parent node of every node that has one.
    fn parents(&self) -> HashMap<usize, usize> {
        let mut parents = HashMap::new();
        for node in self.document.nodes() {
            for child in node.children() {
                parents.insert(child.index(), node.index());
            }
        }
        parents
    }

    /// Rest-pose model-space transform of `node` (import scale included).
    fn node_global(
        &self,
        node: usize,
        parents: &HashMap<usize, usize>,
    ) -> Result<Mat4, ImportError> {
        let nodes: Vec<gltf::Node<'_>> = self.document.nodes().collect();
        let mut m = Mat4::IDENTITY;
        let mut current = Some(node);
        let mut depth = 0;
        while let Some(n) = current {
            depth += 1;
            if depth > MAX_NODE_DEPTH {
                return Err(self.malformed("node hierarchy", "is too deep or cyclic"));
            }
            let local = Mat4::from_cols_array_2d(&nodes[n].transform().matrix());
            m = local * m;
            current = parents.get(&n).copied();
        }
        Ok(Mat4::from_scale(Vec3::splat(self.settings.scale)) * m)
    }

    /// The skin used by the first skinned mesh node of the scene (or of the
    /// document), as a skeleton with parent-first joints.
    fn skin(&self) -> Result<Option<SkinImport>, ImportError> {
        let Some(skin) = self
            .document
            .nodes()
            .filter(|n| n.mesh().is_some())
            .find_map(|n| n.skin())
        else {
            return Ok(None);
        };
        let what = format!("skin {}", skin.index());
        let joint_nodes: Vec<usize> = skin.joints().map(|n| n.index()).collect();
        if joint_nodes.is_empty() {
            return Err(self.malformed(what, "has no joints"));
        }
        if joint_nodes.len() > MAX_SKIN_JOINTS {
            return Err(self.malformed(
                what,
                format!(
                    "has {} joints; at most {MAX_SKIN_JOINTS} are supported",
                    joint_nodes.len()
                ),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if !joint_nodes.iter().all(|n| seen.insert(*n)) {
            return Err(self.malformed(what, "lists a joint twice"));
        }
        let inverse_bind: Vec<Mat4> = match skin.inverse_bind_matrices() {
            Some(acc) => {
                if acc.dimensions() != Dimensions::Mat4 || acc.data_type() != DataType::F32 {
                    return Err(self.malformed(&what, "inverseBindMatrices must be float MAT4"));
                }
                let raw = self.read_raw(&acc, &what)?;
                if raw.len() != joint_nodes.len() {
                    return Err(self.malformed(
                        &what,
                        format!(
                            "has {} inverse bind matrices for {} joints",
                            raw.len(),
                            joint_nodes.len()
                        ),
                    ));
                }
                let mut out = Vec::with_capacity(raw.len());
                for b in raw {
                    let cols: [f32; 16] = std::array::from_fn(|i| {
                        f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
                    });
                    let m = Mat4::from_cols_array(&cols);
                    if !m.is_finite() {
                        return Err(self.malformed(&what, "inverse bind matrix is not finite"));
                    }
                    out.push(m);
                }
                out
            }
            None => vec![Mat4::IDENTITY; joint_nodes.len()],
        };

        let parents = self.parents();
        let depth_of = |mut n: usize| {
            let mut d = 0usize;
            while let Some(&p) = parents.get(&n) {
                d += 1;
                if d > MAX_NODE_DEPTH {
                    break;
                }
                n = p;
            }
            d
        };
        // Parent-first order: sort skin joints by depth in the node tree.
        let mut order: Vec<usize> = (0..joint_nodes.len()).collect();
        order.sort_by_key(|&i| depth_of(joint_nodes[i]));
        let mut remap = vec![0u16; joint_nodes.len()];
        for (ours, &skin_index) in order.iter().enumerate() {
            remap[skin_index] = ours as u16;
        }
        let node_to_joint: HashMap<usize, usize> = order
            .iter()
            .enumerate()
            .map(|(ours, &skin_index)| (joint_nodes[skin_index], ours))
            .collect();
        let nodes: Vec<gltf::Node<'_>> = self.document.nodes().collect();
        let mut joints = Vec::with_capacity(order.len());
        let mut detached = 0usize;
        for &skin_index in &order {
            let node = &nodes[joint_nodes[skin_index]];
            let (t, r, sc) = node.transform().decomposed();
            let rest = Trs {
                translation: Vec3::from(t),
                rotation: Quat::from_array(r).normalize(),
                scale: Vec3::from(sc),
            };
            if !(rest.translation.is_finite()
                && rest.rotation.is_finite()
                && rest.scale.is_finite())
            {
                return Err(
                    self.malformed(format!("node {}", node.index()), "transform is not finite")
                );
            }
            let parent_node = parents.get(&node.index()).copied();
            let parent = parent_node.and_then(|p| node_to_joint.get(&p).copied());
            let base = match (parent, parent_node) {
                (Some(_), _) => Mat4::IDENTITY,
                (None, Some(p)) => {
                    if self.has_joint_ancestor(p, &parents, &node_to_joint) {
                        detached += 1;
                    }
                    self.node_global(p, &parents)?
                }
                (None, None) => Mat4::from_scale(Vec3::splat(self.settings.scale)),
            };
            joints.push(Joint {
                name: node
                    .name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("joint {}", node.index())),
                parent,
                rest,
                base,
            });
        }
        if detached > 0 {
            self.warnings.borrow_mut().push(format!(
                "{what}: {detached} joints sit below non-joint nodes inside the skeleton; they \
                 follow their parent joints only in the rest pose"
            ));
        }
        let inverse_bind = order.iter().map(|&i| inverse_bind[i]).collect();
        Ok(Some(SkinImport {
            index: skin.index(),
            remap,
            node_to_joint,
            skeleton: Skeleton {
                joints,
                inverse_bind,
            },
        }))
    }

    fn has_joint_ancestor(
        &self,
        mut n: usize,
        parents: &HashMap<usize, usize>,
        joints: &HashMap<usize, usize>,
    ) -> bool {
        for _ in 0..MAX_NODE_DEPTH {
            if joints.contains_key(&n) {
                return true;
            }
            match parents.get(&n) {
                Some(&p) => n = p,
                None => return false,
            }
        }
        false
    }

    /// Animation clips whose channels target `skin`'s joints.
    fn clips(&self, skin: &SkinImport) -> Result<Vec<AnimationClip>, ImportError> {
        let mut clips: Vec<AnimationClip> = Vec::new();
        let mut cubic = false;
        let mut ignored = 0usize;
        for anim in self.document.animations() {
            let base_name = anim
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("animation {}", anim.index()));
            let mut name = base_name.clone();
            let mut n = 2;
            while clips.iter().any(|c| c.name == name) {
                name = format!("{base_name} ({n})");
                n += 1;
            }
            let what = format!("animation `{name}`");
            let mut channels = Vec::new();
            for channel in anim.channels() {
                let target = channel.target();
                let Some(&joint) = skin.node_to_joint.get(&target.node().index()) else {
                    ignored += 1;
                    continue;
                };
                let property = match target.property() {
                    gltf::animation::Property::Translation => ChannelProperty::Translation,
                    gltf::animation::Property::Rotation => ChannelProperty::Rotation,
                    gltf::animation::Property::Scale => ChannelProperty::Scale,
                    gltf::animation::Property::MorphTargetWeights => {
                        ignored += 1;
                        continue;
                    }
                };
                let sampler = channel.sampler();
                let times: Vec<f32> = self
                    .read_floats(&sampler.input(), &[Dimensions::Scalar], "animation input")?
                    .into_iter()
                    .map(|v| v[0])
                    .collect();
                if times.is_empty() {
                    return Err(self.malformed(&what, "a sampler has no keyframes"));
                }
                if times[0] < 0.0 || times.windows(2).any(|w| w[1] < w[0]) {
                    return Err(
                        self.malformed(&what, "keyframe times must be non-negative and increasing")
                    );
                }
                let dims: &[Dimensions] = match property {
                    ChannelProperty::Rotation => &[Dimensions::Vec4],
                    _ => &[Dimensions::Vec3],
                };
                let raw = self.read_floats(&sampler.output(), dims, "animation output")?;
                let (interpolation, values): (Interpolation, Vec<Vec4>) = match sampler
                    .interpolation()
                {
                    gltf::animation::Interpolation::Step => (
                        Interpolation::Step,
                        raw.into_iter().map(Vec4::from).collect(),
                    ),
                    gltf::animation::Interpolation::Linear => (
                        Interpolation::Linear,
                        raw.into_iter().map(Vec4::from).collect(),
                    ),
                    gltf::animation::Interpolation::CubicSpline => {
                        cubic = true;
                        if raw.len() != times.len() * 3 {
                            return Err(self.malformed(
                                    &what,
                                    format!(
                                        "cubic spline sampler has {} values for {} keys (expected 3 per key)",
                                        raw.len(),
                                        times.len()
                                    ),
                                ));
                        }
                        (
                            Interpolation::Linear,
                            raw.chunks_exact(3).map(|k| Vec4::from(k[1])).collect(),
                        )
                    }
                };
                if values.len() != times.len() {
                    return Err(self.malformed(
                        &what,
                        format!(
                            "sampler has {} values for {} keyframes",
                            values.len(),
                            times.len()
                        ),
                    ));
                }
                channels.push(Channel {
                    joint,
                    property,
                    interpolation,
                    times,
                    values,
                });
            }
            if channels.is_empty() {
                continue;
            }
            let duration = channels
                .iter()
                .filter_map(|c| c.times.last().copied())
                .fold(0.0f32, f32::max);
            clips.push(AnimationClip {
                name,
                duration,
                channels,
            });
        }
        if cubic {
            self.warnings.borrow_mut().push(
                "cubic spline animation keys are played back linearly (tangents ignored)".into(),
            );
        }
        if ignored > 0 {
            self.warnings.borrow_mut().push(format!(
                "{ignored} animation channels target nodes that are not joints of the skin \
                 (or morph weights) and are ignored"
            ));
        }
        Ok(clips)
    }
}

/// The skin chosen for a model.
struct SkinImport {
    /// glTF skin index.
    index: usize,
    /// glTF skin joint index → skeleton joint index.
    remap: Vec<u16>,
    /// glTF node index → skeleton joint index.
    node_to_joint: HashMap<usize, usize>,
    skeleton: Skeleton,
}

/// Converts strips and fans to a triangle list; drops a trailing partial
/// triangle. Strip winding alternates per the glTF specification.
fn triangulate(mode: Mode, idx: &[u32]) -> Vec<u32> {
    match mode {
        Mode::Triangles => idx[..idx.len() - idx.len() % 3].to_vec(),
        // glTF 2.0 §3.7.2.1: triangle t is (t, t + 1 + t%2, t + 2 - t%2).
        Mode::TriangleStrip => (0..idx.len().saturating_sub(2))
            .flat_map(|t| [idx[t], idx[t + 1 + t % 2], idx[t + 2 - t % 2]])
            .collect(),
        Mode::TriangleFan => (2..idx.len())
            .flat_map(|i| [idx[0], idx[i - 1], idx[i]])
            .collect(),
        _ => Vec::new(),
    }
}

/// Area-weighted vertex normals from the triangles.
fn compute_normals(vertices: &mut [MeshVertex], indices: &[u32]) {
    let mut acc = vec![Vec3::ZERO; vertices.len()];
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(vertices[tri[k] as usize].position));
        let n = (b - a).cross(c - a);
        for &k in tri {
            acc[k as usize] += n;
        }
    }
    for (v, n) in vertices.iter_mut().zip(acc) {
        v.normal = n.normalize_or(Vec3::Y).to_array();
    }
}

/// `Some(Ok((bytes, mime)))` for a base64 `data:` URI, `Some(Err)` for a
/// malformed one, `None` when `uri` is not a data URI.
#[allow(clippy::type_complexity)]
fn decode_data_uri(uri: &str) -> Option<Result<(Vec<u8>, Option<&str>), String>> {
    let rest = uri.strip_prefix("data:")?;
    let Some((header, payload)) = rest.split_once(',') else {
        return Some(Err("data URI has no payload".to_owned()));
    };
    let Some(mime) = header.strip_suffix(";base64") else {
        return Some(Err("only base64 data URIs are supported".to_owned()));
    };
    Some(
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map(|bytes| (bytes, (!mime.is_empty()).then_some(mime)))
            .map_err(|e| format!("invalid base64 data: {e}")),
    )
}

fn load_buffers(
    path: &Path,
    boundary: &Path,
    document: &Document,
    blob: Option<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, ImportError> {
    let mut blob = blob;
    let mut out = Vec::new();
    for buffer in document.buffers() {
        let data =
            match buffer.source() {
                gltf::buffer::Source::Bin => blob.take().ok_or_else(|| ImportError::Malformed {
                    path: path.to_path_buf(),
                    what: format!("buffer {}", buffer.index()),
                    message: "refers to the GLB binary chunk, which is missing".to_owned(),
                })?,
                gltf::buffer::Source::Uri(uri) => match decode_data_uri(uri) {
                    Some(result) => result.map(|(bytes, _)| bytes).map_err(|message| {
                        ImportError::Malformed {
                            path: path.to_path_buf(),
                            what: format!("buffer {}", buffer.index()),
                            message,
                        }
                    })?,
                    None => read_source(&resolve_reference(path, boundary, uri)?)?,
                },
            };
        if data.len() < buffer.length() {
            return Err(ImportError::Malformed {
                path: path.to_path_buf(),
                what: format!("buffer {}", buffer.index()),
                message: format!(
                    "declares {} bytes but only {} are present",
                    buffer.length(),
                    data.len()
                ),
            });
        }
        out.push(data);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
