use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type UvRect = [f32; 4];

/// Result of meshing one chunk: interleaved vertices (pos + uv) and indices.
#[derive(Debug, Default)]
pub struct ChunkMesh {
    /// pos(3) + uv(2), interleaved
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

impl ChunkMesh {
    pub fn extend(&mut self, verts: &[[f32; 5]; 4]) {
        let base = (self.vertices.len() / 5) as u32;
        for v in verts {
            self.vertices.extend_from_slice(v);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Face texture mapping for a block state.
#[derive(Clone, Debug)]
pub struct BlockInfo {
    /// texture tile index per face direction (E, W, U, D, S, N)
    pub tiles: [usize; 6],
    pub transparent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockEntry {
    pub tiles: [usize; 6],
    pub transparent: bool,
}

/// Block-state -> face texture mapping. Falls back to the missing (gray) tile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockMapper {
    palette: HashMap<String, BlockEntry>,
    missing: usize,
    /// uv rect per tile index (empty when no atlas is available)
    rects: Vec<UvRect>,
}

impl BlockMapper {
    pub fn from_atlas(
        meta: &HashMap<String, ([f32; 3], bool)>,
        atlas: &crate::resources::Atlas,
    ) -> Self {
        let missing = atlas.missing_idx;
        let mut palette = HashMap::new();
        for (block, faces) in &atlas.appearances {
            let tiles = faces.map(|f| f.unwrap_or(missing));
            let transparent = meta
                .get(block)
                .map(|(_, t)| *t)
                .unwrap_or_else(|| tiles.iter().all(|&t| t == missing));
            palette.insert(block.clone(), BlockEntry { tiles, transparent });
        }
        let rects = atlas.all_rects().to_vec();
        Self {
            palette,
            missing,
            rects,
        }
    }

    /// Texture-less mapper used when no jar/resource pack is available.
    pub fn fallback() -> Self {
        Self {
            palette: HashMap::new(),
            missing: 0,
            rects: vec![[0.0, 0.0, 1.0, 1.0]],
        }
    }

    pub fn rect(&self, tile_idx: usize) -> UvRect {
        self.rects
            .get(tile_idx)
            .copied()
            .unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    pub fn lookup(&self, block_state: &str) -> BlockInfo {
        let cleaned = block_state.trim_start_matches("minecraft:");
        let base = cleaned.split('[').next().unwrap_or(cleaned);
        match self.palette.get(base) {
            Some(e) => BlockInfo {
                tiles: e.tiles,
                transparent: e.transparent,
            },
            None => BlockInfo {
                tiles: [self.missing; 6],
                transparent: false,
            },
        }
    }
}

/// Face templates: 4 corners (offset), per direction, in CCW order.
/// Order: East(+X), West(-X), Up(+Y), Down(-Y), South(+Z), North(-Z)
/// Each face's UV corners follow the same corner order, matching the
/// conventional face vertex layout of Minecraft cube models.
pub const FACES: [[i32; 3]; 24] = [
    // East
    [1, 0, 1],
    [1, 0, 0],
    [1, 1, 0],
    [1, 1, 1],
    // West
    [0, 0, 0],
    [0, 0, 1],
    [0, 1, 1],
    [0, 1, 0],
    // Up
    [0, 1, 1],
    [1, 1, 1],
    [1, 1, 0],
    [0, 1, 0],
    // Down
    [0, 0, 0],
    [1, 0, 0],
    [1, 0, 1],
    [0, 0, 1],
    // South
    [0, 0, 1],
    [1, 0, 1],
    [1, 1, 1],
    [0, 1, 1],
    // North
    [1, 0, 0],
    [0, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
];

/// UV corner order within a tile rect.
/// Matches Minecraft model conventions: origin at top-left of the texture.
/// Order per face: bottom-left, bottom-right, top-right, top-left of the tile.
const UV_CORNERS: [[u8; 2]; 4] = [[0, 1], [1, 1], [1, 0], [0, 0]];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    East,
    West,
    Up,
    Down,
    South,
    North,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::East,
        Direction::West,
        Direction::Up,
        Direction::Down,
        Direction::South,
        Direction::North,
    ];

    pub fn offset(self) -> [i32; 3] {
        match self {
            Direction::East => [1, 0, 0],
            Direction::West => [-1, 0, 0],
            Direction::Up => [0, 1, 0],
            Direction::Down => [0, -1, 0],
            Direction::South => [0, 0, 1],
            Direction::North => [0, 0, -1],
        }
    }

    pub fn face_idx(self) -> usize {
        match self {
            Direction::East => 0,
            Direction::West => 4,
            Direction::Up => 8,
            Direction::Down => 12,
            Direction::South => 16,
            Direction::North => 20,
        }
    }
}

/// Build the 4 vertices of one face (pos + uv within the tile rect).
pub fn face_vertices(x: i32, y: i32, z: i32, dir: Direction, uv: UvRect) -> [[f32; 5]; 4] {
    let start = dir.face_idx();
    let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
    let corners = UV_CORNERS;
    let mut out = [[0.0f32; 5]; 4];
    for (i, v) in out.iter_mut().enumerate() {
        let off = FACES[start + i];
        let cu = u0 + (u1 - u0) * corners[i][0] as f32;
        let cv = v0 + (v1 - v0) * corners[i][1] as f32;
        *v = [
            (x + off[0]) as f32,
            (y + off[1]) as f32,
            (z + off[2]) as f32,
            cu,
            cv,
        ];
    }
    out
}
