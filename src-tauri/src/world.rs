use crate::glb::{GlbWriter, PrimitiveGroup};
use crate::mesh::{
    BlockMapper, ChunkMesh, Direction, ao_corners_generic, dir_shade, face_vertices,
};
use crate::resources::Atlas;
use fastanvil::Region;
use fastnbt::from_bytes;
use serde::{Deserialize, Serialize};
use std::fs::File;

use std::path::{Path, PathBuf};

/// Height range of a Minecraft world version.
pub const WORLD_MIN_Y: i32 = -64;
pub const WORLD_HEIGHT: usize = 384;

#[derive(Deserialize)]
pub struct ChunkNbt {
    #[serde(rename = "Heightmaps")]
    _heightmaps: Option<serde_json::Value>,
    #[serde(rename = "sections")]
    pub sections: Vec<SectionNbt>,
    #[serde(rename = "block_entities")]
    _block_entities: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct SectionNbt {
    #[serde(rename = "Y")]
    pub y: i8,
    #[serde(rename = "block_states")]
    pub block_states: Option<BlockStatesNbt>,
    #[serde(rename = "BlockLight", default)]
    pub block_light: Option<LongArray>,
}

#[derive(Deserialize)]
pub struct BlockStatesNbt {
    palette: Vec<PaletteEntryNbt>,
    data: Option<LongArray>,
}

use fastnbt::LongArray;

#[derive(Deserialize)]
struct PaletteEntryNbt {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    properties: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct TileManifest {
    pub world_name: String,
    pub tiles: Vec<TileEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TileEntry {
    pub file: String,
    pub x: i32,
    pub z: i32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

pub struct WorldRenderer {
    pub mapper: BlockMapper,
    /// PNG-encoded atlas shared by every chunk GLB (embedded per file).
    pub atlas_png: Vec<u8>,
}

/// Bit-pack reader for packed 64-bit long arrays (Minecraft format).
struct PackedBits<'a> {
    #[allow(dead_code)]
    data: &'a [i64],
    #[allow(dead_code)]
    bits: usize,
    entries: Vec<u16>,
}

impl<'a> PackedBits<'a> {
    fn new(data: &'a [i64], bits: usize, count: usize) -> Self {
        let mut entries = vec![0u16; count];
        if bits == 0 {
            return Self {
                data,
                bits,
                entries,
            };
        }
        let per_long = 64 / bits;
        for i in 0..count {
            let long_idx = i / per_long;
            let offset = (i % per_long) * bits;
            if let Some(l) = data.get(long_idx) {
                let v = (*l as u64 >> offset) & ((1u64 << bits) - 1);
                if let Some(e) = entries.get_mut(i) {
                    *e = v as u16;
                }
            }
        }
        Self {
            data,
            bits,
            entries,
        }
    }

    fn get(&self, i: usize) -> u16 {
        self.entries.get(i).copied().unwrap_or(0)
    }
}

impl WorldRenderer {
    pub fn new(atlas: Option<&Atlas>) -> Self {
        match atlas {
            Some(atlas) => {
                let mapper = crate::resources::mapper_with_atlas(atlas);
                let atlas_png = atlas.to_png().unwrap_or_default();
                Self { mapper, atlas_png }
            }
            None => Self {
                mapper: BlockMapper::fallback(),
                atlas_png: Vec::new(),
            },
        }
    }

    /// Parse one region file (`r.X.Z.mca`) and produce one GLB per chunk.
    /// Returns manifest entries for all chunks that produced geometry.
    pub fn render_region(
        &self,
        region_path: &Path,
        out_dir: &Path,
        rx: i32,
        rz: i32,
    ) -> Result<Vec<TileEntry>, String> {
        let _ = (rx, rz);
        let mut region = open_region(region_path)?;

        let mut tiles = Vec::new();
        // Iterate chunk positions 0..32 relative to region
        for lz in 0..32 {
            for lx in 0..32 {
                let data = region.read_chunk(lx, lz);
                let data = match data {
                    Ok(Some(d)) => d,
                    Ok(None) => continue,
                    Err(_) => continue,
                };
                let tile =
                    self.mesh_chunk(&data, out_dir, rx * 32 + lx as i32, rz * 32 + lz as i32);
                if let Some(t) = tile {
                    tiles.push(t);
                }
            }
        }
        Ok(tiles)
    }

