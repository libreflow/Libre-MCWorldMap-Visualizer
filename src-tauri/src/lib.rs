mod glb;
mod mesh;
mod world;

use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use world::{RenderStats, TileEntry, TileManifest, WorldRenderer};

#[derive(Default)]
pub struct RenderState {
    out_dir: Mutex<Option<PathBuf>>,
}

#[derive(Clone, Serialize)]
struct ProgressEvent {
    done: usize,
    total: usize,
    current_region: String,
}

#[derive(Clone, Serialize)]
struct DoneEvent {
    manifest_path: String,
    tiles: usize,
}

#[tauri::command]
async fn render_world(
    app: AppHandle,
    state: State<'_, RenderState>,
    world_dir: String,
) -> Result<serde_json::Value, String> {
    let world_path = PathBuf::from(&world_dir);
    if !world_path.is_dir() {
        return Err(format!("world directory not found: {world_dir}"));
    }

    let regions = world::find_regions(&world_path)?;
    if regions.is_empty() {
        return Err("no region files found (is this a Minecraft world?)".into());
    }

    let out_dir = world_path.join(".libre-mcworldmap-cache");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    *state.out_dir.lock().await = Some(out_dir.clone());

    let total = regions.len();
    let done = std::sync::Arc::new(AtomicUsize::new(0));
    let stats = std::sync::Arc::new(std::sync::Mutex::new(RenderStats::default()));
    let app_handle = app.clone();

    // Heavy work off the async runtime (spawn_blocking), parallelized per
    // region with rayon. Progress events are emitted directly from the
    // worker closures: AppHandle is Send + Sync.
    let world_name = world_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "world".into());

    let out_dir_blocking = out_dir.clone();
    let (all_tiles, final_stats) = tokio::task::spawn_blocking(move || {
        let done_thread = done.clone();
        let stats_thread = stats.clone();
        let all_tiles: Vec<Vec<TileEntry>> = regions
            .par_iter()
            .map(|(rx, rz, path)| {
                let label = format!("r.{rx}.{rz}");
                let mut local_stats = RenderStats::default();
                let result =
                    WorldRenderer::new().render_region(path, &out_dir_blocking, *rx, *rz, &mut local_stats);
                let tiles = result.unwrap_or_default();
                if let Ok(mut s) = stats_thread.lock() {
                    s.chunks_read += local_stats.chunks_read;
                    s.chunks_parsed += local_stats.chunks_parsed;
                    s.chunks_empty += local_stats.chunks_empty;
                    s.chunks_failed += local_stats.chunks_failed;
                    if s.parse_error_sample.is_none() {
                        s.parse_error_sample = local_stats.parse_error_sample;
                    }
                }
                let d = done_thread.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = app_handle.emit(
                    "render-progress",
                    ProgressEvent { done: d, total, current_region: label },
                );
                tiles
            })
            .collect();
        let final_stats = stats.lock().map(|s| s.clone()).unwrap_or_default();
        (all_tiles, final_stats)
    })
    .await
    .map_err(|e| format!("render task failed: {e}"))?;

    let tiles: Vec<TileEntry> = all_tiles.into_iter().flatten().collect();

    let manifest = TileManifest { world_name, tiles: tiles.clone() };
    let manifest_path = out_dir.join("settings.json");
    serde_json::to_writer_pretty(
        std::fs::File::create(&manifest_path).map_err(|e| e.to_string())?,
        &manifest,
    )
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "render-done",
        DoneEvent { manifest_path: manifest_path.to_string_lossy().to_string(), tiles: tiles.len() },
    );

    Ok(serde_json::json!({
        "manifestPath": manifest_path.to_string_lossy(),
        "tileCount": tiles.len(),
        "regionCount": total,
        "stats": {
            "chunksRead": final_stats.chunks_read,
            "chunksParsed": final_stats.chunks_parsed,
            "chunksEmpty": final_stats.chunks_empty,
            "chunksFailed": final_stats.chunks_failed,
            "parseErrorSample": final_stats.parse_error_sample,
        },
    }))
}

#[tauri::command]
async fn load_manifest(manifest_path: String) -> Result<serde_json::Value, String> {
    let path = Path::new(&manifest_path);
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str::<TileManifest>(&data)
        .map(|m| serde_json::to_value(m).unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_glb(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| format!("failed to read {path}: {e}"))
}

/// Format diagnostics: dump the raw NBT structure of the first readable
/// chunk in the first region of the given world. Returns a text report.
#[tauri::command]
async fn dump_chunk(world_dir: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let world_path = PathBuf::from(&world_dir);
        let regions = world::find_regions(&world_path)?;
        let (rx, rz, ref region_path) = regions[0];
        let mut region = world::open_region(region_path)?;
        for lz in 0..32 {
            for lx in 0..32 {
                match region.read_chunk(lx, lz) {
                    Ok(Some(data)) => {
                        let header = format!(
                            "region r.{rx}.{rz}, chunk {lx},{lz} (world chunk {},{}), {} bytes\n",
                            rx * 32 + lx as i32,
                            rz * 32 + lz as i32,
                            data.len()
                        );
                        let dump = world::dump_chunk_top_level(&data)?;
                        return Ok(format!("{header}{dump}"));
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        return Err(format!("region read error at {lx},{lz}: {e}"));
                    }
                }
            }
        }
        Err("region contains no readable chunks".into())
    })
    .await
    .map_err(|e| format!("dump task failed: {e}"))?
}

#[tauri::command]
async fn start_watching(
    app: AppHandle,
    world_dir: String,
) -> Result<(), String> {
    let world_path = PathBuf::from(&world_dir);
    // Watch the same directories the renderer reads from: legacy <world>/region
    // or the 2026 layout <world>/dimensions/minecraft/overworld/region.
    let region_dir = world_path.join("region");
    let region_dir = if region_dir.is_dir() {
        region_dir
    } else {
        let dim = world_path.join("dimensions/minecraft/overworld/region");
        if dim.is_dir() {
            dim
        } else {
            return Err(format!("no region directory under {world_dir}"));
        }
    };

    std::thread::spawn(move || {
        use notify::{RecursiveMode, Watcher};
        let (wtx, wrx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(wtx) else {
            return;
        };
        if watcher.watch(&region_dir, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        // Keep the watcher alive for the lifetime of the channel loop.
        while let Ok(ev) = wrx.recv() {
            if let Ok(e) = ev {
                let paths: Vec<String> = e
                    .paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                let _ = app.emit("world-changed", serde_json::json!({ "paths": paths }));
            }
        }
        drop(watcher);
    });
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(RenderState::default())
        .invoke_handler(tauri::generate_handler![
            render_world,
            load_manifest,
            read_glb,
            dump_chunk,
            start_watching
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
