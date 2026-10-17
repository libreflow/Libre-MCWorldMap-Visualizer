use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type UvRect = [f32; 4];

/// Result of meshing one chunk: interleaved vertices (pos + uv + shade color) and indices.
#[derive(Debug, Default)]
pub struct ChunkMesh {
    /// pos(3) + uv(2) + shade color(3), interleaved
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

impl ChunkMesh {
    pub fn extend(&mut self, verts: &[[f32; 8]; 4]) {
        let base = (self.vertices.len() / 8) as u32;
        for v in verts {
            self.vertices.extend_from_slice(v);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Face texture + render mapping for a block state.
#[derive(Clone, Debug)]
pub struct BlockInfo {
    /// texture tile index per face direction (E, W, U, D, S, N)
    pub tiles: [usize; 6],
    /// neighbors see through this block (faces drawn behind it)
    pub transparent: bool,
    /// rendered with alpha blending (water, ice) instead of alpha cutout
    pub blend: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockEntry {
    pub tiles: [usize; 6],
    pub transparent: bool,
    pub blend: bool,
}

/// Block-state -> face texture mapping. Falls back to the missing (gray) tile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockMapper {
    palette: HashMap<String, BlockEntry>,
    missing: usize,
    /// uv rect per tile index (empty when no atlas is available)
    #[allow(dead_code)]
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
            let blend = matches!(block.as_str(), "water" | "ice");
            let (transparent, blend) = match meta.get(block) {
                Some((_, t)) => (*t, blend),
                None => (tiles.iter().all(|&t| t == missing), blend),
            };
            palette.insert(
                block.clone(),
                BlockEntry {
                    tiles,
                    transparent,
                    blend,
                },
            );
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
                blend: e.blend,
            },
            None => BlockInfo {
                tiles: [self.missing; 6],
                transparent: false,
                blend: false,
            },
        }
    }
}

/// Face templates: 4 corner offsets per direction, in CCW order.
/// Order: East(+X), West(-X), Up(+Y), Down(-Y), South(+Z), North(-Z)
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

/// UV corner order within a tile rect (bottom-left, bottom-right, top-right, top-left).
const UV_CORNERS: [[u8; 2]; 4] = [[0, 1], [1, 1], [1, 0], [0, 0]];

/// Base brightness per face direction (fake directional light).
pub fn dir_shade(dir: Direction) -> f32 {
    match dir {
        Direction::East => 0.8,
        Direction::West => 0.8,
        Direction::Up => 1.0,
        Direction::Down => 0.5,
        Direction::South => 0.9,
        Direction::North => 0.7,
    }
}

/// Ambient-occlusion brightness levels indexed by the 0fps AO value (0 = darkest).
pub const AO_LEVELS: [f32; 4] = [0.45, 0.66, 0.83, 1.0];

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

    /// The axis this face is perpendicular to (0=X, 1=Y, 2=Z).
    pub fn normal_axis(self) -> usize {
        match self {
            Direction::East | Direction::West => 0,
            Direction::Up | Direction::Down => 1,
            Direction::South | Direction::North => 2,
        }
    }
}

/// Build the 4 vertices of one face (pos + uv + shade color).
/// `corner_shade` holds the brightness (AO * dir shade) per corner.
pub fn face_vertices(
    x: i32,
    y: i32,
    z: i32,
    dir: Direction,
    uv: UvRect,
    corner_shade: [f32; 4],
) -> [[f32; 8]; 4] {
    let start = dir.face_idx();
    let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
    let mut out = [[0.0f32; 8]; 4];
    for (i, v) in out.iter_mut().enumerate() {
        let off = FACES[start + i];
        let cu = u0 + (u1 - u0) * UV_CORNERS[i][0] as f32;
        let cv = v0 + (v1 - v0) * UV_CORNERS[i][1] as f32;
        let s = corner_shade[i];
        *v = [
            (x + off[0]) as f32,
            (y + off[1]) as f32,
            (z + off[2]) as f32,
            cu,
            cv,
            s,
            s,
            s,
        ];
    }
    out
}

/// Compute per-corner ambient occlusion for one block face (0fps algorithm).
/// `solid` reports whether the block at the given offset (relative to the
/// block being meshed) occludes. Corners are in the same order as FACES.
pub fn ao_corners_generic(
    x: i32,
    y: i32,
    z: i32,
    dir: Direction,
    solid: &dyn Fn(i32, i32, i32) -> bool,
) -> [f32; 4] {
    let start = dir.face_idx();
    let mut out = [1.0f32; 4];
    for (corner, v) in out.iter_mut().enumerate() {
        let c = FACES[start + corner];
        // Two edge neighbors + diagonal around this corner, in the plane
        // one step along the face normal.
        let n = dir.offset();
        // corner offset relative to block center: -1 or +1 per axis
        let cx = if c[0] == 1 { 1 } else { -1 };
        let cy = if c[1] == 1 { 1 } else { -1 };
        let cz = if c[2] == 1 { 1 } else { -1 };
        let axis = dir.normal_axis();
        // build the two in-plane axes
        let (a1, a2) = match axis {
            0 => ([0, cy, 0], [0, 0, cz]), // X face: in-plane = Y,Z
            1 => ([cx, 0, 0], [0, 0, cz]), // Y face: in-plane = X,Z
            _ => ([cx, 0, 0], [0, cy, 0]), // Z face: in-plane = X,Y
        };
        let px = x + n[0];
        let py = y + n[1];
        let pz = z + n[2];
        let side1 = solid(px + a1[0], py + a1[1], pz + a1[2]);
        let side2 = solid(px + a2[0], py + a2[1], pz + a2[2]);
        let diag = solid(px + a1[0] + a2[0], py + a1[1] + a2[1], pz + a1[2] + a2[2]);
        let ao = if side1 && side2 {
            0
        } else {
            3 - (side1 as u8 + side2 as u8 + diag as u8)
        };
        *v = AO_LEVELS[ao as usize];
    }
    out
}
