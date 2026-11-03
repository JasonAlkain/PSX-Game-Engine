//! GPU resources for an authored scene, with missing-asset fallbacks.
//!
//! [`SceneResources`] uploads what a [`Scene`] references (built-in shapes,
//! imported models, textures) and turns the scene into draw items. It is
//! shared by the editor viewport and (from Milestone 4) the game runtime.
//!
//! Failure policy:
//! - An asset that is missing from the database, or fails to import and has
//!   never loaded, draws as a magenta/black checker cube (models) or a
//!   magenta/black checker texture (textures), and its status says why.
//! - A reimport that fails keeps the last working version on the GPU and
//!   records the error, so a broken file on disk never blanks the scene.
//!
//! Each loaded mesh also keeps its triangle positions on the CPU for
//! picking ([`SceneResources::pick`]).

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use psx_assets::project::{AssetDatabase, ImportedAsset, Project};
use psx_assets::{CpuMesh, CpuTexture, ImportedModel};
use psx_core::{AssetId, EntityId};
use psx_render::{
    AlphaMode, DrawItem, GpuMesh, GpuTexture, Material, MeshData, RetroRenderer, TextureData,
    Vertex,
};
use psx_scene::{AlphaSetting, MaterialOverride, MeshComponent, MeshSource, PrimitiveShape, Scene};

/// Positions and triangles kept for ray picking (local space).
#[derive(Debug, Clone, Default)]
pub struct PickMesh {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    pub min: Vec3,
    pub max: Vec3,
}

impl PickMesh {
    fn new(positions: Vec<Vec3>, indices: Vec<u32>) -> Self {
        let min = positions
            .iter()
            .copied()
            .fold(Vec3::splat(f32::MAX), Vec3::min);
        let max = positions
            .iter()
            .copied()
            .fold(Vec3::splat(f32::MIN), Vec3::max);
        Self {
            positions,
            indices,
            min,
            max,
        }
    }

    /// Nearest hit distance along a local-space ray (Möller–Trumbore,
    /// both faces).
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        if !ray_aabb(origin, dir, self.min, self.max) {
            return None;
        }
        let mut best: Option<f32> = None;
        for tri in self.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| self.positions[tri[k] as usize]);
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / det;
            let s = origin - a;
            let u = s.dot(p) * inv;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = dir.dot(q) * inv;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let t = e2.dot(q) * inv;
            if t > 0.0 && best.is_none_or(|b| t < b) {
                best = Some(t);
            }
        }
        best
    }
}

fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> bool {
    let inv = dir.recip();
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    // NaN (a zero direction component on a slab edge) counts as a hit.
    tmax.partial_cmp(&tmin.max(0.0)) != Some(std::cmp::Ordering::Less)
}

struct Part {
    mesh: GpuMesh,
    pick: Arc<PickMesh>,
    triangles: u32,
    texture: Option<usize>,
    material: Material,
}

struct LoadedModel {
    parts: Vec<Part>,
    textures: Vec<GpuTexture>,
}

/// Import state of one referenced asset.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetStatus {
    Loaded {
        warnings: Vec<String>,
    },
    /// The last import failed. `kept_previous` says whether an earlier
    /// working version is still drawn.
    Failed {
        error: String,
        kept_previous: bool,
    },
    /// Not in the asset database.
    Missing,
}

/// One mesh to draw, borrowed from [`SceneResources`].
pub struct SceneDraw<'a> {
    pub entity: EntityId,
    pub item: DrawItem<'a>,
    pub triangles: u32,
}

pub struct SceneResources {
    shapes: HashMap<PrimitiveShape, (GpuMesh, Arc<PickMesh>, u32)>,
    checker: GpuTexture,
    /// For model parts without a base texture (colour comes from the factor).
    white: GpuTexture,
    missing_texture: GpuTexture,
    models: HashMap<AssetId, LoadedModel>,
    textures: HashMap<AssetId, GpuTexture>,
    status: HashMap<AssetId, AssetStatus>,
    /// Owned materials for draw items (index-stable during one frame).
    scratch_materials: Vec<Material>,
}

