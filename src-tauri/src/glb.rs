use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
struct GlbJson {
    asset: Asset,
    scene: usize,
    scenes: [Scene; 1],
    nodes: [Node; 1],
    meshes: [Mesh; 1],
    buffers: [Buffer; 1],
    #[serde(rename = "bufferViews")]
    buffer_views: Vec<BufferView>,
    accessors: Vec<Accessor>,
    images: [Image; 1],
    samplers: [Sampler; 1],
    textures: [Texture; 1],
    materials: [Material; 2],
}

#[derive(Serialize)]
struct Asset {
    version: String,
}

#[derive(Serialize)]
struct Scene {
    nodes: [usize; 1],
}

#[derive(Serialize)]
struct Node {
    mesh: usize,
    name: String,
}

#[derive(Serialize)]
struct Mesh {
    primitives: Vec<Primitive>,
}

#[derive(Serialize)]
struct Primitive {
    attributes: Attributes,
    indices: usize,
    material: usize,
    mode: u32,
}

#[derive(Serialize)]
struct Attributes {
    #[serde(rename = "POSITION")]
    position: usize,
    #[serde(rename = "TEXCOORD_0")]
    texcoord: usize,
    #[serde(rename = "COLOR_0")]
    color: usize,
}

#[derive(Serialize)]
struct Buffer {
    #[serde(rename = "byteLength")]
    byte_length: usize,
}

#[derive(Serialize)]
struct BufferView {
    buffer: usize,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    target: Option<u32>,
}

#[derive(Serialize)]
struct Accessor {
    #[serde(rename = "bufferView")]
    buffer_view: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    type_: String,
    min: Option<Vec<f32>>,
    max: Option<Vec<f32>>,
}

#[derive(Serialize)]
struct Image {
    #[serde(rename = "bufferView")]
    buffer_view: usize,
    mime_type: String,
}

#[derive(Serialize)]
struct Sampler {
    #[serde(rename = "magFilter")]
    mag_filter: u32,
    #[serde(rename = "minFilter")]
    min_filter: u32,
    wrap_s: u32,
    wrap_t: u32,
}

#[derive(Serialize)]
struct Texture {
    sampler: usize,
    source: usize,
}

#[derive(Serialize)]
struct Material {
    name: String,
    #[serde(rename = "pbrMetallicRoughness")]
    pbr: Pbr,
    #[serde(rename = "alphaMode")]
    alpha_mode: String,
    #[serde(rename = "doubleSided")]
    double_sided: bool,
}

#[derive(Serialize)]
struct Pbr {
    #[serde(rename = "baseColorTexture")]
    base_color_texture: TextureRef,
    #[serde(rename = "metallicFactor")]
    metallic_factor: f32,
    #[serde(rename = "roughnessFactor")]
    roughness_factor: f32,
}

#[derive(Serialize)]
struct TextureRef {
    index: usize,
}

