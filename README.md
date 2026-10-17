# Libre-MCWorldMap-Visualizer

Cartographe 3D de mondes Minecraft (inspiré de BlueMap) construit avec **Tauri 2.0** :
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
`<monde>/.libre-mcworldmap-cache/`.

## État (MVP)

- ✅ Parsing Anvil + meshing + export GLB par chunk (validé sur une région de test)
- ✅ Rendu 3D avec lazy loading + contrôles orbital/vol
- ✅ Textures réelles : atlas construit depuis votre jar Minecraft ou un resource pack (`.jar`/`.zip` ou dossier), UVs par face (top/side/bottom), `NearestFilter` pixelisé — voir « Textures » ci-dessous
- ⏳ V1 : multi-threadé complet + UI de progression fine
- ⏳ V2 : live update (watcher branché), LOD, Nether/End, modèles non-cubiques (escaliers, dalles), animations d'eau
- ⏳ V3 : marqueurs, export statique partageable

## Textures

Les textures de Minecraft sont copyrightées (Mojang) : l'app ne les embarque pas et ne les
redistribue pas. Elle les lit localement, dans cet ordre de priorité :

1. Le pack de textures choisi via « Pack de textures » (`.jar`/`.zip` ou dossier de resource pack)
2. Le jar vanilla détecté automatiquement (`~/.minecraft/versions/<v>/<v>.jar` ou `%APPDATA%\.minecraft\...`)
3. Aucune source trouvée → tuile grise de secours (le rendu reste fonctionnel)

Le pipeline : PNG de `textures/block/` → atlas unique (tuiles 16×16, textures animées tronquées
à leur première frame) → mapping `blockstates/*.json` → modèles → texture par face (haut/côtés/bas).
L'atlas est embarqué dans chaque GLB (`TEXCOORD_0` + PNG dans le binaire glTF). L'atlas est aussi
écrit dans `<monde>/.libre-mcworldmap-cache/atlas.png` à des fins d'inspection.
