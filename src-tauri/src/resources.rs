//! Resource pack / client jar loading: textures + blockstates + models -> atlas.
//!
//! Textures are read from the user's own Minecraft jar or resource pack and are
//! never embedded in the app or the repository (Mojang assets are copyrighted).

use crate::mesh::{BlockMapper, UvRect};
use image::{Rgba, RgbaImage};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

const TILE: u32 = 16;

/// Raw assets extracted from a jar/zip or a resource pack directory.
pub struct ResourcePack {
    /// "block/<name>" (no extension) -> decoded image
    textures: HashMap<String, RgbaImage>,
    /// "grass" / "foliage" colormaps (256x256, climate-indexed)
    pub colormaps: HashMap<String, RgbaImage>,
    /// "block/<name>" (no extension) -> raw model JSON
    models: HashMap<String, serde_json::Value>,
    /// "<block_name>" (no namespace) -> raw blockstate JSON
    blockstates: HashMap<String, serde_json::Value>,
}

impl ResourcePack {
    /// Open a `.jar`, `.zip` or a resource pack directory.
    pub fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            Self::open_dir(path)
        } else {
            Self::open_archive(path)
        }
    }

    fn open_archive(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("read zip {path:?}: {e}"))?;
        let mut pack = ResourcePack {
            textures: HashMap::new(),
            colormaps: HashMap::new(),
            models: HashMap::new(),
            blockstates: HashMap::new(),
        };
        for i in 0..zip.len() {
            let Ok(mut entry) = zip.by_index(i) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            let Some(rel) = strip_asset_prefix(&name) else {
                continue;
            };
            let mut bytes = Vec::new();
            if entry.read_to_end(&mut bytes).is_err() {
                continue;
            }
            pack.store(&rel, &bytes);
        }
        Ok(pack)
    }

    fn open_dir(path: &Path) -> Result<Self, String> {
        let mut pack = ResourcePack {
            textures: HashMap::new(),
            colormaps: HashMap::new(),
            models: HashMap::new(),
            blockstates: HashMap::new(),
        };
        let base = path.join("assets").join("minecraft");
        let base = if base.is_dir() {
            base
        } else {
            path.to_path_buf()
        };
        walk(&base.join("textures").join("block"), &base, &mut pack);
        walk(&base.join("models").join("block"), &base, &mut pack);
        walk(&base.join("blockstates"), &base, &mut pack);
        Ok(pack)
    }

    fn store(&mut self, rel: &str, bytes: &[u8]) {
        if let Some(cm) = rel.strip_prefix("colormap/") {
            let cm = cm.strip_suffix(".png").unwrap_or(cm);
            if let Ok(img) = image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
                self.colormaps.insert(cm.to_string(), img.to_rgba8());
            }
        } else if let Some(name) = rel.strip_prefix("textures/block/") {
            let name = name.strip_suffix(".png").unwrap_or(name);
            if let Ok(img) = image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
                self.textures
                    .insert(format!("block/{name}"), img.to_rgba8());
            }
        } else if let Some(name) = rel.strip_prefix("models/block/") {
            let name = name.strip_suffix(".json").unwrap_or(name);
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
                self.models.insert(format!("block/{name}"), v);
            }
        } else if let Some(name) = rel.strip_prefix("blockstates/") {
            let name = name.strip_suffix(".json").unwrap_or(name);
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
                self.blockstates.insert(name.to_string(), v);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    pub fn textures_debug(&self) -> Vec<&String> {
        self.textures.keys().collect()
    }

    pub fn blockstates_debug(&self) -> &HashMap<String, serde_json::Value> {
        &self.blockstates
    }

    pub fn models_debug(&self) -> &HashMap<String, serde_json::Value> {
        &self.models
    }

    pub fn build_atlas(&self) -> Atlas {
        Atlas::build(self)
    }
}

fn walk(dir: &Path, base: &Path, pack: &mut ResourcePack) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, base, pack);
        } else if let Ok(rel) = path.strip_prefix(base) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if let Ok(bytes) = std::fs::read(&path) {
                pack.store(&rel, &bytes);
            }
        }
    }
}