    fn mesh_chunk(&self, nbt_bytes: &[u8], out_dir: &Path, cx: i32, cz: i32) -> Option<TileEntry> {
        let chunk: ChunkNbt = from_bytes(nbt_bytes).ok()?;
        let mut opaque = ChunkMesh::default();
        let mut blended = ChunkMesh::default();

        let world = BlockWorld::from_chunk(&chunk);
        let surface = world.surface_heights();

        for y in WORLD_MIN_Y..(WORLD_MIN_Y + WORLD_HEIGHT as i32) {
            for z in 0..16 {
                for x in 0..16 {
                    let Some(state_idx) = world.block(x, y, z) else {
                        continue;
                    };
                    let Some(name) = world.palette_name(state_idx) else {
                        continue;
                    };
                    if world.is_air(state_idx) {
                        continue;
                    }
                    let info = self.mapper.lookup(name);
                    let wx = cx * 16 + x as i32;
                    let wz = cz * 16 + z as i32;

                    // Darken blocks far below the surface (caves, overhangs).
                    let depth = (surface[(z * 16 + x) as usize] - y).max(0) as f32;
                    let light = if info.blend {
                        1.0
                    } else {
                        (1.0 - depth * 0.045).max(0.35)
                    };

                    for dir in Direction::ALL {
                        let [dx, dy, dz] = dir.offset();
                        let nb = world.block(x + dx, y + dy, z + dz);
                        let nb_transparent = match nb {
                            None => true,
                            Some(idx) => world.is_transparent(idx, &self.mapper),
                        };
                        if !nb_transparent {
                            continue;
                        }
                        let face_idx = dir.face_idx() / 4;
                        let uv = self.mapper.rect(info.tiles[face_idx]);
                        // Block light from the cell this face looks into.
                        let bl = world.light_at(x + dx, y + dy, z + dz) as f32;
                        let block_light = bl / 15.0;
                        let face_light = if info.blend {
                            light
                        } else {
                            (light + block_light * (1.0 - light)).min(1.0)
                        };
                        let base = dir_shade(dir) * face_light;
                        let ao = world.ao_corners(&self.mapper, x, y, z, dir);
                        let shade = ao.map(|a| (base * a).clamp(0.0, 1.0));
                        let verts = face_vertices(wx, y, wz, dir, uv, shade);
                        if info.blend {
                            blended.extend(&verts);
                        } else {
                            opaque.extend(&verts);
                        }
                    }
                }
            }
        }

        if opaque.vertices.is_empty() && blended.vertices.is_empty() {
            return None;
        }

        let file_name = format!("chunk_{cx}_{cz}.glb");
        let path = out_dir.join(&file_name);
        GlbWriter::write(
            &path,
            &PrimitiveGroup {
                vertices: opaque.vertices.clone(),
                indices: opaque.indices.clone(),
            },
            &PrimitiveGroup {
                vertices: blended.vertices.clone(),
                indices: blended.indices.clone(),
            },
            &self.atlas_png,
        )
        .ok()?;

        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in opaque
            .vertices
            .chunks_exact(8)
            .chain(blended.vertices.chunks_exact(8))
        {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }

        Some(TileEntry {
            file: file_name,
            x: cx,
            z: cz,
            min,
            max,
        })
    }
}

/// A loaded chunk, expanded into a flat block index array for fast neighbor lookups.
struct BlockWorld {
    palette: Vec<String>,
    /// Index into palette, per block in the chunk (y-major relative to WORLD_MIN_Y).
    blocks: Vec<u16>,
    /// Block light level (0-15) per block, 0 where absent.
    light: Vec<u8>,
}

impl BlockWorld {
    fn from_chunk(chunk: &ChunkNbt) -> Self {
        let mut palette = vec!["minecraft:air".to_string()];
        let mut blocks = vec![0u16; 16 * 16 * WORLD_HEIGHT];
        let mut palette_map: std::collections::HashMap<String, u16> =
            std::collections::HashMap::new();
        palette_map.insert("minecraft:air".into(), 0);

        let mut light = vec![0u8; 16 * 16 * WORLD_HEIGHT];

        for section in &chunk.sections {
            if let Some(bl) = &section.block_light {
                let sec_min_y = section.y as i32 * 16;
                for i in 0..4096usize {
                    let y = (i >> 8) & 15;
                    let z = (i >> 4) & 15;
                    let x = i & 15;
                    let wy = sec_min_y + y as i32;
                    if wy < WORLD_MIN_Y || wy >= WORLD_MIN_Y + WORLD_HEIGHT as i32 {
                        continue;
                    }
                    light[((wy - WORLD_MIN_Y) as usize * 16 + z) * 16 + x] = unpack_nibble(bl, i);
                }
            }
        }

        for section in &chunk.sections {
            let Some(bs) = &section.block_states else {
                continue;
            };
            // Local palette -> global palette indices
            let mut local_to_global = Vec::with_capacity(bs.palette.len());
            for entry in &bs.palette {
                let idx = match palette_map.get(&entry.name) {
                    Some(i) => *i,
                    None => {
                        palette.push(entry.name.clone());
                        let i = (palette.len() - 1) as u16;
                        palette_map.insert(entry.name.clone(), i);
                        i
                    }
                };
                local_to_global.push(idx);
            }

            let sec_min_y = section.y as i32 * 16;
            match &bs.data {
                Some(data) => {
                    let bits = usize::max(4, bit_width((bs.palette.len() - 1) as u64));
                    let packed = PackedBits::new(data, bits, 4096);
                    for i in 0..4096usize {
                        let y = (i >> 8) & 15;
                        let z = (i >> 4) & 15;
                        let x = i & 15;
                        let wy = sec_min_y + y as i32;
                        if wy < WORLD_MIN_Y || wy >= WORLD_MIN_Y + WORLD_HEIGHT as i32 {
                            continue;
                        }
                        let g = local_to_global[packed.get(i) as usize % local_to_global.len()];
                        let gi = ((wy - WORLD_MIN_Y) as usize * 16 + z) * 16 + x;
                        blocks[gi] = g;
                    }
                }
                None => {
                    // Single-value section: fill with palette[0]
                    if let Some(&g) = local_to_global.first() {
                        for y in 0..16 {
                            let wy = sec_min_y + y as i32;
                            if wy < WORLD_MIN_Y || wy >= WORLD_MIN_Y + WORLD_HEIGHT as i32 {
                                continue;
                            }
                            for z in 0..16 {
                                for x in 0..16 {
                                    let gi = ((wy - WORLD_MIN_Y) as usize * 16 + z) * 16 + x;
                                    blocks[gi] = g;
                                }
                            }
                        }
                    }
                }
            }
        }
        Self {
            palette,
            blocks,
            light,
        }
    }

