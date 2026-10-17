<template>
  <div class="app">
    <div ref="canvasHost" class="canvas-host"></div>

    <div v-if="phase === 'idle'" class="overlay panel">
      <h1>Libre-MCWorldMap-Visualizer</h1>
      <p>Sélectionnez un dossier de monde Minecraft (contenant <code>region/</code>).</p>
      <button :disabled="busy" @click="pickWorld">Ouvrir un monde…</button>
      <p v-if="error" class="error">{{ error }}</p>
    </div>

    <div v-else-if="phase === 'rendering'" class="overlay panel">
      <h2>Rendu en cours…</h2>
      <div class="progress">
        <div class="bar" :style="{ width: progressPct + '%' }"></div>
      </div>
      <p>{{ done }} / {{ total }} régions — {{ currentRegion }}</p>
    </div>

    <div v-else class="hud">
      <div class="hud-top">
        <span class="title">{{ worldName }} — {{ loadedTiles }} / {{ totalTiles }} tuiles</span>
        <div class="hud-actions">
          <button class="small" @click="toggleControls">{{ flyMode ? 'Mode orbital' : 'Mode vol libre' }}</button>
          <button class="small" :disabled="busy" @click="diagnoseFormat">Diagnostiquer le format</button>
        </div>
      </div>
      <div class="hud-bottom">
        <span>Clic gauche : orbite · Molette : zoom · Clic droit : pan</span>
        <pre v-if="dumpReport" class="dump">{{ dumpReport }}</pre>
        <span v-if="renderStats">chunks : {{ renderStats.chunksRead }} lus, {{ renderStats.chunksParsed }} parsés, {{ renderStats.chunksEmpty }} vides, {{ renderStats.chunksFailed }} échoués</span>
        <span v-if="renderStats?.parseErrorSample" class="error">{{ renderStats.parseErrorSample }}</span>
        <span v-if="flyMode">ZQSD/WASD + souris pour voler</span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, computed } from 'vue'
import * as THREE from 'three'
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js'
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js'
import { PointerLockControls } from 'three/examples/jsm/controls/PointerLockControls.js'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'

interface TileEntry {
  file: string
  x: number
  z: number
  min: [number, number, number]
  max: [number, number, number]
}

interface Manifest {
  world_name: string
  tiles: TileEntry[]
}

interface RenderStats {
  chunksRead: number
  chunksParsed: number
  chunksEmpty: number
  chunksFailed: number
  parseErrorSample: string | null
}

const canvasHost = ref<HTMLDivElement | null>(null)
const phase = ref<'idle' | 'rendering' | 'viewing'>('idle')
const busy = ref(false)
const error = ref('')
const done = ref(0)
const total = ref(0)
const currentRegion = ref('')
const worldName = ref('')
const totalTiles = ref(0)
const loadedTiles = ref(0)
const flyMode = ref(false)
const renderStats = ref<RenderStats | null>(null)
const dumpReport = ref('')

async function diagnoseFormat() {
  busy.value = true
  error.value = ''
  try {
    const dir = await open({ directory: true, multiple: false, title: 'Dossier du monde Minecraft' })
    if (!dir || typeof dir !== 'string') return
    dumpReport.value = await invoke<string>('dump_chunk', { worldDir: dir })
  } catch (e) {
    error.value = String(e)
  } finally {
    busy.value = false
  }
}

const progressPct = computed(() => (total.value === 0 ? 0 : Math.round((done.value / total.value) * 100)))

let renderer: THREE.WebGLRenderer | null = null
let scene: THREE.Scene
let camera: THREE.PerspectiveCamera
let orbit: OrbitControls
let fly: PointerLockControls
let raf = 0
let cacheDir = ''
const gltfLoader = new GLTFLoader()
const loaded = new Set<string>()
const loading = new Set<string>()
const tilesByXZ = new Map<string, TileEntry>()
let unlistenFns: (() => void)[] = []

const VIEW_RADIUS_CHUNKS = 12
const PATH_SEP = navigator.platform.startsWith('Win') ? '\\' : '/'