/// Map "assets/minecraft/<rel>" (or pack-format "blockstates/x.json") to "<rel>".
fn strip_asset_prefix(name: &str) -> Option<String> {
    let rel = name.replace('\\', "/");
    let rel = rel.strip_prefix("assets/minecraft/")?;
    Some(rel.to_string())
}

/// Try to locate the user's vanilla client jar (newest version found).
pub fn detect_vanilla_jar() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut v = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            v.push(PathBuf::from(appdata).join(".minecraft"));
        }
        if let Some(h) = &home {
            v.push(h.join(".minecraft"));
            v.push(h.join("Library/Application Support/minecraft"));
        }
        v
    };
    for root in candidates {
        let versions = root.join("versions");
        let Ok(entries) = std::fs::read_dir(&versions) else {
            continue;
        };
        let mut jars: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path().join(e.file_name());
                let p = p.with_extension("jar");
                let meta = std::fs::metadata(&p).ok()?;
                Some((meta.modified().ok()?, p))
            })
            .collect();
        jars.sort_by_key(|(t, _)| *t);
        if let Some((_, jar)) = jars.pop() {
            return Some(jar);
        }
    }
    None
}

/// A texture atlas over every block texture in a pack, plus per-block face mapping.
/// One non-cubic element of a model: an axis-aligned sub-box in 0..16 space
/// with a resolved texture per face.
#[derive(Clone, Debug)]
pub struct ElementBox {
    /// min corner in 16ths of a block
    pub from: [f32; 3],
    /// max corner in 16ths of a block
    pub to: [f32; 3],
    /// tile index per face direction (E, W, U, D, S, N)
    pub tiles: [Option<usize>; 6],
}

pub struct Atlas {
    pub image: RgbaImage,
    /// uv rect per tile index, in three.js (flipY) space
    #[allow(dead_code)]
    rects: Vec<UvRect>,
    /// index of the gray fallback tile
    pub missing_idx: usize,
    /// texture name ("block/stone") -> tile index
    #[allow(dead_code)]
    tiles: HashMap<String, usize>,
    /// block base name -> tile index per face direction (E,W,U,D,S,N)
    pub appearances: HashMap<String, [Option<usize>; 6]>,
    /// block base name -> non-cubic geometry (stairs, slabs, ...). Empty = cube.
    pub custom_shapes: HashMap<String, Vec<ElementBox>>,
}

/// Tint a texture by multiplying RGB (alpha kept) — used for grass/leaves.
fn tint_tile(img: &RgbaImage, tint: [u8; 3]) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        p.0[0] = ((p.0[0] as u32 * tint[0] as u32) / 255).min(255) as u8;
        p.0[1] = ((p.0[1] as u32 * tint[1] as u32) / 255).min(255) as u8;
        p.0[2] = ((p.0[2] as u32 * tint[2] as u32) / 255).min(255) as u8;
    }
    out
}

