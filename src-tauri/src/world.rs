use crate::glb::GlbWriter;
use crate::mesh::{face_vertices, BlockMapper, ChunkMesh, Direction};
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
}

#[derive(Deserialize)]
struct BlockStatesNbt {
    palette: Vec<PaletteEntryNbt>,
    data: Option<LongArray>,
}

use fastnbt::LongArray;

#[derive(Deserialize)]
struct PaletteEntryNbt {
    name: String,
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
}

/// Bit-pack reader for packed 64-bit long arrays (Minecraft format).
struct PackedBits<'a> {
    data: &'a [i64],
    bits: usize,
    entries: Vec<u16>,
}

impl<'a> PackedBits<'a> {
    fn new(data: &'a [i64], bits: usize, count: usize) -> Self {
        let mut entries = vec![0u16; count];
        if bits == 0 {
            return Self { data, bits, entries };
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
        Self { data, bits, entries }
    }

    fn get(&self, i: usize) -> u16 {
        self.entries.get(i).copied().unwrap_or(0)
    }
}

impl WorldRenderer {
    pub fn new() -> Self {
        Self { mapper: BlockMapper::builtin() }
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
                let tile = self.mesh_chunk(&data, out_dir, rx * 32 + lx as i32, rz * 32 + lz as i32);
                if let Some(t) = tile {
                    tiles.push(t);
                }
            }
        }
        Ok(tiles)
    }

    fn mesh_chunk(&self, nbt_bytes: &[u8], out_dir: &Path, cx: i32, cz: i32) -> Option<TileEntry> {
        let chunk: ChunkNbt = from_bytes(nbt_bytes).ok()?;
        let mut mesh = ChunkMesh::default();

        // Section Y -> sections map
        let _sections: std::collections::HashMap<i8, &SectionNbt> =
            chunk.sections.iter().map(|s| (s.y, s)).collect();

        let world = BlockWorld::from_chunk(&chunk);

        for y in WORLD_MIN_Y..(WORLD_MIN_Y + WORLD_HEIGHT as i32) {
            for z in 0..16 {
                for x in 0..16 {
                    let Some(state_idx) = world.block(x, y, z) else { continue };
                    let Some(name) = world.palette_name(state_idx) else { continue };
                    if world.is_air(state_idx) {
                        continue;
                    }
                    let info = self.mapper.lookup(name);
                    let wx = cx * 16 + x as i32;
                    let wz = cz * 16 + z as i32;

                    for dir in Direction::ALL {
                        let [dx, dy, dz] = dir.offset();
                        let nb = world.block(x + dx, y + dy, z + dz);
                        let nb_transparent = match nb {
                            None => true,
                            Some(idx) => world.is_transparent(idx, &self.mapper),
                        };
                        if nb_transparent {
                            let verts = face_vertices(wx, y, wz, dir, info.color);
                            mesh.extend(&verts);
                        }
                    }
                }
            }
        }

        if mesh.vertices.is_empty() {
            return None;
        }

        let file_name = format!("chunk_{cx}_{cz}.glb");
        let path = out_dir.join(&file_name);
        GlbWriter::write(&path, &mesh.vertices, &mesh.indices).ok()?;

        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in mesh.vertices.chunks_exact(6) {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }

        Some(TileEntry { file: file_name, x: cx, z: cz, min, max })
    }
}

/// A loaded chunk, expanded into a flat block index array for fast neighbor lookups.
struct BlockWorld {
    palette: Vec<String>,
    /// Index into palette, per block in the chunk (y-major relative to WORLD_MIN_Y).
    blocks: Vec<u16>,
}

impl BlockWorld {
    fn from_chunk(chunk: &ChunkNbt) -> Self {
        let mut palette = vec!["minecraft:air".to_string()];
        let mut blocks = vec![0u16; 16 * 16 * WORLD_HEIGHT];
        let mut palette_map: std::collections::HashMap<String, u16> = std::collections::HashMap::new();
        palette_map.insert("minecraft:air".into(), 0);

        for section in &chunk.sections {
            let Some(bs) = &section.block_states else { continue };
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
        Self { palette, blocks }
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
        matches!(self.palette_name(idx), Some("minecraft:air") | Some("minecraft:cave_air") | Some("minecraft:void_air"))
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

fn bit_width(max: u64) -> usize {
    (64 - max.leading_zeros() as usize).max(1)
}

fn open_region(path: &Path) -> Result<Region<File>, String> {
    let file = File::open(path).map_err(|e| format!("failed to open {path:?}: {e}"))?;
    Region::from_stream(file).map_err(|e| format!("invalid region file {path:?}: {e}"))
}

/// Scan the world directory and return all region files (rx, rz, path).
/// Looks in `<world>/region/` first, then recursively (limited depth) to
/// handle nested structures (dimension subfolders, renamed region dirs).
pub fn find_regions(world_dir: &Path) -> Result<Vec<(i32, i32, PathBuf)>, String> {
    if !world_dir.is_dir() {
        return Err(format!("world directory not found: {world_dir:?}"));
    }
    let mut out = Vec::new();
    let mut searched = Vec::new();
    collect_regions(&world_dir.join("region"), &mut out);
    searched.push(world_dir.join("region"));
    if out.is_empty() {
        // Recursive search for any directory containing .mca files.
        collect_regions_recursive(world_dir, 0, &mut out, &mut searched);
    }
    if out.is_empty() {
        return Err(format!(
            "no region files (*.mca) found under {world_dir:?} (searched: {:?})",
            searched.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>()
        ));
    }
    out.sort_by_key(|(rx, rz, _)| (*rx, *rz));
    Ok(out)
}

fn collect_regions(region_dir: &Path, out: &mut Vec<(i32, i32, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(region_dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some((rx, rz)) = parse_region_name(&name) {
            out.push((rx, rz, entry.path()));
        }
    }
}

fn collect_regions_recursive(
    dir: &Path,
    depth: usize,
    out: &mut Vec<(i32, i32, PathBuf)>,
    searched: &mut Vec<PathBuf>,
) {
    const MAX_DEPTH: usize = 4;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            // Skip cache/session dirs that never contain regions
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "playerdata" || name == "stats" || name == "advancements" {
                continue;
            }
            collect_regions_recursive(&path, depth + 1, out, searched);
        } else if let Some((rx, rz)) = parse_region_name(&path.file_name().unwrap_or_default().to_string_lossy()) {
            out.push((rx, rz, path.clone()));
        }
    }
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