/// Unit shapes as described by [`PrimitiveShape`].
pub fn primitive_mesh(shape: PrimitiveShape) -> MeshData {
    match shape {
        PrimitiveShape::Cube => MeshData::cube(1.0).with_color([1.0; 4]),
        PrimitiveShape::Plane => MeshData::plane(1.0, 1.0, (4, 4), (1.0, 1.0)),
        PrimitiveShape::Sphere => MeshData::sphere(0.5, 16, 10),
        PrimitiveShape::Quad => MeshData::quad(1.0, 1.0),
    }
}

fn to_mesh_data(mesh: &CpuMesh) -> MeshData {
    MeshData {
        vertices: mesh
            .vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                color: v.color,
            })
            .collect(),
        indices: mesh.indices.clone(),
    }
}

fn pick_of(mesh: &MeshData) -> PickMesh {
    PickMesh::new(
        mesh.vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .collect(),
        mesh.indices.clone(),
    )
}

fn to_texture_data(t: &CpuTexture) -> TextureData {
    TextureData {
        width: t.width,
        height: t.height,
        rgba: t.rgba.clone(),
    }
}

fn alpha_mode(a: AlphaSetting) -> AlphaMode {
    match a {
        AlphaSetting::Opaque => AlphaMode::Opaque,
        AlphaSetting::Cutout { threshold } => AlphaMode::Cutout { threshold },
        AlphaSetting::Blend => AlphaMode::Blend,
    }
}

/// Applies a scene material override on top of a source material.
pub fn apply_override(mut m: Material, o: &MaterialOverride) -> Material {
    if let Some(t) = o.tint {
        for (c, t) in m.tint.iter_mut().zip(t) {
            *c *= t;
        }
    }
    if let Some(a) = o.alpha {
        m.alpha = alpha_mode(a);
    }
    m.lit = o.lit.unwrap_or(m.lit);
    m.flat_shading = o.flat_shading.unwrap_or(m.flat_shading);
    m.allow_affine = o.allow_affine.unwrap_or(m.allow_affine);
    m.double_sided = o.double_sided.unwrap_or(m.double_sided);
    m
}

impl SceneResources {
    pub fn new(renderer: &RetroRenderer, device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut shapes = HashMap::new();
        for shape in PrimitiveShape::ALL {
            let data = primitive_mesh(shape);
            // Built-in shapes are valid by construction (unit tested).
            if let Ok(mesh) = renderer.upload_mesh(device, &data) {
                let tris = (data.indices.len() / 3) as u32;
                shapes.insert(shape, (mesh, Arc::new(pick_of(&data)), tris));
            }
        }
        let upload = |t: &TextureData| {
            renderer
                .upload_texture(device, queue, t)
                .expect("built-in 64x64 texture is within every device limit")
        };
        let checker = upload(&TextureData::checkerboard(
            64,
            8,
            [200, 200, 200, 255],
            [120, 120, 130, 255],
        ));
        let white = upload(&TextureData {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
        });
        let missing_texture = upload(&TextureData::checkerboard(
            16,
            2,
            [255, 0, 255, 255],
            [0, 0, 0, 255],
        ));
        Self {
            shapes,
            checker,
            white,
            missing_texture,
            models: HashMap::new(),
            textures: HashMap::new(),
            status: HashMap::new(),
            scratch_materials: Vec::new(),
        }
    }

    pub fn status(&self, id: AssetId) -> Option<&AssetStatus> {
        self.status.get(&id)
    }

