<template>
  <div class="app">
    <div ref="canvasHost" class="canvas-host"></div>

    <div v-if="phase === 'idle'" class="overlay panel">
      <h1>Libre-MCWorldMap-Visualizer</h1>
      <p>Sélectionnez un dossier de monde Minecraft (contenant <code>region/</code>).</p>
      <button :disabled="busy" @click="pickWorld">Ouvrir un monde…</button>
      <div class="pack-row">
        <button class="small" :disabled="busy" @click="pickTexturePack">{{ texturePackPath ? 'Changer le pack de textures…' : 'Pack de textures (optionnel)…' }}</button>
        <span v-if="texturePackPath" class="pack-name">{{ texturePackName }}</span>
      </div>
      <p v-if="textureSource" class="hint">Textures : {{ textureSource }}</p>
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
        <button class="small" @click="toggleControls">{{ flyMode ? 'Mode orbital' : 'Mode vol libre' }}</button>
      </div>
      <div class="hud-bottom">
        <span>Clic gauche : orbite · Molette : zoom · Clic droit : pan</span>
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
const texturePackPath = ref('')
const textureSource = ref('')

const texturePackName = computed(() => texturePackPath.value.split(/[\\/]/).pop() || '')

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

async function pickTexturePack() {
  const file = await open({
    multiple: false,
    title: 'Jar Minecraft ou resource pack (.jar/.zip)',
    filters: [{ name: 'Minecraft jar / resource pack', extensions: ['jar', 'zip'] }],
  })
  if (typeof file === 'string') texturePackPath.value = file
}

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
      textured: boolean
      textureSource: string | null
    }>('render_world', {
      worldDir: dir,
      texturePackPath: texturePackPath.value || null,
    })
    textureSource.value = result.textureSource ?? ''
    cacheDir = result.manifestPath.replace(/[/\\]settings\.json$/, '')
    const manifest = await invoke<Manifest>('load_manifest', { manifestPath: result.manifestPath })
    worldName.value = manifest.world_name
    totalTiles.value = manifest.tiles.length
    for (const t of manifest.tiles) tilesByXZ.set(`${t.x},${t.z}`, t)
    phase.value = 'viewing'
    await invoke('start_watching', { worldDir: dir }).catch(() => {})
  } catch (e) {
    error.value = String(e)
    phase.value = 'idle'
  } finally {
    busy.value = false
  }
}

function tileVisible(t: TileEntry, camX: number, camZ: number): boolean {
  const centerX = (t.min[0] + t.max[0]) / 2
  const centerZ = (t.min[2] + t.max[2]) / 2
  const dx = (centerX - camX) / 16
  const dz = (centerZ - camZ) / 16
  return dx * dx + dz * dz <= VIEW_RADIUS_CHUNKS * VIEW_RADIUS_CHUNKS
}

async function loadTile(t: TileEntry) {
  const key = `${t.x},${t.z}`
  if (loaded.has(key) || loading.has(key)) return
  loading.add(key)
  try {
    const bytes = await invoke<number[]>('read_glb', { path: `${cacheDir}/${t.file}` })
    const buf = new Uint8Array(bytes).buffer
    const gltf = await gltfLoader.parseAsync(buf, '')
    gltf.scene.traverse((o) => {
      if ((o as THREE.Mesh).isMesh) {
        const m = o as THREE.Mesh
        const mat = m.material as THREE.MeshStandardMaterial
        if (mat?.map) {
          mat.map.magFilter = THREE.NearestFilter
          mat.map.minFilter = THREE.NearestMipmapLinearFilter
          mat.map.generateMipmaps = true
          mat.map.wrapS = THREE.ClampToEdgeWrapping
          mat.map.wrapT = THREE.ClampToEdgeWrapping
          mat.map.colorSpace = THREE.SRGBColorSpace
          mat.map.anisotropy = 4
        }
        if (mat) {
          mat.vertexColors = true
          const isWater = mat.name === 'water'
          mat.alphaTest = isWater ? 0 : 0.5
          mat.transparent = isWater
          mat.depthWrite = !isWater
          mat.side = isWater ? THREE.FrontSide : THREE.DoubleSide
        }
      }
    })
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
  const camX = camera.position.x
  const camZ = camera.position.z
  for (const t of tilesByXZ.values()) {
    if (tileVisible(t, camX, camZ)) void loadTile(t)
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
  const skyTop = new THREE.Color(0x2a5faf)
  const skyHorizon = new THREE.Color(0xbcd8ee)
  const skyGeo = new THREE.SphereGeometry(2000, 16, 12)
  const skyMat = new THREE.ShaderMaterial({
    side: THREE.BackSide,
    depthWrite: false,
    fog: false,
    uniforms: {
      top: { value: skyTop },
      bottom: { value: skyHorizon },
      offset: { value: 60 },
      exponent: { value: 0.7 },
    },
    vertexShader: `
      varying vec3 vWorldPosition;
      void main() {
        vec4 wp = modelMatrix * vec4(position, 1.0);
        vWorldPosition = wp.xyz;
        gl_Position = projectionMatrix * viewMatrix * wp;
      }
    `,
    fragmentShader: `
      uniform vec3 top;
      uniform vec3 bottom;
      uniform float offset;
      uniform float exponent;
      varying vec3 vWorldPosition;
      void main() {
        float h = normalize(vWorldPosition + offset).y;
        gl_FragColor = vec4(mix(bottom, top, max(pow(max(h, 0.0), exponent), 0.0)), 1.0);
      }
    `,
  })
  const sky = new THREE.Mesh(skyGeo, skyMat)
  scene.add(sky)
  scene.fog = new THREE.Fog(0xbcd8ee, 200, 900)

  camera = new THREE.PerspectiveCamera(70, host.clientWidth / host.clientHeight, 0.1, 3000)
  camera.position.set(80, 120, 80)

  orbit = new OrbitControls(camera, renderer.domElement)
  orbit.target.set(8, 32, 8)
  orbit.enableDamping = true
  orbit.maxPolarAngle = Math.PI / 2 - 0.02

  fly = new PointerLockControls(camera, renderer.domElement)
  scene.add(fly.object ?? camera)

  const sun = new THREE.DirectionalLight(0xffffff, 2.5)
  sun.position.set(0.5, 1, 0.3).multiplyScalar(100)
  scene.add(sun)
  scene.add(new THREE.AmbientLight(0xffffff, 1.2))

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
.pack-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  margin-top: 0.75rem;
}
.pack-row .small {
  margin-top: 0;
}
.pack-name {
  font-size: 12px;
  color: #9fb2cc;
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.hint {
  margin: 0.5rem 0 0;
  font-size: 12px;
  color: #9fb2cc;
  max-width: 400px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