/// One primitive group (vertices + indices) sharing a material.
pub struct PrimitiveGroup {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

/// Minimal glTF 2.0 GLB writer: textured mesh with per-vertex shade colors,
/// split into an opaque group (material 0, MASK) and a blended group
/// (material 1, BLEND) for water/ice. Atlas PNG embedded.
pub struct GlbWriter;

impl GlbWriter {
    /// `opaque` and `blended` hold interleaved pos(3)+uv(2)+shade(3) vertices.
    pub fn write(
        path: &std::path::Path,
        opaque: &PrimitiveGroup,
        blended: &PrimitiveGroup,
        atlas_png: &[u8],
    ) -> std::io::Result<()> {
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut colors = Vec::new();
        let mut indices = Vec::new();

        let mut base_vertex = 0usize;
        let mut ranges: Vec<(usize, usize)> = Vec::new(); // (index offset, count) per group
        for group in [opaque, blended] {
            let start = indices.len();
            for v in group.vertices.chunks_exact(8) {
                positions.extend_from_slice(&v[0..3]);
                uvs.extend_from_slice(&v[3..5]);
                colors.extend_from_slice(&v[5..8]);
            }
            for &i in &group.indices {
                indices.push(i + base_vertex as u32);
            }
            ranges.push((start, indices.len() - start));
            base_vertex += group.vertices.len() / 8;
        }

        let pos_bytes = to_le_bytes_f32(&positions);
        let uv_bytes = to_le_bytes_f32(&uvs);
        let col_bytes = to_le_bytes_f32(&colors);
        let idx_bytes = to_le_bytes_u32(&indices);

        let pos_offset = 0usize;
        let uv_offset = pos_offset + padded_len(&pos_bytes);
        let col_offset = uv_offset + padded_len(&uv_bytes);
        let idx_offset = col_offset + padded_len(&col_bytes);
        let png_offset = idx_offset + padded_len(&idx_bytes);
        let bin_len = png_offset + atlas_png.len();

        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in positions.chunks_exact(3) {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }

        let make_primitive = |i: usize| Primitive {
            attributes: Attributes {
                position: 0,
                texcoord: 1,
                color: 2,
            },
            indices: 3,
            material: i,
            mode: 4,
        };
        let primitives: Vec<Primitive> = ranges
            .iter()
            .enumerate()
            .filter(|(_, (_, count))| *count > 0)
            .map(|(i, _)| make_primitive(i))
            .collect();

        let json = GlbJson {
            asset: Asset {
                version: "2.0".into(),
            },
            scene: 0,
            scenes: [Scene { nodes: [0] }],
            nodes: [Node {
                mesh: 0,
                name: "chunk".into(),
            }],
            meshes: [Mesh { primitives }],
            buffers: [Buffer {
                byte_length: bin_len,
            }],
            buffer_views: vec![
                BufferView {
                    buffer: 0,
                    byte_offset: pos_offset,
                    byte_length: pos_bytes.len(),
                    target: Some(34962),
                },
                BufferView {
                    buffer: 0,
                    byte_offset: uv_offset,
                    byte_length: uv_bytes.len(),
                    target: Some(34962),
                },
                BufferView {
                    buffer: 0,
                    byte_offset: col_offset,
                    byte_length: col_bytes.len(),
                    target: Some(34962),
                },
                BufferView {
                    buffer: 0,
                    byte_offset: idx_offset,
                    byte_length: idx_bytes.len(),
                    target: Some(34963),
                },
                BufferView {
                    buffer: 0,
                    byte_offset: png_offset,
                    byte_length: atlas_png.len(),
                    target: None,
                },
            ],
            accessors: vec![
                Accessor {
                    buffer_view: 0,
                    component_type: 5126,
                    count: positions.len() / 3,
                    type_: "VEC3".into(),
                    min: Some(min.to_vec()),
                    max: Some(max.to_vec()),
                },
                Accessor {
                    buffer_view: 1,
                    component_type: 5126,
                    count: uvs.len() / 2,
                    type_: "VEC2".into(),
                    min: None,
                    max: None,
                },
                Accessor {
                    buffer_view: 2,
                    component_type: 5126,
                    count: colors.len() / 3,
                    type_: "VEC3".into(),
                    min: None,
                    max: None,
                },
                Accessor {
                    buffer_view: 3,
                    component_type: 5125,
                    count: indices.len(),
                    type_: "SCALAR".into(),
                    min: None,
                    max: None,
                },
            ],
            images: [Image {
                buffer_view: 4,
                mime_type: "image/png".into(),
            }],
            samplers: [Sampler {
                mag_filter: 9728,
                min_filter: 9984,
                wrap_s: 33071,
                wrap_t: 33071,
            }],
            textures: [Texture {
                sampler: 0,
                source: 0,
            }],
            materials: [
                Material {
                    name: "atlas".into(),
                    pbr: Pbr {
                        base_color_texture: TextureRef { index: 0 },
                        metallic_factor: 0.0,
                        roughness_factor: 1.0,
                    },
                    alpha_mode: "MASK".into(),
                    double_sided: true,
                },
                Material {
                    name: "water".into(),
                    pbr: Pbr {
                        base_color_texture: TextureRef { index: 0 },
                        metallic_factor: 0.0,
                        roughness_factor: 0.35,
                    },
                    alpha_mode: "BLEND".into(),
                    double_sided: false,
                },
            ],
        };

        let mut json_bytes = serde_json::to_vec(&json).expect("glTF json is serializable");
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }

        let mut bin = Vec::with_capacity(bin_len + 4);
        append_padded(&mut bin, &pos_bytes);
        append_padded(&mut bin, &uv_bytes);
        append_padded(&mut bin, &col_bytes);
        append_padded(&mut bin, &idx_bytes);
        bin.extend_from_slice(atlas_png);

        let full_len = 12u32 + 8 + json_bytes.len() as u32 + 8 + bin.len() as u32;
        let mut out = std::fs::File::create(path)?;
        out.write_all(b"glTF")?;
        out.write_all(&2u32.to_le_bytes())?;
        out.write_all(&full_len.to_le_bytes())?;
        out.write_all(&(json_bytes.len() as u32).to_le_bytes())?;
        out.write_all(&0x4E4F534Au32.to_le_bytes())?;
        out.write_all(&json_bytes)?;
        out.write_all(&(bin.len() as u32).to_le_bytes())?;
        out.write_all(&0x004E4942u32.to_le_bytes())?;
        out.write_all(&bin)?;
        Ok(())
    }
}

fn padded_len(bytes: &[u8]) -> usize {
    (bytes.len() + 3) & !3
}

fn append_padded(bin: &mut Vec<u8>, bytes: &[u8]) {
    bin.extend_from_slice(bytes);
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
}

fn to_le_bytes_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn to_le_bytes_u32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