    /// Loads every asset the scene references that has not been tried yet.
    /// Returns messages for newly failed or missing assets.
    pub fn sync(
        &mut self,
        project: &Project,
        db: &AssetDatabase,
        scene: &Scene,
        renderer: &RetroRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<String> {
        self.sync_meshes(
            project,
            db,
            scene.entities.iter().filter_map(|e| e.mesh.as_ref()),
            renderer,
            device,
            queue,
        )
    }

    /// [`SceneResources::sync`] for an explicit set of mesh components.
    pub fn sync_meshes<'m>(
        &mut self,
        project: &Project,
        db: &AssetDatabase,
        meshes: impl IntoIterator<Item = &'m MeshComponent>,
        renderer: &RetroRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<String> {
        let mut messages = Vec::new();
        let refs: Vec<AssetId> = meshes
            .into_iter()
            .flat_map(|m| m.source.asset_refs())
            .collect();
        for asset in refs {
            if self.status.contains_key(&asset) {
                continue;
            }
            if let Some(msg) = self.load(project, db, asset, renderer, device, queue) {
                messages.push(msg);
            }
        }
        messages
    }

    /// (Re)imports one asset. On failure the previous version, if any, stays
    /// in use. Returns an error message on failure.
    pub fn load(
        &mut self,
        project: &Project,
        db: &AssetDatabase,
        asset: AssetId,
        renderer: &RetroRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<String> {
        let Some(entry) = db.get(asset) else {
            self.status.insert(asset, AssetStatus::Missing);
            return Some(format!(
                "asset {asset} is not in the project; drawing a fallback"
            ));
        };
        let result = AssetDatabase::import(project, entry)
            .map_err(|e| e.to_string())
            .and_then(|imported| match imported {
                ImportedAsset::Model(model) => {
                    let warnings = model.warnings.clone();
                    self.upload_model(&model, renderer, device, queue)
                        .map(|m| {
                            self.models.insert(asset, m);
                            self.textures.remove(&asset);
                            warnings
                        })
                        .map_err(|e| format!("{}: {e}", entry.relative))
                }
                ImportedAsset::Texture(t) => renderer
                    .upload_texture(device, queue, &to_texture_data(&t))
                    .map(|t| {
                        self.textures.insert(asset, t);
                        self.models.remove(&asset);
                        Vec::new()
                    })
                    .map_err(|e| format!("{}: {e}", entry.relative)),
                ImportedAsset::Audio(_) => Err(format!(
                    "{} is a sound, not a model or texture",
                    entry.relative
                )),
            });
        match result {
            Ok(warnings) => {
                self.status.insert(asset, AssetStatus::Loaded { warnings });
                None
            }
            Err(error) => {
                let kept_previous =
                    self.models.contains_key(&asset) || self.textures.contains_key(&asset);
                let msg = if kept_previous {
                    format!("{error} (keeping the last working import)")
                } else {
                    format!("{error} (drawing a fallback)")
                };
                self.status.insert(
                    asset,
                    AssetStatus::Failed {
                        error,
                        kept_previous,
                    },
                );
                Some(msg)
            }
        }
    }

    /// Forgets the status of assets so the next [`SceneResources::sync`]
    /// retries them (e.g. after the asset database was rescanned). Loaded
    /// GPU data is kept until a reimport replaces it.
    pub fn retry_unloaded(&mut self) {
        self.status
            .retain(|_, s| matches!(s, AssetStatus::Loaded { .. }));
    }

    fn upload_model(
        &self,
        model: &ImportedModel,
        renderer: &RetroRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<LoadedModel, String> {
        let textures = model
            .textures
            .iter()
            .map(|t| {
                renderer
                    .upload_texture(device, queue, &to_texture_data(t))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut parts = Vec::new();
        for part in &model.parts {
            let data = to_mesh_data(&part.mesh);
            let mesh = renderer
                .upload_mesh(device, &data)
                .map_err(|e| format!("mesh {}: {e}", part.name))?;
            let src = model
                .materials
                .get(part.material)
                .cloned()
                .unwrap_or_default();
            parts.push(Part {
                mesh,
                pick: Arc::new(pick_of(&data)),
                triangles: (data.indices.len() / 3) as u32,
                texture: src.base_texture,
                material: Material {
                    tint: src.base_color,
                    alpha: alpha_mode(src.alpha),
                    lit: !src.unlit,
                    flat_shading: false,
                    allow_affine: true,
                    double_sided: src.double_sided,
                    emissive: src.emissive,
                },
            });
        }
        Ok(LoadedModel { parts, textures })
    }

    /// Draw items for every entity with a mesh, using `world` matrices
    /// (from [`Scene::world_matrices`]; entities missing there are skipped).
    pub fn draws<'a>(
        &'a mut self,
        scene: &Scene,
        world: &HashMap<EntityId, Mat4>,
    ) -> Vec<SceneDraw<'a>> {
        let items = scene.entities.iter().filter_map(|e| {
            let (Some(mesh), Some(&model)) = (&e.mesh, world.get(&e.id)) else {
                return None;
            };
            Some((e.id, mesh, model))
        });
        self.draws_from(items)
    }

    /// Draw items for explicit `(entity, mesh, world matrix)` triples (the
    /// game runtime, whose entities live in an ECS rather than a `Scene`).
    pub fn draws_from<'a, 'm>(
        &'a mut self,
        items: impl IntoIterator<Item = (EntityId, &'m MeshComponent, Mat4)>,
    ) -> Vec<SceneDraw<'a>> {
        // Materials are computed first so draw items can borrow them.
        let mut plan: Vec<(EntityId, DrawRef, Mat4, usize)> = Vec::new();
        self.scratch_materials.clear();
        for (id, mesh, model) in items {
            let e_id = id;
            match &mesh.source {
                MeshSource::Primitive { shape, texture } => {
                    let tex = match texture {
                        None => TexRef::Checker,
                        Some(id) if self.textures.contains_key(id) => TexRef::Asset(*id),
                        Some(_) => TexRef::Missing,
                    };
                    let material = match tex {
                        TexRef::Missing => Material {
                            lit: false,
                            ..Material::default()
                        },
                        _ => apply_override(Material::default(), &mesh.material),
                    };
                    self.scratch_materials.push(material);
                    plan.push((
                        e_id,
                        DrawRef::Shape(*shape, tex),
                        model,
                        self.scratch_materials.len() - 1,
                    ));
                }
                MeshSource::Model { asset } => match self.models.get(asset) {
                    Some(m) => {
                        for (i, part) in m.parts.iter().enumerate() {
                            self.scratch_materials
                                .push(apply_override(part.material, &mesh.material));
                            plan.push((
                                e_id,
                                DrawRef::Part(*asset, i),
                                model,
                                self.scratch_materials.len() - 1,
                            ));
                        }
                    }
                    None => {
                        // Unlit so the fallback reads clearly in any lighting;
                        // overrides are ignored for the same reason.
                        self.scratch_materials.push(Material {
                            lit: false,
                            ..Material::default()
                        });
                        plan.push((
                            e_id,
                            DrawRef::Shape(PrimitiveShape::Cube, TexRef::Missing),
                            model,
                            self.scratch_materials.len() - 1,
                        ));
                    }
                },
            }
        }
        let this = &*self;
        plan.into_iter()
            .filter_map(|(entity, r, model, mat)| {
                let material = &this.scratch_materials[mat];
                let (mesh, texture, triangles) = match r {
                    DrawRef::Shape(shape, tex) => {
                        let (mesh, _, tris) = this.shapes.get(&shape)?;
                        let texture = match tex {
                            TexRef::Checker => &this.checker,
                            TexRef::Missing => &this.missing_texture,
                            TexRef::Asset(id) => this.textures.get(&id)?,
                        };
                        (mesh, texture, *tris)
                    }
                    DrawRef::Part(asset, i) => {
                        let m = this.models.get(&asset)?;
                        let part = &m.parts[i];
                        let texture = part
                            .texture
                            .and_then(|t| m.textures.get(t))
                            .unwrap_or(&this.white);
                        (&part.mesh, texture, part.triangles)
                    }
                };
                Some(SceneDraw {
                    entity,
                    item: DrawItem {
                        mesh,
                        texture,
                        material,
                        model,
                    },
                    triangles,
                })
            })
            .collect()
    }

    /// Pick meshes (local space) used to draw `entity`'s mesh component.
    pub fn pick_meshes(&self, scene: &Scene, entity: EntityId) -> Vec<Arc<PickMesh>> {
        let Some(mesh) = scene.get(entity).and_then(|e| e.mesh.as_ref()) else {
            return Vec::new();
        };
        match &mesh.source {
            MeshSource::Primitive { shape, .. } => self
                .shapes
                .get(shape)
                .map(|s| s.1.clone())
                .into_iter()
                .collect(),
            MeshSource::Model { asset } => match self.models.get(asset) {
                Some(m) => m.parts.iter().map(|p| p.pick.clone()).collect(),
                None => self
                    .shapes
                    .get(&PrimitiveShape::Cube)
                    .map(|s| s.1.clone())
                    .into_iter()
                    .collect(),
            },
        }
    }

    /// The entity whose mesh a world-space ray hits first, with the distance.
    pub fn pick(
        &self,
        scene: &Scene,
        world: &HashMap<EntityId, Mat4>,
        origin: Vec3,
        dir: Vec3,
    ) -> Option<(EntityId, f32)> {
        let mut best: Option<(EntityId, f32)> = None;
        for e in &scene.entities {
            let Some(m) = world.get(&e.id) else { continue };
            if m.determinant().abs() < 1e-12 {
                continue;
            }
            let inv = m.inverse();
            let lo = inv.transform_point3(origin);
            let ld = inv.transform_vector3(dir);
            for pick in self.pick_meshes(scene, e.id) {
                if let Some(t) = pick.raycast(lo, ld) {
                    // t is in units of `ld`; measure the hit in world space.
                    let hit = m.transform_point3(lo + ld * t);
                    let d = hit.distance(origin);
                    if best.is_none_or(|(_, b)| d < b) {
                        best = Some((e.id, d));
                    }
                }
            }
        }
        best
    }

    /// Local-space bounds of `entity`'s mesh, if it has one.
    pub fn local_bounds(&self, scene: &Scene, entity: EntityId) -> Option<(Vec3, Vec3)> {
        let meshes = self.pick_meshes(scene, entity);
        let mut it = meshes.iter().filter(|m| !m.positions.is_empty());
        let first = it.next()?;
        let (mut lo, mut hi) = (first.min, first.max);
        for m in it {
            lo = lo.min(m.min);
            hi = hi.max(m.max);
        }
        Some((lo, hi))
    }
}

#[derive(Clone, Copy)]
enum TexRef {
    Checker,
    Missing,
    Asset(AssetId),
}

#[derive(Clone, Copy)]
enum DrawRef {
    Shape(PrimitiveShape, TexRef),
    Part(AssetId, usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_are_valid_unit_shapes() {
        for shape in PrimitiveShape::ALL {
            let m = primitive_mesh(shape);
            assert_eq!(m.validate(), Ok(()), "{shape:?}");
            let p = pick_of(&m);
            assert!(p.max.max_element() <= 0.5 + 1e-5 && p.min.min_element() >= -0.5 - 1e-5);
        }
    }

    #[test]
    fn raycast_hits_nearest_face() {
        let p = pick_of(&primitive_mesh(PrimitiveShape::Cube));
        let t = p.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        assert!((t - 4.5).abs() < 1e-5);
        assert!(p.raycast(Vec3::new(2.0, 0.0, 5.0), Vec3::NEG_Z).is_none());
        assert!(p.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::Z).is_none());
    }

    #[test]
    fn overrides_replace_and_tint_multiplies() {
        let base = Material {
            tint: [0.5, 1.0, 1.0, 1.0],
            ..Material::default()
        };
        let o = MaterialOverride {
            tint: Some([0.5, 0.5, 1.0, 1.0]),
            alpha: Some(AlphaSetting::Cutout { threshold: 0.25 }),
            lit: Some(false),
            ..Default::default()
        };
        let m = apply_override(base, &o);
        assert_eq!(m.tint, [0.25, 0.5, 1.0, 1.0]);
        assert_eq!(m.alpha, AlphaMode::Cutout { threshold: 0.25 });
        assert!(!m.lit && !m.double_sided);
    }
}