impl Atlas {
    fn build(pack: &ResourcePack) -> Self {
        let mut names: Vec<&String> = pack.textures.keys().collect();
        names.sort();
        // Climate variants for tinted blocks (grass, leaves): plains / forest /
        // savanna / taiga. Additional tiles are appended after the base ones.
        const TINTS: [(&str, [u8; 3]); 4] = [
            ("plains", [145, 189, 89]),
            ("forest", [121, 189, 107]),
            ("savanna", [191, 183, 85]),
            ("taiga", [104, 168, 113]),
        ];
        let tintable = |name: &str| -> bool {
            name.contains("grass_block")
                || name.contains("leaves")
                || name.contains("grass") && !name.contains("block")
        };
        let variant_count = names.iter().filter(|n| tintable(n)).count() * TINTS.len();
        let count = names.len() + variant_count + 1; // + tinted variants + missing tile
        let side = (count as f64).sqrt().ceil() as u32;
        let atlas_wh = side * TILE;
        let mut image = RgbaImage::new(atlas_wh, atlas_wh);
        let mut rects = Vec::with_capacity(count as usize);
        let mut tiles = HashMap::new();
        let half_texel = 0.5 / atlas_wh as f32;
        let mut next_slot = 0usize;

        let push_tile = |image: &mut RgbaImage,
                         rects: &mut Vec<UvRect>,
                         tiles: &mut HashMap<String, usize>,
                         slot: &mut usize,
                         key: String,
                         tile_img: RgbaImage|
         -> usize {
            let i = *slot;
            let col = (i as u32) % side;
            let row = (i as u32) / side;
            image::imageops::overlay(image, &tile_img, (col * TILE) as i64, (row * TILE) as i64);
            let u0 = col as f32 / side as f32 + half_texel;
            let u1 = (col + 1) as f32 / side as f32 - half_texel;
            let v0 = 1.0 - (row + 1) as f32 / side as f32 + half_texel;
            let v1 = 1.0 - row as f32 / side as f32 - half_texel;
            rects.push([u0, v0, u1, v1]);
            tiles.insert(key, i);
            *slot += 1;
            i
        };

        for name in names.iter() {
            let tile = normalize_tile(&pack.textures[*name]);
            push_tile(
                &mut image,
                &mut rects,
                &mut tiles,
                &mut next_slot,
                (*name).clone(),
                tile.clone(),
            );
            // Tinted climate variants for grass/leaves textures.
            if tintable(name) {
                for (climate, tint) in TINTS {
                    let key = format!("{name}#{climate}");
                    let tinted = tint_tile(&tile, tint);
                    push_tile(
                        &mut image,
                        &mut rects,
                        &mut tiles,
                        &mut next_slot,
                        key,
                        tinted,
                    );
                }
            }
        }
        // Missing-texture tile (gray)
        let missing_idx = push_tile(
            &mut image,
            &mut rects,
            &mut tiles,
            &mut next_slot,
            "__missing__".to_string(),
            RgbaImage::from_pixel(TILE, TILE, Rgba([128, 128, 128, 255])),
        );

        let mut appearances = HashMap::new();
        let mut custom_shapes: HashMap<String, Vec<ElementBox>> = HashMap::new();
        for (block, state) in &pack.blockstates {
            if let Some(model) = state_model(state) {
                let faces = resolve_model_faces(&pack.models, &model);
                let mapped = faces.map(|face| face.and_then(|t| tiles.get(&t).copied()));
                appearances.insert(block.clone(), mapped);
                if let Some(boxes) = resolve_model_boxes(&pack.models, &model) {
                    let resolved: Vec<ElementBox> = boxes
                        .into_iter()
                        .map(|(from, to, face_names)| ElementBox {
                            from,
                            to,
                            tiles: face_names.map(|f| f.and_then(|t| tiles.get(&t).copied())),
                        })
                        .collect();
                    custom_shapes.insert(block.clone(), resolved);
                }
            }
        }

        Self {
            image,
            rects,
            tiles,
            appearances,
            custom_shapes,
            missing_idx,
        }
    }

    pub fn tile_count(&self) -> usize {
        self.rects.len()
    }

    pub fn rect(&self, idx: usize) -> UvRect {
        self.rects.get(idx).copied().unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    pub fn all_rects(&self) -> &[UvRect] {
        &self.rects
    }

    /// texture name -> tile index (includes climate variants "name#plains")
    pub fn tile_index(&self) -> &HashMap<String, usize> {
        &self.tiles
    }

    /// block name -> non-cubic geometry (stairs, slabs...)
    pub fn custom_shapes(&self) -> &HashMap<String, Vec<ElementBox>> {
        &self.custom_shapes
    }

    /// Encode the atlas as PNG bytes.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(
            enc,
            self.image.as_raw(),
            self.image.width(),
            self.image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("png encode: {e}"))?;
        Ok(out)
    }
}

