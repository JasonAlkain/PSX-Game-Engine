//! Static glTF 2.0 import (`.gltf` with external or embedded buffers, and
//! binary `.glb`).
//!
//! Supported: triangle lists, strips and fans; POSITION, NORMAL (computed
//! when absent), TEXCOORD_0, COLOR_0; u8/u16/u32 indices; node transforms
//! of the default scene (baked into the vertices, mirrored nodes get their
//! winding flipped); base colour factor and texture (PNG/JPEG, embedded or
//! external), alpha mode and cutoff, double-sided, emissive factor, and
//! `KHR_materials_unlit`.
//!
//! Reported and ignored (import still succeeds, with a warning): points and
//! lines, morph targets, skins and animations, UV1+, metallic/roughness,
//! normal/occlusion/emissive textures, texture transforms on non-required
//! extensions, samplers (the engine samples nearest without mipmaps).
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
use glam::{Mat3, Mat4, Vec3};
use gltf::accessor::{DataType, Dimensions};
use gltf::mesh::Mode;
use gltf::{Accessor, Document, Semantic};
use psx_scene::AlphaSetting;

use super::texture::decode_image;
use super::{
    CpuMesh, ImportError, ImportedMaterial, ImportedModel, MeshVertex, ModelPart, guarded,
    read_source, resolve_reference,
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
        let parts = importer.parts(materials.len())?;
        if parts.iter().any(|p| p.material == materials.len()) {
            materials.push(ImportedMaterial::default());
        }
        if parts.iter().all(|p| p.mesh.indices.is_empty()) {
            return Err(ImportError::Empty {
                path: path.to_path_buf(),
            });
        }
        importer.warn_unsupported_features();
        Ok(ImportedModel {
            parts,
            materials,
            textures,
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

    fn parts(&self, material_count: usize) -> Result<Vec<ModelPart>, ImportError> {
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
        let mut instances: Vec<(gltf::Mesh<'_>, Mat4, String)> = Vec::new();
        if roots.is_empty() {
            for mesh in self.document.meshes() {
                let name = mesh.name().unwrap_or("mesh").to_owned();
                instances.push((mesh, scale, name));
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
                if node.skin().is_some() {
                    self.warnings.borrow_mut().push(format!(
                        "node {}: skinning is not supported yet; drawn in bind pose",
                        node.index()
                    ));
                }
                if let Some(mesh) = node.mesh() {
                    let name = node
                        .name()
                        .or(mesh.name())
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("node {}", node.index()));
                    instances.push((mesh, world, name));
                }
                for child in node.children() {
                    stack.push((child, world, depth + 1));
                }
            }
            // Stack order is reversed; keep document order for stable output.
            instances.reverse();
        }

        let mut parts = Vec::new();
        for (mesh, world, name) in instances {
            for (p, primitive) in mesh.primitives().enumerate() {
                let part_name = if mesh.primitives().len() > 1 {
                    format!("{name}.{p}")
                } else {
                    name.clone()
                };
                if let Some(cpu) = self.primitive(&primitive, world, &part_name)? {
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
                    });
                }
            }
        }
        Ok(parts)
    }

    fn primitive(
        &self,
        primitive: &gltf::Primitive<'_>,
        world: Mat4,
        name: &str,
    ) -> Result<Option<CpuMesh>, ImportError> {
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
                MeshVertex {
                    position: p.to_array(),
                    normal: n.to_array(),
                    uv: uvs.as_ref().map_or([0.0, 0.0], |u| [u[i][0], u[i][1]]),
                    color: colors.as_ref().map_or([1.0; 4], |c| c[i]),
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

    fn warn_unsupported_features(&self) {
        if self.document.animations().len() > 0 {
            self.warnings
                .borrow_mut()
                .push("animations are not supported yet; ignored".to_owned());
        }
        if self.document.skins().len() > 0
            && !self
                .warnings
                .borrow()
                .iter()
                .any(|w| w.contains("skinning"))
        {
            self.warnings
                .borrow_mut()
                .push("skins are not supported yet; ignored".to_owned());
        }
    }
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
