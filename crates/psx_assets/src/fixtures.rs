//! Small, valid glTF files built in code, for tests and example projects.
//!
//! [`Fixture::textured_box`] produces a box with normals, UVs, u16 indices,
//! one material and (optionally) an embedded PNG. [`Fixture::skinned_arm`]
//! produces a two-joint skinned bar with two animation clips.
//! [`Fixture::rigged_boxes`] builds a skinned character from coloured
//! boxes and rotation clips (the example project's caretaker). Tests edit
//! [`Fixture::json`] to produce malformed variants.

use serde_json::{Value, json};

/// One joint of [`Fixture::rigged_boxes`] with the boxes it moves.
#[derive(Debug, Clone)]
pub struct RigBone {
    pub name: &'static str,
    /// Index of an earlier bone, or `None` for the root.
    pub parent: Option<usize>,
    /// Joint position relative to the parent joint (the root's is relative
    /// to the model origin), in the rest pose.
    pub offset: [f32; 3],
    /// Boxes rigidly attached to this joint: centre relative to the joint,
    /// half extents, and linear RGB vertex colour.
    pub boxes: Vec<([f32; 3], [f32; 3], [f32; 3])>,
}

/// A clip of [`Fixture::rigged_boxes`]: linear rotation keys per bone.
#[derive(Debug, Clone)]
pub struct RigClip {
    pub name: &'static str,
    /// `(bone index, key times in seconds, unit quaternions [x, y, z, w])`.
    pub tracks: Vec<(usize, Vec<f32>, Vec<[f32; 4]>)>,
}

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

    /// A skinned bar 0.4 wide and 2 tall: the lower half (y 0..1) follows
    /// joint `lower`, the upper half (y 1..2) joint `upper`, a child of
    /// `lower` one meter up. Nodes: 0 `Armature` (plain root), 1 `lower`,
    /// 2 `upper`, 3 `bar` (the skinned mesh). The skin lists its joints
    /// child first (`[upper, lower]`) so importers must reorder them.
    ///
    /// Clips: `bend` (1 s, linear) rotates `upper` from 0° to 90° about +Z;
    /// `lift` (0.5 s, step) raises `lower` by 0.5 m at 0.5 s.
    pub fn skinned_arm() -> Fixture {
        let mut pos: Vec<f32> = Vec::new();
        let mut joints: Vec<u8> = Vec::new();
        let mut weights: Vec<f32> = Vec::new();
        let mut idx: Vec<u16> = Vec::new();
        // Box corners per segment; skin joint 1 = lower, 0 = upper.
        for (segment, skin_joint) in [(0.0f32, 1u8), (1.0, 0u8)] {
            let base = (pos.len() / 3) as u16;
            for &(x, y, z) in &[
                (-0.2, 0.0, -0.2),
                (0.2, 0.0, -0.2),
                (0.2, 0.0, 0.2),
                (-0.2, 0.0, 0.2),
                (-0.2, 1.0, -0.2),
                (0.2, 1.0, -0.2),
                (0.2, 1.0, 0.2),
                (-0.2, 1.0, 0.2),
            ] {
                pos.extend_from_slice(&[x, y + segment, z]);
                joints.extend_from_slice(&[skin_joint, 0, 0, 0]);
                weights.extend_from_slice(&[1.0, 0.0, 0.0, 0.0]);
            }
            for f in [
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ] {
                idx.extend(f.iter().map(|i| base + i));
            }
        }
        let mut bin = Vec::new();
        let (p_off, p_len) = push_f32s(&mut bin, pos.iter().copied());
        let j_off = bin.len();
        bin.extend_from_slice(&joints);
        let j_len = bin.len() - j_off;
        let (w_off, w_len) = push_f32s(&mut bin, weights.iter().copied());
        let i_off = bin.len();
        for i in &idx {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let i_len = bin.len() - i_off;
        pad4(&mut bin);
        // Inverse bind matrices in skin order: upper (1 m up), lower.
        let mut upper_ibm = [0.0f32; 16];
        for i in 0..4 {
            upper_ibm[i * 5] = 1.0;
        }
        upper_ibm[13] = -1.0;
        let mut identity = [0.0f32; 16];
        for i in 0..4 {
            identity[i * 5] = 1.0;
        }
        let (m_off, m_len) = push_f32s(&mut bin, upper_ibm.into_iter().chain(identity));
        let (t_off, t_len) = push_f32s(&mut bin, [0.0, 1.0]);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let (r_off, r_len) = push_f32s(&mut bin, [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, s]);
        let (lt_off, lt_len) = push_f32s(&mut bin, [0.0, 0.5]);
        let (lv_off, lv_len) = push_f32s(&mut bin, [0.0, 0.0, 0.0, 0.0, 0.5, 0.0]);
        let doc = json!({
            "asset": {"version": "2.0", "generator": "psx_assets fixtures"},
            "scene": 0,
            "scenes": [{"nodes": [0, 3]}],
            "nodes": [
                {"name": "Armature", "children": [1]},
                {"name": "lower", "children": [2]},
                {"name": "upper", "translation": [0.0, 1.0, 0.0]},
                {"name": "bar", "mesh": 0, "skin": 0}
            ],
            "skins": [{"joints": [2, 1], "inverseBindMatrices": 4, "skeleton": 0}],
            "meshes": [{"name": "bar", "primitives": [{
                "attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2},
                "indices": 3
            }]}],
            "animations": [
                {"name": "bend",
                 "samplers": [{"input": 5, "output": 6, "interpolation": "LINEAR"}],
                 "channels": [{"sampler": 0, "target": {"node": 2, "path": "rotation"}}]},
                {"name": "lift",
                 "samplers": [{"input": 7, "output": 8, "interpolation": "STEP"}],
                 "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}]}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 16, "type": "VEC3",
                 "min": [-0.2, 0.0, -0.2], "max": [0.2, 2.0, 0.2]},
                {"bufferView": 1, "componentType": 5121, "count": 16, "type": "VEC4"},
                {"bufferView": 2, "componentType": 5126, "count": 16, "type": "VEC4"},
                {"bufferView": 3, "componentType": 5123, "count": 72, "type": "SCALAR"},
                {"bufferView": 4, "componentType": 5126, "count": 2, "type": "MAT4"},
                {"bufferView": 5, "componentType": 5126, "count": 2, "type": "SCALAR",
                 "min": [0.0], "max": [1.0]},
                {"bufferView": 6, "componentType": 5126, "count": 2, "type": "VEC4"},
                {"bufferView": 7, "componentType": 5126, "count": 2, "type": "SCALAR",
                 "min": [0.0], "max": [0.5]},
                {"bufferView": 8, "componentType": 5126, "count": 2, "type": "VEC3"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": p_off, "byteLength": p_len},
                {"buffer": 0, "byteOffset": j_off, "byteLength": j_len},
                {"buffer": 0, "byteOffset": w_off, "byteLength": w_len},
                {"buffer": 0, "byteOffset": i_off, "byteLength": i_len},
                {"buffer": 0, "byteOffset": m_off, "byteLength": m_len},
                {"buffer": 0, "byteOffset": t_off, "byteLength": t_len},
                {"buffer": 0, "byteOffset": r_off, "byteLength": r_len},
                {"buffer": 0, "byteOffset": lt_off, "byteLength": lt_len},
                {"buffer": 0, "byteOffset": lv_off, "byteLength": lv_len}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        Fixture { json: doc, bin }
    }

    /// A skinned character made of boxes: one node per bone (parents
    /// first), one skinned mesh with flat per-face normals and vertex
    /// colours, joints listed in bone order, and one animation per clip
    /// with linear rotation channels.
    pub fn rigged_boxes(bones: &[RigBone], clips: &[RigClip]) -> Fixture {
        // Bind-pose world position of every joint (offsets accumulate; the
        // rest pose has no rotation).
        let mut origin = vec![[0.0f32; 3]; bones.len()];
        for (i, b) in bones.iter().enumerate() {
            let p = b.parent.filter(|&p| p < i).map_or([0.0; 3], |p| origin[p]);
            origin[i] = [p[0] + b.offset[0], p[1] + b.offset[1], p[2] + b.offset[2]];
        }
        let (mut pos, mut nrm, mut col) = (Vec::new(), Vec::new(), Vec::new());
        let (mut joints, mut weights, mut idx) = (Vec::<u8>::new(), Vec::new(), Vec::<u16>::new());
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        // Faces: normal axis, sign, and the two in-plane axes (ordered so
        // the corners wind counter-clockwise seen from outside).
        let faces: [(usize, f32, usize, usize); 6] = [
            (0, 1.0, 1, 2),
            (0, -1.0, 2, 1),
            (1, 1.0, 2, 0),
            (1, -1.0, 0, 2),
            (2, 1.0, 0, 1),
            (2, -1.0, 1, 0),
        ];
        for (j, b) in bones.iter().enumerate() {
            for (centre, half, color) in &b.boxes {
                for &(axis, sign, u, v) in &faces {
                    let base = (pos.len() / 3) as u16;
                    for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                        let mut p = [0.0f32; 3];
                        p[axis] = sign * half[axis];
                        p[u] = su * half[u];
                        p[v] = sv * half[v];
                        let mut n = [0.0f32; 3];
                        n[axis] = sign;
                        for k in 0..3 {
                            let w = origin[j][k] + centre[k] + p[k];
                            lo[k] = lo[k].min(w);
                            hi[k] = hi[k].max(w);
                            pos.push(w);
                        }
                        nrm.extend_from_slice(&n);
                        col.extend_from_slice(&[color[0], color[1], color[2], 1.0]);
                        joints.extend_from_slice(&[j as u8, 0, 0, 0]);
                        weights.extend_from_slice(&[1.0f32, 0.0, 0.0, 0.0]);
                    }
                    idx.extend([0, 1, 2, 0, 2, 3].iter().map(|i| base + i));
                }
            }
        }
        let count = pos.len() / 3;
        let mut bin = Vec::new();
        let mut views = Vec::new();
        let mut view = |bin: &mut Vec<u8>, start: usize| {
            pad4(bin);
            views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": bin.len() - start}));
            views.len() - 1
        };
        let start = bin.len();
        push_f32s(&mut bin, pos.iter().copied());
        let v_pos = view(&mut bin, start);
        let start = bin.len();
        push_f32s(&mut bin, nrm.iter().copied());
        let v_nrm = view(&mut bin, start);
        let start = bin.len();
        push_f32s(&mut bin, col.iter().copied());
        let v_col = view(&mut bin, start);
        let start = bin.len();
        bin.extend_from_slice(&joints);
        let v_jnt = view(&mut bin, start);
        let start = bin.len();
        push_f32s(&mut bin, weights.iter().copied());
        let v_wgt = view(&mut bin, start);
        let start = bin.len();
        for i in &idx {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let v_idx = view(&mut bin, start);
        // Inverse bind matrices: translations by minus the joint origin.
        let start = bin.len();
        for o in &origin {
            let mut m = [0.0f32; 16];
            for i in 0..4 {
                m[i * 5] = 1.0;
            }
            m[12] = -o[0];
            m[13] = -o[1];
            m[14] = -o[2];
            push_f32s(&mut bin, m);
        }
        let v_ibm = view(&mut bin, start);
        let mut accessors = vec![
            json!({"bufferView": v_pos, "componentType": 5126, "count": count, "type": "VEC3",
                   "min": lo, "max": hi}),
            json!({"bufferView": v_nrm, "componentType": 5126, "count": count, "type": "VEC3"}),
            json!({"bufferView": v_col, "componentType": 5126, "count": count, "type": "VEC4"}),
            json!({"bufferView": v_jnt, "componentType": 5121, "count": count, "type": "VEC4"}),
            json!({"bufferView": v_wgt, "componentType": 5126, "count": count, "type": "VEC4"}),
            json!({"bufferView": v_idx, "componentType": 5123, "count": idx.len(), "type": "SCALAR"}),
            json!({"bufferView": v_ibm, "componentType": 5126, "count": bones.len(), "type": "MAT4"}),
        ];
        let mut animations = Vec::new();
        for clip in clips {
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            for (bone, times, values) in &clip.tracks {
                let start = bin.len();
                push_f32s(&mut bin, times.iter().copied());
                let v_t = view(&mut bin, start);
                let start = bin.len();
                push_f32s(&mut bin, values.iter().flatten().copied());
                let v_v = view(&mut bin, start);
                let t_min = times.iter().copied().fold(f32::MAX, f32::min);
                let t_max = times.iter().copied().fold(f32::MIN, f32::max);
                accessors.push(json!({"bufferView": v_t, "componentType": 5126,
                    "count": times.len(), "type": "SCALAR", "min": [t_min], "max": [t_max]}));
                accessors.push(json!({"bufferView": v_v, "componentType": 5126,
                    "count": values.len(), "type": "VEC4"}));
                samplers.push(
                    json!({"input": accessors.len() - 2, "output": accessors.len() - 1,
                    "interpolation": "LINEAR"}),
                );
                channels.push(json!({"sampler": samplers.len() - 1,
                    "target": {"node": bone, "path": "rotation"}}));
            }
            animations.push(json!({"name": clip.name, "samplers": samplers, "channels": channels}));
        }
        let mut nodes: Vec<Value> = bones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let children: Vec<usize> = bones
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.parent == Some(i))
                    .map(|(k, _)| k)
                    .collect();
                let mut n = json!({"name": b.name, "translation": b.offset});
                if !children.is_empty() {
                    n["children"] = json!(children);
                }
                n
            })
            .collect();
        let mesh_node = nodes.len();
        nodes.push(json!({"name": "body", "mesh": 0, "skin": 0}));
        let mut roots: Vec<usize> = bones
            .iter()
            .enumerate()
            .filter(|(_, b)| b.parent.is_none())
            .map(|(i, _)| i)
            .collect();
        roots.push(mesh_node);
        let doc = json!({
            "asset": {"version": "2.0", "generator": "psx_assets fixtures"},
            "scene": 0,
            "scenes": [{"nodes": roots}],
            "nodes": nodes,
            "skins": [{"joints": (0..bones.len()).collect::<Vec<_>>(), "inverseBindMatrices": 6}],
            "meshes": [{"name": "body", "primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "COLOR_0": 2, "JOINTS_0": 3, "WEIGHTS_0": 4},
                "indices": 5
            }]}],
            "animations": animations,
            "accessors": accessors,
            "bufferViews": views,
            "buffers": [{"byteLength": bin.len()}]
        });
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