/// Fit any texture into a 16x16 tile: use the first frame of animation strips,
/// nearest-resize anything else.
fn normalize_tile(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let frame = if h > w {
        image::imageops::crop_imm(img, 0, 0, w, w).to_image()
    } else if w > h {
        image::imageops::crop_imm(img, 0, 0, h, h).to_image()
    } else {
        img.clone()
    };
    if frame.width() == TILE && frame.height() == TILE {
        return frame;
    }
    image::imageops::resize(&frame, TILE, TILE, image::imageops::FilterType::Nearest)
}

fn normalize_ref(name: &str) -> String {
    let n = name.trim_start_matches("minecraft:");
    let n = n
        .strip_prefix("blocks/")
        .map(|s| format!("block/{s}"))
        .unwrap_or_else(|| n.to_string());
    n
}

/// From a blockstate JSON, pick the model ref of the default variant.
fn state_model(state: &serde_json::Value) -> Option<String> {
    let variants = state.get("variants")?.as_object()?;
    let pick = variants
        .get("")
        .or_else(|| variants.get("normal"))
        .or_else(|| variants.values().next())?;
    let pick = pick.as_array().and_then(|a| a.first()).unwrap_or(pick);
    let model = pick.get("model")?.as_str()?;
    Some(normalize_ref(model))
}

/// Walk the parent chain and resolve the 6 face texture names of a model.
/// Returns per-direction texture names (E, W, U, D, S, N), None when unknown.
fn resolve_model_faces(
    models: &HashMap<String, serde_json::Value>,
    model_ref: &str,
) -> [Option<String>; 6] {
    let mut textures: HashMap<String, String> = HashMap::new();
    let mut elements: Option<&serde_json::Value> = None;
    let mut current = Some(model_ref.to_string());
    for _ in 0..16 {
        let Some(name) = current else { break };
        let Some(model) = models.get(&name) else {
            break;
        };
        if let Some(tex) = model.get("textures").and_then(|t| t.as_object()) {
            for (k, v) in tex {
                if let Some(s) = v.as_str() {
                    textures.insert(k.clone(), normalize_ref(s));
                }
            }
        }
        if elements.is_none() {
            if let Some(el) = model.get("elements") {
                if el.as_array().is_some_and(|a| !a.is_empty()) {
                    elements = Some(el);
                }
            }
        }
        match model.get("parent").and_then(|p| p.as_str()) {
            Some(p) => current = Some(normalize_ref(p)),
            None => break,
        }
    }

    const FACE_KEYS: [(&str, usize); 6] = [
        ("east", 0),
        ("west", 1),
        ("up", 2),
        ("down", 3),
        ("south", 4),
        ("north", 5),
    ];
    let mut faces: [Option<String>; 6] = std::array::from_fn(|_| None);
    if let Some(face_map) = elements
        .and_then(|el| el.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.get("faces"))
        .and_then(|f| f.as_object())
    {
        for (key, dir_idx) in FACE_KEYS {
            if let Some(tex) = face_map
                .get(key)
                .and_then(|f| f.get("texture"))
                .and_then(|t| t.as_str())
            {
                faces[dir_idx] = resolve_tex(&textures, tex);
            }
        }
        // side texture fill for models that only define some faces
        let side = faces[0].clone().or_else(|| faces[5].clone());
        for i in 0..6 {
            if faces[i].is_none() {
                if let Some(s) = &side {
                    faces[i] = Some(s.clone());
                }
            }
        }
        return faces;
    }

    // Cube template models without elements: resolve by texture key conventions.
    const CANDIDATES: [(&[&str], usize); 6] = [
        (&["east", "side", "all", "particle"], 0),
        (&["west", "side", "all", "particle"], 1),
        (&["up", "end_top", "top", "end", "all", "particle"], 2),
        (
            &["down", "end_bottom", "bottom", "end", "all", "particle"],
            3,
        ),
        (&["south", "side", "all", "particle"], 4),
        (&["north", "side", "all", "particle"], 5),
    ];
    for (keys, dir_idx) in CANDIDATES {
        for key in keys {
            if let Some(tex) = textures.get(*key) {
                faces[dir_idx] = Some(tex.clone());
                break;
            }
        }
    }
    faces
}

