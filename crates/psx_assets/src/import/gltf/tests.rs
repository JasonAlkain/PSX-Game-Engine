use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;
use crate::fixtures::Fixture;
use crate::import::texture::tests::png;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!(
            "psx_gltf_{name}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn import(dir: &Dir, file: &Path) -> Result<ImportedModel, ImportError> {
    import_gltf(file, &dir.0, &ImportSettings::default())
}

fn checker() -> Vec<u8> {
    png(4, 4, |x, y| {
        if (x + y) % 2 == 0 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 128]
        }
    })
}

#[test]
fn imports_textured_glb() {
    let dir = Dir::new("glb");
    let fx = Fixture::textured_box([0.5, 1.0, 0.25], [1.0, 0.5, 0.25, 1.0], Some(&checker()));
    let file = dir.write("box.glb", fx.to_glb());
    let model = import(&dir, &file).unwrap();
    assert_eq!(model.parts.len(), 1);
    assert_eq!(model.triangle_count(), 12);
    assert_eq!(model.textures.len(), 1);
    assert_eq!((model.textures[0].width, model.textures[0].height), (4, 4));
    assert_eq!(&model.textures[0].rgba[..4], &[255, 0, 0, 255]);
    let m = &model.materials[model.parts[0].material];
    assert_eq!(m.base_color, [1.0, 0.5, 0.25, 1.0]);
    assert_eq!(m.base_texture, Some(0));
    let (lo, hi) = model.bounds().unwrap();
    assert_eq!(lo, [-0.5, -1.0, -0.25]);
    assert_eq!(hi, [0.5, 1.0, 0.25]);
    assert!(model.warnings.is_empty(), "{:?}", model.warnings);
}

#[test]
fn imports_gltf_with_external_buffer_and_image_in_subfolders() {
    let dir = Dir::new("external");
    let mut fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    fx.json["images"] = json!([{"uri": "../textures/checker%20a.png"}]);
    fx.json["textures"] = json!([{"source": 0}]);
    fx.json["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": 0});
    dir.write("textures/checker a.png", checker());
    dir.write("models/bin/box.bin", &fx.bin);
    let file = dir.write("models/box.gltf", fx.to_gltf_external("bin/box.bin"));
    let model = import(&dir, &file).unwrap();
    assert_eq!(model.textures.len(), 1);
    assert_eq!(model.triangle_count(), 12);
}

#[test]
fn data_uri_buffers_and_scale_setting() {
    use base64::Engine as _;
    let dir = Dir::new("datauri");
    let fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    let mut doc = fx.json.clone();
    doc["buffers"][0]["uri"] = json!(format!(
        "data:application/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&fx.bin)
    ));
    let file = dir.write("box.gltf", serde_json::to_vec(&doc).unwrap());
    let settings = ImportSettings {
        scale: 2.0,
        ..Default::default()
    };
    let model = import_gltf(&file, &dir.0, &settings).unwrap();
    assert_eq!(model.bounds().unwrap().1, [2.0, 2.0, 2.0]);
}

#[test]
fn node_transforms_are_baked_and_mirroring_flips_winding() {
    let dir = Dir::new("mirror");
    let mut fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    fx.json["nodes"] = json!([
        {"name": "root", "translation": [10.0, 0.0, 0.0], "children": [1]},
        {"name": "box", "mesh": 0, "scale": [-1.0, 1.0, 1.0]}
    ]);
    let file = dir.write("m.glb", fx.to_glb());
    let model = import(&dir, &file).unwrap();
    let mesh = &model.parts[0].mesh;
    assert_eq!(model.parts[0].name, "box");
    let (lo, hi) = model.bounds().unwrap();
    assert_eq!((lo[0], hi[0]), (9.0, 11.0));
    for tri in mesh.indices.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(mesh.vertices[tri[i] as usize].position));
        let n = Vec3::from(mesh.vertices[tri[0] as usize].normal);
        assert!(
            (b - a).cross(c - a).dot(n) > 0.0,
            "winding must match normals"
        );
    }
}

#[test]
fn computes_normals_and_converts_strips() {
    let dir = Dir::new("strip");
    let mut fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    let prim = &mut fx.json["meshes"][0]["primitives"][0];
    prim["attributes"] = json!({"POSITION": 0});
    prim["mode"] = json!(5); // TRIANGLE_STRIP over the first 4 indices
    fx.json["accessors"][3]["count"] = json!(4);
    // Strip order for the first face: bottom-left, bottom-right, top-left, top-right.
    let off = fx.json["bufferViews"][3]["byteOffset"].as_u64().unwrap() as usize;
    for (k, i) in [0u16, 1, 3, 2].into_iter().enumerate() {
        fx.bin[off + 2 * k..off + 2 * k + 2].copy_from_slice(&i.to_le_bytes());
    }
    let file = dir.write("s.glb", fx.to_glb());
    let model = import(&dir, &file).unwrap();
    let mesh = &model.parts[0].mesh;
    assert_eq!(mesh.indices.len(), 6);
    for v in &mesh.vertices[..4] {
        assert_eq!(v.normal, [0.0, 0.0, 1.0]);
    }
}