    /// Block light level at a position (0-15).
    fn light_at(&self, x: i32, y: i32, z: i32) -> u8 {
        if x < 0 || x > 15 || z < 0 || z > 15 {
            return 0;
        }
        if y < WORLD_MIN_Y || y >= WORLD_MIN_Y + WORLD_HEIGHT as i32 {
            return 0;
        }
        self.light[((y - WORLD_MIN_Y) as usize * 16 + z as usize) * 16 + x as usize]
    }

    fn block(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        if x < 0 || x > 15 || z < 0 || z > 15 {
            return None; // cross-chunk neighbor: treat as transparent (chunk seams acceptable for MVP)
        }
        if y < WORLD_MIN_Y || y >= WORLD_MIN_Y + WORLD_HEIGHT as i32 {
            return None;
        }
        let gi = ((y - WORLD_MIN_Y) as usize * 16 + z as usize) * 16 + x as usize;
        self.blocks.get(gi).copied()
    }

    fn palette_name(&self, idx: u16) -> Option<&str> {
        self.palette.get(idx as usize).map(|s| s.as_str())
    }

    fn is_air(&self, idx: u16) -> bool {
        matches!(
            self.palette_name(idx),
            Some("minecraft:air") | Some("minecraft:cave_air") | Some("minecraft:void_air")
        )
    }