async function pickWorld() {
  busy.value = true
  error.value = ''
  try {
    const dir = await open({ directory: true, multiple: false, title: 'Dossier du monde Minecraft' })
    if (!dir || typeof dir !== 'string') return
    phase.value = 'rendering'
    const result = await invoke<{
      manifestPath: string
      tileCount: number
      regionCount: number
      stats: RenderStats
    }>('render_world', { worldDir: dir })
    renderStats.value = result.stats
    cacheDir = result.manifestPath.replace(/[/\\]settings\.json$/, '')
    tilesByXZ.clear()
    tilesByCell.clear()
    loadedTiles.value = 0
    const manifest = await invoke<Manifest>('load_manifest', { manifestPath: result.manifestPath })
    worldName.value = manifest.world_name
    totalTiles.value = manifest.tiles.length
    for (const t of manifest.tiles) {
      tilesByXZ.set(`${t.x},${t.z}`, t)
      const key = `${t.x},${t.z}`
      let list = tilesByCell.get(key)
      if (!list) {
        list = []
        tilesByCell.set(key, list)
      }
      list.push(t)
    }
    phase.value = 'viewing'
    // Center the camera on the rendered data (worlds often live in negative coords).
    if (manifest.tiles.length > 0) {
      let minX = Infinity, maxX = -Infinity
      let minY = Infinity, maxY = -Infinity
      let minZ = Infinity, maxZ = -Infinity
      for (const t of manifest.tiles) {
        minX = Math.min(minX, t.min[0]); maxX = Math.max(maxX, t.max[0])
        minY = Math.min(minY, t.min[1]); maxY = Math.max(maxY, t.max[1])
        minZ = Math.min(minZ, t.min[2]); maxZ = Math.max(maxZ, t.max[2])
      }
      const cx = (minX + maxX) / 2
      const cy = (minY + maxY) / 2
      const cz = (minZ + maxZ) / 2
      const span = Math.max(maxX - minX, maxZ - minZ, 64)
      camera.position.set(cx + span * 0.5, cy + span * 0.6, cz + span * 0.5)
      orbit.target.set(cx, cy, cz)
    }
    await invoke('start_watching', { worldDir: dir }).catch(() => {})
  } catch (e) {
    error.value = String(e)
    phase.value = 'idle'
  } finally {
    busy.value = false
  }
}

// Spatial index: chunk cell -> tiles, so per-frame visibility checks only
// touch cells near the camera instead of scanning the whole manifest.
const tilesByCell = new Map<string, TileEntry[]>()
let lastCamCellX = Number.NaN
let lastCamCellZ = Number.NaN

async function loadTile(t: TileEntry) {
  const key = `${t.x},${t.z}`
  if (loaded.has(key) || loading.has(key)) return
  loading.add(key)
  try {
    const bytes = await invoke<number[]>('read_glb', { path: `${cacheDir}${PATH_SEP}${t.file}` })
    const buf = new Uint8Array(bytes).buffer
    const gltf = await gltfLoader.parseAsync(buf, '')
    scene.add(gltf.scene)
    loaded.add(key)
    loadedTiles.value = loaded.size
  } catch {
    // tile failed to load; skip silently
  } finally {
    loading.delete(key)
  }
}

function updateVisibleTiles() {
  // Re-scan only when the camera crosses into a new chunk cell.
  const cellX = Math.floor(camera.position.x / 16)
  const cellZ = Math.floor(camera.position.z / 16)
  if (cellX === lastCamCellX && cellZ === lastCamCellZ) return
  lastCamCellX = cellX
  lastCamCellZ = cellZ

  for (let dz = -VIEW_RADIUS_CHUNKS; dz <= VIEW_RADIUS_CHUNKS; dz++) {
    for (let dx = -VIEW_RADIUS_CHUNKS; dx <= VIEW_RADIUS_CHUNKS; dx++) {
      if (dx * dx + dz * dz > VIEW_RADIUS_CHUNKS * VIEW_RADIUS_CHUNKS) continue
      const list = tilesByCell.get(`${cellX + dx},${cellZ + dz}`)
      if (list) for (const t of list) void loadTile(t)
    }
  }
}

function toggleControls() {
  flyMode.value = !flyMode.value
  if (flyMode.value) {
    orbit.enabled = false
    fly.lock()
  } else {
    fly.unlock()
    orbit.enabled = true
  }
}

const flyKeys = new Set<string>()
function onKeyDown(e: KeyboardEvent) {
  flyKeys.add(e.code)
}
function onKeyUp(e: KeyboardEvent) {
  flyKeys.delete(e.code)
}

function animate() {
  raf = requestAnimationFrame(animate)
  if (flyMode.value) {
    const speed = flyKeys.has('ShiftLeft') ? 60 : 20
    const dir = new THREE.Vector3()
    if (flyKeys.has('KeyW') || flyKeys.has('KeyZ')) dir.z -= 1
    if (flyKeys.has('KeyS')) dir.z += 1
    if (flyKeys.has('KeyA') || flyKeys.has('KeyQ')) dir.x -= 1
    if (flyKeys.has('KeyD')) dir.x += 1
    if (flyKeys.has('Space')) dir.y += 1
    if (flyKeys.has('ShiftLeft') && flyKeys.has('KeyW')) dir.y += 1
    if (flyKeys.has('ControlLeft')) dir.y -= 1
    if (dir.lengthSq() > 0) {
      dir.normalize().multiplyScalar(speed * 0.016)
      dir.applyQuaternion(camera.quaternion)
      camera.position.add(dir)
    }
  }
  orbit?.update()
  updateVisibleTiles()
  renderer!.render(scene, camera)
}

