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
    primitives: [Primitive; 1],
}

#[derive(Serialize)]
struct Primitive {
    attributes: Attributes,
    indices: usize,
    mode: u32,
}

#[derive(Serialize)]
struct Attributes {
    #[serde(rename = "POSITION")]
    position: usize,
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
    target: u32,
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

/// Minimal glTF 2.0 GLB writer for a single colored mesh (POSITION + COLOR_0).
pub struct GlbWriter;

impl GlbWriter {
    /// Write an interleaved mesh (pos+color per vertex) as a GLB file.
    pub fn write(path: &std::path::Path, vertices: &[f32], indices: &[u32]) -> std::io::Result<()> {
        let vertex_count = vertices.len() / 6;
        let mut positions = Vec::with_capacity(vertex_count * 3);
        let mut colors = Vec::with_capacity(vertex_count * 3);
        for v in vertices.chunks_exact(6) {
            positions.extend_from_slice(&v[0..3]);
            colors.extend_from_slice(&v[3..6]);
        }

        let pos_bytes = to_le_bytes_f32(&positions);
        let col_bytes = to_le_bytes_f32(&colors);
        let idx_bytes = to_le_bytes_u32(indices);

        let pos_offset = 0usize;
        let col_offset = pos_offset + pos_bytes.len();
        let idx_offset = col_offset + col_bytes.len();
        let bin_len_unpadded = idx_offset + idx_bytes.len();

        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in positions.chunks_exact(3) {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }

        let json = GlbJson {
            asset: Asset { version: "2.0".into() },
            scene: 0,
            scenes: [Scene { nodes: [0] }],
            nodes: [Node { mesh: 0, name: "chunk".into() }],
            meshes: [Mesh {
                primitives: [Primitive {
                    attributes: Attributes { position: 0, color: 1 },
                    indices: 2,
                    mode: 4,
                }],
            }],
            buffers: [Buffer { byte_length: bin_len_unpadded }],
            buffer_views: vec![
                BufferView { buffer: 0, byte_offset: pos_offset, byte_length: pos_bytes.len(), target: 34962 },
                BufferView { buffer: 0, byte_offset: col_offset, byte_length: col_bytes.len(), target: 34962 },
                BufferView { buffer: 0, byte_offset: idx_offset, byte_length: idx_bytes.len(), target: 34963 },
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
                Accessor { buffer_view: 1, component_type: 5126, count: colors.len() / 3, type_: "VEC3".into(), min: None, max: None },
                Accessor { buffer_view: 2, component_type: 5125, count: indices.len(), type_: "SCALAR".into(), min: None, max: None },
            ],
        };

        let mut json_bytes = serde_json::to_vec(&json).expect("glTF json is serializable");
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }

        let mut bin = Vec::with_capacity(bin_len_unpadded + 4);
        bin.extend_from_slice(&pos_bytes);
        bin.extend_from_slice(&col_bytes);
        bin.extend_from_slice(&idx_bytes);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }

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

fn to_le_bytes_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn to_le_bytes_u32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