    /// Highest non-air block per column, for cave darkening.
    pub fn surface_heights(&self) -> Vec<i32> {
        let mut out = vec![WORLD_MIN_Y; 16 * 16];
        for z in 0..16 {
            for x in 0..16 {
                'col: for y in (WORLD_MIN_Y + WORLD_HEIGHT as i32 - 1)..WORLD_MIN_Y {
                    let y = y as i32;
                    if let Some(idx) = self.block(x, y, z) {
                        if !self.is_air(idx) {
                            out[(z * 16 + x) as usize] = y;
                            break 'col;
                        }
                    }
                }
            }
        }
        out
    }

    /// Per-corner ambient occlusion for one face (0fps algorithm).
    pub fn ao_corners(
        &self,
        mapper: &BlockMapper,
        x: i32,
        y: i32,
        z: i32,
        dir: Direction,
    ) -> [f32; 4] {
        let solid = |sx: i32, sy: i32, sz: i32| -> bool {
            match self.block(sx, sy, sz) {
                None => false,
                Some(idx) => !self.is_transparent(idx, mapper) && !self.is_air(idx),
            }
        };
        ao_corners_generic(x, y, z, dir, &solid)
    }

    fn is_transparent(&self, idx: u16, mapper: &BlockMapper) -> bool {
        if self.is_air(idx) {
            return true;
        }
        match self.palette_name(idx) {
            Some(n) => mapper.lookup(n).transparent,
            None => true,
        }
    }
}

/// Unpack the i-th 4-bit value from a packed long array (Minecraft light format).
fn unpack_nibble(arr: &LongArray, i: usize) -> u8 {
    let data: &[i64] = arr;
    let long_idx = i / 16;
    let offset = (i % 16) * 4;
    data.get(long_idx)
        .map(|l| ((*l as u64 >> offset) & 0xF) as u8)
        .unwrap_or(0)
}

fn bit_width(max: u64) -> usize {
    (64 - max.leading_zeros() as usize).max(1)
}

fn open_region(path: &Path) -> Result<Region<File>, String> {
    let file = File::open(path).map_err(|e| format!("failed to open {path:?}: {e}"))?;
    Region::from_stream(file).map_err(|e| format!("invalid region file {path:?}: {e}"))
}

/// Scan the world directory and return all region files (rx, rz, path).
pub fn find_regions(world_dir: &Path) -> Result<Vec<(i32, i32, PathBuf)>, String> {
    let region_dir = world_dir.join("region");
    if !region_dir.is_dir() {
        return Err(format!("no region directory in {world_dir:?}"));
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&region_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some((rx, rz)) = parse_region_name(&name) else {
            continue;
        };
        out.push((rx, rz, entry.path()));
    }
    out.sort_by_key(|(rx, rz, _)| (*rx, *rz));
    Ok(out)
}

fn parse_region_name(name: &str) -> Option<(i32, i32)> {
    let name = name.strip_suffix(".mca")?;
    let mut parts = name.split('.');
    let r = parts.next()?;
    if r != "r" {
        return None;
    }
    let rx = parts.next()?.parse().ok()?;
    let rz = parts.next()?.parse().ok()?;
    Some((rx, rz))
}
