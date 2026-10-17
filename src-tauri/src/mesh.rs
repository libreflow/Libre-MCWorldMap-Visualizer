use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of meshing one chunk: interleaved vertices (pos + color) and indices.
#[derive(Debug, Default)]
pub struct ChunkMesh {
    /// pos(3) + color(3), interleaved
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

impl ChunkMesh {
    pub fn extend(&mut self, verts: &[[f32; 6]; 4]) {
        let base = (self.vertices.len() / 6) as u32;
        for v in verts {
            self.vertices.extend_from_slice(v);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Renderable info for a block state (MVP: colored cubes).
#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub color: [f32; 3],
    pub transparent: bool,
}

fn hex_to_rgb(hex: &str) -> [f32; 3] {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0x8080_80);
    [
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
    ]
}

/// Minimal block-state -> render model mapping (stands in for resource pack blockstates).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BlockMapper {
    palette: HashMap<String, BlockEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BlockEntry {
    color: String,
    transparent: bool,
}

impl BlockMapper {
    pub fn builtin() -> Self {
        let entries = [
            ("grass_block", "#5D9C3D", false),
            ("dirt", "#8B5A2B", false),
            ("stone", "#7F7F7F", false),
            ("cobblestone", "#828282", false),
            ("sand", "#DBD3A0", false),
            ("sandstone", "#D8CBAD", false),
            ("water", "#3F76E4", true),
            ("ice", "#A5C8F0", true),
            ("oak_log", "#6B5433", false),
            ("oak_leaves", "#4A7A28", true),
            ("oak_planks", "#9C7F4E", false),
            ("bedrock", "#333333", false),
            ("snow_block", "#F9FBFD", false),
            ("gravel", "#7F6B5B", false),
            ("deepslate", "#4F4F4F", false),
            ("granite", "#9A6A55", false),
            ("diorite", "#BBBBBB", false),
            ("andesite", "#8A8A8A", false),
            ("netherrack", "#700202", false),
        ];
        Self::from_entries(entries.iter().map(|(n, c, t)| (*n, *c, *t)))
    }

    fn from_entries<'a>(entries: impl Iterator<Item = (&'a str, &'a str, bool)>) -> Self {
        let mut palette = HashMap::new();
        for (name, color, transparent) in entries {
            palette.insert(
                name.to_string(),
                BlockEntry {
                    color: color.to_string(),
                    transparent,
                },
            );
        }
        Self { palette }
    }

    pub fn lookup(&self, block_state: &str) -> BlockInfo {
        let cleaned = block_state.trim_start_matches("minecraft:");
        let base = cleaned.split('[').next().unwrap_or(cleaned);
        match self.palette.get(base) {
            Some(e) => BlockInfo {
                color: hex_to_rgb(&e.color),
                transparent: e.transparent,
            },
            None => BlockInfo {
                color: [0.5, 0.5, 0.5],
                transparent: false,
            },
        }
    }
}

/// Face templates: 4 corners (offset + shade), per direction, in CCW order.
/// Order: East(+X), West(-X), Up(+Y), Down(-Y), South(+Z), North(-Z)
pub const FACES: [([i32; 3], f32); 24] = [
    // East
    ([1, 0, 1], 0.8), ([1, 0, 0], 0.8), ([1, 1, 0], 0.8), ([1, 1, 1], 0.8),
    // West
    ([0, 0, 0], 0.8), ([0, 0, 1], 0.8), ([0, 1, 1], 0.8), ([0, 1, 0], 0.8),
    // Up
    ([0, 1, 1], 1.0), ([1, 1, 1], 1.0), ([1, 1, 0], 1.0), ([0, 1, 0], 1.0),
    // Down
    ([0, 0, 0], 0.5), ([1, 0, 0], 0.5), ([1, 0, 1], 0.5), ([0, 0, 1], 0.5),
    // South
    ([0, 0, 1], 0.9), ([1, 0, 1], 0.9), ([1, 1, 1], 0.9), ([0, 1, 1], 0.9),
    // North
    ([1, 0, 0], 0.7), ([0, 0, 0], 0.7), ([0, 1, 0], 0.7), ([1, 1, 0], 0.7),
];

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

/// Build the 4 shaded vertices of one face.
pub fn face_vertices(
    x: i32,
    y: i32,
    z: i32,
    dir: Direction,
    color: [f32; 3],
) -> [[f32; 6]; 4] {
    let start = dir.face_idx();
    let mut out = [[0.0f32; 6]; 4];
    for (i, v) in out.iter_mut().enumerate() {
        let (off, shade) = FACES[start + i];
        *v = [
            (x + off[0]) as f32,
            (y + off[1]) as f32,
            (z + off[2]) as f32,
            color[0] * shade,
            color[1] * shade,
            color[2] * shade,
        ];
    }
    out
}