#[test]
fn reports_ignored_features() {
    let dir = Dir::new("warn");
    let mut fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    fx.json["materials"][0]["pbrMetallicRoughness"]["metallicFactor"] = json!(0.0);
    fx.json["materials"][0]["emissiveFactor"] = json!([0.5, 0.0, 0.0]);
    fx.json["materials"][0]["alphaMode"] = json!("MASK");
    fx.json["materials"][0]["alphaCutoff"] = json!(0.3);
    fx.json["materials"][0]["doubleSided"] = json!(true);
    fx.json["extensionsUsed"] = json!(["KHR_materials_unlit", "KHR_texture_transform"]);
    fx.json["materials"][0]["extensions"] = json!({"KHR_materials_unlit": {}});
    fx.json["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .push(json!({"attributes": {"POSITION": 0}, "mode": 1}));
    let file = dir.write("w.glb", fx.to_glb());
    let model = import(&dir, &file).unwrap();
    let m = &model.materials[0];
    assert!(m.unlit && m.double_sided);
    assert_eq!(m.emissive, [0.5, 0.0, 0.0]);
    assert_eq!(m.alpha, AlphaSetting::Cutout { threshold: 0.3 });
    let w = model.warnings.join("\n");
    assert!(w.contains("metallic/roughness"), "{w}");
    assert!(w.contains("KHR_texture_transform"), "{w}");
    assert!(w.contains("Lines"), "{w}");
}

#[test]
fn missing_external_buffer_is_a_missing_dependency() {
    let dir = Dir::new("missing");
    let fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    let file = dir.write("m.gltf", fx.to_gltf_external("nowhere.bin"));
    match import(&dir, &file) {
        Err(ImportError::MissingDependency { dependency, .. }) => {
            assert!(dependency.ends_with("nowhere.bin"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn references_outside_the_project_are_rejected() {
    let outer = Dir::new("outer");
    let fx = Fixture::textured_box([1.0; 3], [1.0; 4], None);
    outer.write("secret.bin", &fx.bin);
    let file = outer.write("project/m.gltf", fx.to_gltf_external("../secret.bin"));
    let err = import_gltf(&file, &outer.0.join("project"), &ImportSettings::default()).unwrap_err();
    assert!(matches!(err, ImportError::OutsideProject { .. }), "{err}");
    let file = outer.write(
        "project/n.gltf",
        fx.to_gltf_external("https://example.com/a.bin"),
    );
    let err = import_gltf(&file, &outer.0.join("project"), &ImportSettings::default()).unwrap_err();
    assert!(matches!(err, ImportError::UnsupportedUri { .. }), "{err}");
}

#[test]
fn malformed_files_fail_with_context() {
    let dir = Dir::new("malformed");
    let base = Fixture::textured_box([1.0; 3], [1.0; 4], Some(&checker()));
    type Edit = fn(&mut Fixture);
    let cases: Vec<(&str, Edit, &str)> = vec![
        (
            "count",
            |f| f.json["accessors"][0]["count"] = json!(10_000),
            "exceed",
        ),
        (
            "index",
            |f| {
                let off = f.json["bufferViews"][3]["byteOffset"].as_u64().unwrap() as usize;
                f.bin[off..off + 2].copy_from_slice(&999u16.to_le_bytes());
            },
            "out of range",
        ),
        (
            "nan",
            |f| f.bin[0..4].copy_from_slice(&f32::NAN.to_le_bytes()),
            "non-finite",
        ),
        (
            "view",
            |f| f.json["bufferViews"][0]["byteLength"] = json!(1_000_000),
            "exceeds buffer",
        ),
        ("short", |f| f.bin.truncate(64), "declares"),
        (
            "type",
            |f| f.json["accessors"][0]["type"] = json!("VEC2"),
            "not supported",
        ),
        (
            "int",
            |f| f.json["accessors"][0]["componentType"] = json!(5123),
            "normalized",
        ),
        (
            "draco",
            |f| {
                f.json["extensionsUsed"] = json!(["KHR_draco_mesh_compression"]);
                f.json["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
            },
            "KHR_draco_mesh_compression",
        ),
        (
            "image",
            |f| {
                let off = f.json["bufferViews"][4]["byteOffset"].as_u64().unwrap() as usize;
                f.bin[off + 20] ^= 0xff;
                f.bin[off + 30] ^= 0xff;
            },
            "image",
        ),
        (
            "sparse",
            |f| {
                f.json["accessors"][0]["sparse"] = json!({
                    "count": 1,
                    "indices": {"bufferView": 3, "componentType": 5123},
                    "values": {"bufferView": 0}
                })
            },
            "sparse",
        ),
        (
            "noposition",
            |f| f.json["meshes"][0]["primitives"][0]["attributes"] = json!({"NORMAL": 1}),
            "POSITION",
        ),
    ];
    for (name, edit, expect) in cases {
        let mut f = base.clone();
        edit(&mut f);
        let file = dir.write(&format!("{name}.glb"), f.to_glb());
        let err = import(&dir, &file).expect_err(name);
        let text = err.to_string();
        assert!(text.contains(expect), "{name}: {text}");
        assert!(
            text.contains(&format!("{name}.glb")),
            "{name}: path missing in {text}"
        );
    }
    // Not glTF at all, and a truncated GLB.
    let file = dir.write("junk.glb", b"definitely not gltf");
    assert!(matches!(
        import(&dir, &file),
        Err(ImportError::Parse { .. })
    ));
    let glb = base.to_glb();
    let file = dir.write("cut.glb", &glb[..glb.len() / 3]);
    assert!(import(&dir, &file).is_err());
}

#[test]
fn every_truncation_fails_cleanly() {
    // No prefix of a valid file may panic or claim success with bad data.
    let dir = Dir::new("trunc");
    let glb = Fixture::textured_box([1.0; 3], [1.0; 4], Some(&checker())).to_glb();
    for len in (0..glb.len()).step_by(7) {
        let file = dir.write("t.glb", &glb[..len]);
        let result = import(&dir, &file);
        assert!(
            !matches!(result, Err(ImportError::Internal { .. })),
            "panic at length {len}: {result:?}"
        );
    }
}
