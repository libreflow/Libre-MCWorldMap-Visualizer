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
    // Heightmaps (long arrays) and block_entities are not needed for rendering;
    // unknown fields are ignored by serde, so they are simply not declared.
    #[serde(rename = "sections")]
    pub sections: Vec<SectionNbt>,
}

#[derive(Deserialize)]
pub struct SectionNbt {
    #[serde(rename = "Y")]
    pub y: i8,
    #[serde(rename = "block_states")]
    pub block_states: Option<BlockStatesNbt>,
}

#[derive(Deserialize)]
pub struct BlockStatesNbt {
    pub palette: Vec<PaletteEntryNbt>,
    pub data: Option<LongArray>,
}

use fastnbt::LongArray;

#[derive(Deserialize)]
#[serde(untagged)]
pub enum PaletteEntryNbt {
    /// Modern (26.3+): palette is a bare TAG_String, e.g. "minecraft:stone"
    Direct(String),
    /// Compound entry { Name: "minecraft:...", Properties: {...} } — the
    /// real pre-26.3 layout uses a capital N; alias keeps lowercase working.
    Named {
        #[serde(alias = "Name", alias = "name")]
        name: String,
    },
}

impl PaletteEntryNbt {
    pub fn name(&self) -> &str {
        match self {
            PaletteEntryNbt::Direct(s) => s,
            PaletteEntryNbt::Named { name } => name,
        }
    }
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

/// Parsing/rendering statistics for one render pass, surfaced to the UI.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RenderStats {
    pub chunks_read: usize,
    pub chunks_parsed: usize,
    pub chunks_empty: usize,
    pub chunks_failed: usize,
    pub parse_error_sample: Option<String>,
}

/// Bit-pack reader for packed 64-bit long arrays (Minecraft format).
struct PackedBits {
    entries: Vec<u16>,
}

impl PackedBits {
    fn new(data: &[i64], bits: usize, count: usize) -> Self {
        let mut entries = vec![0u16; count];
        if bits > 0 {
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
        }
        Self { entries }
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
        stats: &mut RenderStats,
    ) -> Result<Vec<TileEntry>, String> {
        let _ = (rx, rz);
        let mut region = open_region(region_path)?;

        let mut tiles = Vec::new();
        // Iterate chunk positions 0..32 relative to region
        for lz in 0..32 {
            for lx in 0..32 {
                let data = region.read_chunk(lx, lz);
                let data = match data {
                    Ok(Some(d)) => {
                        stats.chunks_read += 1;
                        d
                    }
                    Ok(None) => continue,
                    Err(_) => {
                        stats.chunks_failed += 1;
                        continue;
                    }
                };
                let tile = self.mesh_chunk(&data, out_dir, rx * 32 + lx as i32, rz * 32 + lz as i32, stats);
                if let Some(t) = tile {
                    tiles.push(t);
                }
            }
        }
        Ok(tiles)
    }

    fn mesh_chunk(
        &self,
        nbt_bytes: &[u8],
        out_dir: &Path,
        cx: i32,
        cz: i32,
        stats: &mut RenderStats,
    ) -> Option<TileEntry> {
        let chunk: ChunkNbt = match from_bytes(nbt_bytes) {
            Ok(c) => {
                stats.chunks_parsed += 1;
                c
            }
            Err(e) => {
                stats.chunks_failed += 1;
                if stats.parse_error_sample.is_none() {
                    stats.parse_error_sample = Some(format!("chunk {cx},{cz}: {e}"));
                }
                return None;
            }
        };
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
            stats.chunks_empty += 1;
            return None;
        }

