mod glb;
mod mesh;
mod resources;
mod world;

use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use world::{TileEntry, TileManifest, WorldRenderer};

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
    texture_pack_path: Option<String>,
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

    // Texture source: explicit jar/zip > auto-detected vanilla jar > color fallback.
    let pack_path = texture_pack_path
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(resources::detect_vanilla_jar());
    let atlas = pack_path.as_deref().and_then(|p| {
        resources::ResourcePack::open(p)
            .map_err(|e| log_pack_error(p, &e))
            .ok()
            .filter(|pack| !pack.is_empty())
            .map(|pack| pack.build_atlas())
    });
    let renderer = WorldRenderer::new(atlas.as_ref());
    let total = regions.len();
    let done = std::sync::Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<(String, Vec<TileEntry>)>();
    // mpsc::Sender is !Sync; wrap so the rayon closure can capture it.
    let tx = std::sync::Mutex::new(tx);

    // Heavy work off the async runtime, parallelized per region with rayon.
    // `done` is shared between the render thread and the progress loop below.
    let done_thread = done.clone();
    let out_dir_thread = out_dir.clone();
    let handle = std::thread::spawn(move || {
        let all_tiles: Vec<Vec<TileEntry>> = regions
            .par_iter()
            .map(|(rx, rz, path)| {
                let label = format!("r.{rx}.{rz}");
                let tiles = renderer
                    .render_region(path, &out_dir_thread, *rx, *rz)
                    .unwrap_or_default();
                done_thread.fetch_add(1, Ordering::Relaxed);
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send((label, tiles.clone()));
                }
                tiles
            })
            .collect();
        all_tiles
    });

    while let Ok((label, _)) = rx.recv() {
        let d = done.load(Ordering::Relaxed);
        let _ = app.emit(
            "render-progress",
            ProgressEvent {
                done: d,
                total,
                current_region: label,
            },
        );
    }

    let all_tiles = handle.join().map_err(|_| "render thread panicked")?;
    let tiles: Vec<TileEntry> = all_tiles.into_iter().flatten().collect();

    if let Some(atlas) = &atlas {
        if let Ok(png) = atlas.to_png() {
            let _ = std::fs::write(out_dir.join("atlas.png"), png);
        }
    }

    let manifest = TileManifest {
        world_name: world_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "world".into()),
        tiles: tiles.clone(),
    };
    let manifest_path = out_dir.join("settings.json");
    serde_json::to_writer_pretty(
        std::fs::File::create(&manifest_path).map_err(|e| e.to_string())?,
        &manifest,
    )
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "render-done",
        DoneEvent {
            manifest_path: manifest_path.to_string_lossy().to_string(),
            tiles: tiles.len(),
        },
    );

    Ok(serde_json::json!({
        "manifestPath": manifest_path.to_string_lossy(),
        "tileCount": tiles.len(),
        "regionCount": total,
        "textured": atlas.is_some(),
        "textureSource": pack_path.map(|p| p.to_string_lossy().to_string()),
    }))
}

fn log_pack_error(path: &Path, e: &str) {
    eprintln!("failed to load texture pack {path:?}: {e}");
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

#[tauri::command]
async fn start_watching(
    app: AppHandle,
    state: State<'_, RenderState>,
    world_dir: String,
) -> Result<(), String> {
    let region_dir = PathBuf::from(&world_dir).join("region");
    if !region_dir.is_dir() {
        return Err(format!("no region directory: {region_dir:?}"));
    }

    std::thread::spawn(move || {
        use notify::{RecursiveMode, Watcher};
        let (wtx, wrx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(wtx) else {
            return;
        };
        if watcher
            .watch(&region_dir, RecursiveMode::NonRecursive)
            .is_err()
        {
            return;
        }
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
    });
    let _ = state;
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
            start_watching
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
