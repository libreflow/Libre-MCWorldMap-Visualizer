# bluemap-tauri

Cartographe 3D de mondes Minecraft (type BlueMap) construit avec **Tauri 2.0** :
le backend Rust lit les fichiers de région Anvil et exporte des tuiles GLB,
le frontend three.js les affiche dans une carte 3D navigable.

## Architecture

- **Backend Rust** (`src-tauri/src/`)
  - `world.rs` — parsing Anvil (`fastanvil`/`fastnbt`), meshing par chunk avec face culling, manifest `settings.json`
  - `mesh.rs` — mapping block states → cubes colorés (palette minimaliste) + éclairage par face (style Minecraft)
  - `glb.rs` — export glTF 2.0 binaire (GLB) sans dépendance externe
  - `lib.rs` — commandes Tauri (`render_world`, `load_manifest`, `read_glb`, `start_watching`), parallélisation `rayon`, progression via events, watcher `notify` sur `region/`
- **Frontend** (`src/`) — Vue 3 + three.js : GLTFLoader, lazy loading des tuiles autour de la caméra, OrbitControls + mode vol libre (PointerLockControls), fog + skybox

## Développement

```bash
npm install
npm run tauri dev
```

Sélectionnez le dossier d'un monde Minecraft (contenant `region/*.mca`),
le rendu s'affiche progressivement. Les tuiles sont écrites dans
`<monde>/.bluemap-tauri-cache/`.

## État (MVP)

- ✅ Parsing Anvil + meshing + export GLB par chunk (validé sur une région de test)
- ✅ Rendu 3D avec lazy loading + contrôles orbital/vol
- ⏳ V1 : multi-threadé complet + UI de progression fine
- ⏳ V2 : live update (watcher branché), LOD, Nether/End, resource packs
- ⏳ V3 : marqueurs, export statique partageable