        let file_name = format!("chunk_{cx}_{cz}.glb");
        let path = out_dir.join(&file_name);
        if let Err(e) = GlbWriter::write(&path, &mesh.vertices, &mesh.indices) {
            stats.chunks_failed += 1;
            if stats.parse_error_sample.is_none() {
                stats.parse_error_sample = Some(format!("chunk {cx},{cz} glb write failed: {e}"));
            }
            return None;
        }

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
            if bs.palette.is_empty() {
                continue;
            }
            // Local palette -> global palette indices
            let mut local_to_global = Vec::with_capacity(bs.palette.len());
            for entry in &bs.palette {
                let entry_name = entry.name();
                let idx = match palette_map.get(entry_name) {
                    Some(i) => *i,
                    None => {
                        palette.push(entry_name.to_string());
                        let i = (palette.len() - 1) as u16;
                        palette_map.insert(entry_name.to_string(), i);
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
                        let g = local_to_global
                            .get(packed.get(i) as usize % local_to_global.len())
                            .copied()
                            .unwrap_or(0);
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

pub fn open_region(path: &Path) -> Result<Region<File>, String> {
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
    // Legacy layout: <world>/region/*.mca (single dimension, pre-1.21-style saves).
    collect_regions(&world_dir.join("region"), &mut out);
    searched.push(world_dir.join("region"));
    if out.is_empty() {
        // Newer layouts: regions live under <world>/dimensions/<ns>/<dim>/region/.
        // Prefer the Overworld; other dimensions would overlap at the same coordinates.
        let overworld = world_dir.join("dimensions/minecraft/overworld/region");
        collect_regions(&overworld, &mut out);
        searched.push(overworld);
    }
    if out.is_empty() {
        // Fallback: recursive search for any .mca files anywhere in the world folder.
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

/// True when the .mca file at `path` lives in a non-terrain folder such as
/// `entities/` or `poi/` — those share the r.X.Z.mca naming but hold entity
/// data, not block sections.
fn is_non_terrain_region(path: &Path) -> bool {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| {
            let n = n.to_string_lossy();
            n == "entities" || n == "poi"
        })
        .unwrap_or(false)
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
            if !is_non_terrain_region(&path) {
                out.push((rx, rz, path.clone()));
            }
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

// ---------------------------------------------------------------------------
// Chunk format diagnostics (for adapting the parser to new Minecraft versions)
// ---------------------------------------------------------------------------

/// A raw NBT tag walker: reports field names and exact tag types without
/// assuming any schema. Used by the dump_chunk command to diagnose format
/// changes in newly released Minecraft versions.
pub fn dump_chunk_top_level(nbt_bytes: &[u8]) -> Result<String, String> {
    let mut out = String::new();
    let mut r = Reader { data: nbt_bytes, pos: 0 };
    // Root: TAG_Compound (0x0A) + name
    let root_tag = r.u8().map_err(|e| e.to_string())?;
    if root_tag != 0x0A {
        return Err(format!("root tag is 0x{root_tag:02X}, expected compound"));
    }
    let _root_name = r.string().map_err(|e| e.to_string())?;
    loop {
        let tag = r.u8().map_err(|e| e.to_string())?;
        if tag == 0x00 {
            break;
        }
        let name = r.string().map_err(|e| e.to_string())?;
        let desc = describe_payload(&mut r, tag, 0).map_err(|e| e.to_string())?;
        out.push_str(&format!("  {name}: {}\n", desc));
    }
    Ok(out)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        self.take(1).map(|b| b[0])
    }
    fn i32(&mut self) -> Result<i32, String> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Result<String, String> {
        let b = self.take(2)?;
        let len = u16::from_be_bytes([b[0], b[1]]) as usize;
        let b = self.take(len)?;
        String::from_utf8(b.to_vec()).map_err(|e| e.to_string())
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("unexpected end of NBT data".into());
        }
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }
}

fn tag_name(tag: u8) -> &'static str {
    match tag {
        0x00 => "TAG_End",
        0x01 => "TAG_Byte",
        0x02 => "TAG_Short",
        0x03 => "TAG_Int",
        0x04 => "TAG_Long",
        0x05 => "TAG_Float",
        0x06 => "TAG_Double",
        0x07 => "TAG_Byte_Array",
        0x08 => "TAG_String",
        0x09 => "TAG_List",
        0x0A => "TAG_Compound",
        0x0B => "TAG_Int_Array",
        0x0C => "TAG_Long_Array",
        _ => "TAG_Unknown",
    }
}

/// Walk one payload (after tag byte + name) and return a human-readable
/// description. Recurses with a depth limit to keep the dump readable.
fn describe_payload(r: &mut Reader, tag: u8, depth: usize) -> Result<String, String> {
    const MAX_DEPTH: usize = 3;
    match tag {
        0x01 => Ok(format!("TAG_Byte = {}", r.u8()? as i8)),
        0x02 => {
            let b = r.take(2)?;
            Ok(format!("TAG_Short = {}", i16::from_be_bytes([b[0], b[1]])))
        }
        0x03 => Ok(format!("TAG_Int = {}", r.i32()?)),
        0x04 => {
            let b = r.take(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            Ok(format!("TAG_Long = {}", i64::from_be_bytes(a)))
        }
        0x05 => {
            let b = r.take(4)?;
            let mut a = [0u8; 4];
            a.copy_from_slice(b);
            Ok(format!("TAG_Float = {}", f32::from_be_bytes(a)))
        }
        0x06 => {
            let b = r.take(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            Ok(format!("TAG_Double = {}", f64::from_be_bytes(a)))
        }
        0x07 => {
            let len = r.i32()? as usize;
            let bytes = r.take(len)?;
            let head: Vec<String> = bytes.iter().take(16).map(|b| format!("{b:02X}")).collect();
            Ok(format!(
                "TAG_Byte_Array(len={len}) first bytes: {}",
                head.join(" ")
            ))
        }
        0x08 => {
            let s = r.string()?;
            if s.len() > 64 {
                Ok(format!("TAG_String = {:?}…", &s[..64]))
            } else {
                Ok(format!("TAG_String = {s:?}"))
            }
        }
        0x09 => {
            let elem_tag = r.u8()?;
            let count = r.i32()?;
            if depth >= MAX_DEPTH {
                // Skip payloads we won't describe
                skip_list_payloads(r, elem_tag, count as usize)?;
                return Ok(format!("TAG_List<{}> x{count}", tag_name(elem_tag)));
            }
            let shown = count.min(3) as usize;
            let mut descs = Vec::new();
            for _ in 0..shown {
                descs.push(describe_payload(r, elem_tag, depth + 1)?);
            }
            // Skip the rest
            skip_list_payloads(r, elem_tag, count as usize - shown)?;
            Ok(format!("TAG_List<{}> x{count} [{}]", tag_name(elem_tag), descs.join("; ")))
        }
        0x0A => {
            if depth >= MAX_DEPTH {
                skip_compound(r)?;
                return Ok("TAG_Compound {…}".to_string());
            }
            let mut descs = Vec::new();
            loop {
                let t = r.u8()?;
                if t == 0x00 {
                    break;
                }
                let name = r.string()?;
                let d = describe_payload(r, t, depth + 1)?;
                descs.push(format!("{name}: {d}"));
            }
            Ok(format!("TAG_Compound {{ {} }}", descs.join(", ")))
        }
        0x0B => {
            let len = r.i32()?;
            r.take(len as usize * 4)?;
            Ok(format!("TAG_Int_Array(len={len})"))
        }
        0x0C => {
            let len = r.i32()?;
            r.take(len as usize * 8)?;
            Ok(format!("TAG_Long_Array(len={len})"))
        }
        _ => Err(format!("unknown tag 0x{tag:02X}")),
    }
}

fn skip_list_payloads(r: &mut Reader, elem_tag: u8, count: usize) -> Result<(), String> {
    for _ in 0..count {
        skip_payload(r, elem_tag)?;
    }
    Ok(())
}

fn skip_compound(r: &mut Reader) -> Result<(), String> {
    loop {
        let t = r.u8()?;
        if t == 0x00 {
            return Ok(());
        }
        let _name = r.string()?;
        skip_payload(r, t)?;
    }
}

fn skip_payload(r: &mut Reader, tag: u8) -> Result<(), String> {
    match tag {
        0x01 => r.take(1).map(|_| ()),
        0x02 => r.take(2).map(|_| ()),
        0x03 => r.take(4).map(|_| ()),
        0x04 | 0x06 => r.take(8).map(|_| ()),
        0x05 => r.take(4).map(|_| ()),
        0x07 => {
            let len = r.i32()? as usize;
            r.take(len).map(|_| ())
        }
        0x08 => r.string().map(|_| ()),
        0x09 => {
            let elem_tag = r.u8()?;
            let count = r.i32()? as usize;
            skip_list_payloads(r, elem_tag, count)
        }
        0x0A => skip_compound(r),
        0x0B => {
            let len = r.i32()? as usize;
            r.take(len * 4).map(|_| ())
        }
        0x0C => {
            let len = r.i32()? as usize;
            r.take(len * 8).map(|_| ())
        }
        _ => Err(format!("unknown tag 0x{tag:02X} in skip")),
    }
}
