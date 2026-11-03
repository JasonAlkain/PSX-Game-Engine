//! Small, valid glTF files built in code, for tests and example projects.
//!
//! [`Fixture::textured_box`] produces a box with normals, UVs, u16 indices,
//! one material and (optionally) an embedded PNG. Tests edit
//! [`Fixture::json`] to produce malformed variants.

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Fixture {
    /// The glTF JSON document. Buffer 0 is the binary payload.
    pub json: Value,
    /// Binary payload of buffer 0.
    pub bin: Vec<u8>,
}

fn push_f32s(bin: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) -> (usize, usize) {
    let start = bin.len();
    for v in values {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    (start, bin.len() - start)
}

fn pad4(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}

impl Fixture {
    /// A box with half extents `half` centred on the origin, base colour
    /// `color` (linear RGBA) and an optional embedded PNG base texture.
    pub fn textured_box(half: [f32; 3], color: [f32; 4], png: Option<&[u8]>) -> Fixture {
        // (normal, right, up) per face, right × up = normal (CCW front).
        let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([0., 0., 1.], [1., 0., 0.], [0., 1., 0.]),
            ([0., 0., -1.], [-1., 0., 0.], [0., 1., 0.]),
            ([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]),
            ([-1., 0., 0.], [0., 0., 1.], [0., 1., 0.]),
            ([0., 1., 0.], [1., 0., 0.], [0., 0., -1.]),
            ([0., -1., 0.], [1., 0., 0.], [0., 0., 1.]),
        ];
        let (mut pos, mut nrm, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
        for (n, r, u) in faces {
            let base = (pos.len() / 3) as u16;
            for (sx, sy, t) in [
                (-1.0, -1.0, [0.0, 1.0]),
                (1.0, -1.0, [1.0, 1.0]),
                (1.0, 1.0, [1.0, 0.0]),
                (-1.0, 1.0, [0.0, 0.0]),
            ] {
                for i in 0..3 {
                    pos.push((n[i] + sx * r[i] + sy * u[i]) * half[i]);
                }
                nrm.extend_from_slice(&n);
                uv.extend_from_slice(&t);
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mut bin = Vec::new();
        let (p_off, p_len) = push_f32s(&mut bin, pos.iter().copied());
        let (n_off, n_len) = push_f32s(&mut bin, nrm.iter().copied());
        let (u_off, u_len) = push_f32s(&mut bin, uv.iter().copied());
        let i_off = bin.len();
        for i in &idx {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let i_len = bin.len() - i_off;
        pad4(&mut bin);
        let mut views = vec![
            json!({"buffer": 0, "byteOffset": p_off, "byteLength": p_len}),
            json!({"buffer": 0, "byteOffset": n_off, "byteLength": n_len}),
            json!({"buffer": 0, "byteOffset": u_off, "byteLength": u_len}),
            json!({"buffer": 0, "byteOffset": i_off, "byteLength": i_len}),
        ];
        let mut material = json!({
            "name": "box",
            "pbrMetallicRoughness": {"baseColorFactor": color}
        });
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "psx_assets fixtures"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "box", "mesh": 0}],
            "meshes": [{"name": "box", "primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
                "indices": 3,
                "material": 0
            }]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 24, "type": "VEC3",
                 "min": [-half[0], -half[1], -half[2]], "max": half},
                {"bufferView": 1, "componentType": 5126, "count": 24, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5126, "count": 24, "type": "VEC2"},
                {"bufferView": 3, "componentType": 5123, "count": 36, "type": "SCALAR"}
            ]
        });
        if let Some(png) = png {
            let off = bin.len();
            bin.extend_from_slice(png);
            views.push(json!({"buffer": 0, "byteOffset": off, "byteLength": png.len()}));
            pad4(&mut bin);
            doc["images"] = json!([{"bufferView": 4, "mimeType": "image/png"}]);
            doc["samplers"] = json!([{"magFilter": 9728, "minFilter": 9728}]);
            doc["textures"] = json!([{"source": 0, "sampler": 0}]);
            material["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": 0});
        }
        doc["bufferViews"] = Value::Array(views);
        doc["materials"] = json!([material]);
        doc["buffers"] = json!([{"byteLength": bin.len()}]);
        Fixture { json: doc, bin }
    }

    /// Binary glTF (GLB) with the payload in the BIN chunk.
    pub fn to_glb(&self) -> Vec<u8> {
        let mut json_bytes = serde_json::to_vec(&self.json).unwrap_or_default();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let mut bin = self.bin.clone();
        pad4(&mut bin);
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    /// `.gltf` JSON text whose buffer 0 is the external file `bin_uri`.
    pub fn to_gltf_external(&self, bin_uri: &str) -> String {
        let mut doc = self.json.clone();
        doc["buffers"][0]["uri"] = json!(bin_uri);
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }
}