fn resolve_tex(textures: &HashMap<String, String>, tex: &str) -> Option<String> {
    if let Some(hash) = tex.strip_prefix('#') {
        return textures.get(hash).cloned();
    }
    Some(normalize_ref(tex))
}

/// Build a texture-aware block mapper: atlas face tiles + tint/transparent metadata
/// (from the builtin palette) with graceful color fallback for anything missing.
pub fn mapper_with_atlas(atlas: &Atlas) -> BlockMapper {
    BlockMapper::from_atlas(&builtin_meta(), atlas)
}

/// block base name -> (tint color applied to all faces, transparent)
pub fn builtin_meta() -> HashMap<String, ([f32; 3], bool)> {
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
        ("birch_leaves", "#6E9C4A", true),
        ("spruce_leaves", "#3E6B2E", true),
        ("jungle_leaves", "#3FA142", true),
        ("acacia_leaves", "#5A9C2F", true),
        ("dark_oak_leaves", "#2E5C1E", true),
        ("mangrove_leaves", "#4C8A3A", true),
        ("cherry_leaves", "#E8A9C8", true),
        ("glass", "#C8E8F0", true),
    ];
    let tint = |hex: &str| -> [f32; 3] {
        let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x808080);
        Rgba([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]);
        [
            ((v >> 16) & 0xFF) as f32 / 255.0,
            ((v >> 8) & 0xFF) as f32 / 255.0,
            (v & 0xFF) as f32 / 255.0,
        ]
    };
    entries
        .iter()
        .map(|(n, c, t)| (n.to_string(), (tint(c), *t)))
        .collect()
}

/// Extract `elements` boxes from a model chain: (from, to, face texture names).
/// Returns None when the model resolves to a plain cube (no elements).
fn resolve_model_boxes(
    models: &HashMap<String, serde_json::Value>,
    model_ref: &str,
) -> Option<Vec<([f32; 3], [f32; 3], [Option<String>; 6])>> {
    let mut textures: HashMap<String, String> = HashMap::new();
    let mut elements: Option<&Vec<serde_json::Value>> = None;
    let mut current = Some(model_ref.to_string());
    for _ in 0..16 {
        let Some(name) = current else { break };
        let Some(model) = models.get(&name) else {
            break;
        };
        if let Some(tex) = model.get("textures").and_then(|t| t.as_object()) {
            for (k, v) in tex {
                if let Some(s) = v.as_str() {
                    textures.insert(k.clone(), normalize_ref(s));
                }
            }
        }
        if elements.is_none() {
            if let Some(el) = model.get("elements").and_then(|e| e.as_array()) {
                if !el.is_empty() {
                    elements = Some(el);
                }
            }
        }
        match model.get("parent").and_then(|p| p.as_str()) {
            Some(p) => current = Some(normalize_ref(p)),
            None => break,
        }
    }
    let elements = elements?;

    const FACE_KEYS: [(&str, usize); 6] = [
        ("east", 0),
        ("west", 1),
        ("up", 2),
        ("down", 3),
        ("south", 4),
        ("north", 5),
    ];
    let f = |v: &serde_json::Value| -> f32 { v.as_f64().map(|x| x as f32).unwrap_or(0.0) };
    let mut out = Vec::with_capacity(elements.len());
    for el in elements {
        let from = [f(&el["from"][0]), f(&el["from"][1]), f(&el["from"][2])];
        let to = [f(&el["to"][0]), f(&el["to"][1]), f(&el["to"][2])];
        let mut face_names: [Option<String>; 6] = std::array::from_fn(|_| None);
        if let Some(faces) = el.get("faces").and_then(|fs| fs.as_object()) {
            for (key, dir_idx) in FACE_KEYS {
                if let Some(tex) = faces
                    .get(key)
                    .and_then(|fc| fc.get("texture"))
                    .and_then(|t| t.as_str())
                {
                    face_names[dir_idx] = resolve_tex(&textures, tex);
                }
            }
        }
        out.push((from, to, face_names));
    }
    Some(out)
}