onMounted(async () => {
  const host = canvasHost.value!
  renderer = new THREE.WebGLRenderer({ antialias: true })
  renderer.setSize(host.clientWidth, host.clientHeight)
  renderer.setPixelRatio(window.devicePixelRatio)
  host.appendChild(renderer.domElement)

  scene = new THREE.Scene()
  scene.background = new THREE.Color(0x87ceeb)
  scene.fog = new THREE.Fog(0x87ceeb, 200, 900)

  camera = new THREE.PerspectiveCamera(70, host.clientWidth / host.clientHeight, 0.1, 3000)
  camera.position.set(80, 120, 80)

  orbit = new OrbitControls(camera, renderer.domElement)
  orbit.target.set(8, 32, 8)
  orbit.enableDamping = true
  orbit.maxPolarAngle = Math.PI / 2 - 0.02

  fly = new PointerLockControls(camera, renderer.domElement)
  scene.add(fly.object ?? camera)

  const sun = new THREE.DirectionalLight(0xffffff, 1.6)
  sun.position.set(0.5, 1, 0.3).multiplyScalar(100)
  scene.add(sun)
  scene.add(new THREE.AmbientLight(0xb0c4de, 0.9))

  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('keyup', onKeyUp)

  animate()

  unlistenFns.push(
    await listen<{ done: number; total: number; current_region: string }>('render-progress', (e) => {
      done.value = e.payload.done
      total.value = e.payload.total
      currentRegion.value = e.payload.current_region
    }),
  )
  unlistenFns.push(
    await listen('render-done', () => {
      // handled by command return value
    }),
  )
  unlistenFns.push(
    await listen('world-changed', () => {
      // V2: live update would re-render changed regions; MVP keeps the static render.
    }),
  )

  const onResize = () => {
    if (!renderer) return
    renderer.setSize(host.clientWidth, host.clientHeight)
    camera.aspect = host.clientWidth / host.clientHeight
    camera.updateProjectionMatrix()
  }
  window.addEventListener('resize', onResize)
})

onBeforeUnmount(() => {
  cancelAnimationFrame(raf)
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('keyup', onKeyUp)
  for (const fn of unlistenFns) fn()
  renderer?.dispose()
})
</script>

<style scoped>
.app,
.canvas-host {
  position: absolute;
  inset: 0;
  overflow: hidden;
}
.overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(15, 20, 30, 0.7);
}
.panel {
  background: #1e2430;
  color: #e8ecf4;
  border-radius: 12px;
  padding: 2rem 2.5rem;
  text-align: center;
  box-shadow: 0 10px 40px rgba(0, 0, 0, 0.5);
}
.panel h1 {
  margin: 0 0 0.5rem;
  font-size: 1.4rem;
}
.panel button {
  margin-top: 1rem;
  padding: 0.6rem 1.4rem;
  font-size: 1rem;
  border: none;
  border-radius: 8px;
  background: #4f8cff;
  color: white;
  cursor: pointer;
}
.panel button:disabled {
  opacity: 0.5;
  cursor: default;
}
.progress {
  width: 300px;
  height: 12px;
  background: #10141c;
  border-radius: 6px;
  overflow: hidden;
  margin: 1rem auto;
}
.bar {
  height: 100%;
  background: linear-gradient(90deg, #4f8cff, #38d39f);
  transition: width 0.2s;
}
.error {
  color: #ff7b72;
  margin-top: 1rem;
  max-width: 400px;
}
.hud {
  position: absolute;
  inset: 0;
  pointer-events: none;
  color: white;
  font-family: system-ui, sans-serif;
}
.hud-top {
  position: absolute;
  top: 12px;
  left: 12px;
  right: 12px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  pointer-events: auto;
}
.hud-top .title {
  background: rgba(15, 20, 30, 0.6);
  padding: 6px 12px;
  border-radius: 8px;
  font-size: 13px;
}
.hud-bottom {
  position: absolute;
  bottom: 12px;
  left: 12px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  background: rgba(15, 20, 30, 0.6);
  padding: 6px 12px;
  border-radius: 8px;
}
.hud-actions {
  display: flex;
  gap: 8px;
}
pre.dump {
  position: absolute;
  bottom: 70px;
  left: 12px;
  max-width: 90%;
  max-height: 40vh;
  overflow: auto;
  background: rgba(10, 14, 20, 0.85);
  color: #9ad;
  font-size: 11px;
  padding: 10px;
  border-radius: 8px;
  pointer-events: auto;
  white-space: pre-wrap;
}
button.small {
  pointer-events: auto;
  padding: 6px 12px;
  border: none;
  border-radius: 8px;
  background: #2c3546;
  color: white;
  cursor: pointer;
  font-size: 13px;
}
</style>
