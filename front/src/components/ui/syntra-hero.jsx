// The syntra hero, ported to Preact + plain JS (standard style). Structure and motion
// follow the original single-file component; the palette and type follow the page's
// design plan (board colours: silkscreen paper, solder-mask ink, activity amber).
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import {
  AnimatePresence, LazyMotion, animate, domAnimation, m,
  useInView, useMotionValue, useReducedMotion, useSpring, useTransform
} from 'motion/react'
import {
  ArrowDownToLine, ArrowUpFromLine, Boxes, Globe, Layers, Menu,
  MousePointer2, Network, Router, Workflow, X
} from 'lucide-react'
import { cn } from 'cn'

// Block stylesheet. Inlined because the published component is a single file.
const BLOCK_CSS = `
:root {
  --color-n3-page: #eaeefc;
  --color-n3-ink: #16211e;
  --color-n3-grey: #5c6b64;
  --color-n3-line: #c6cfc9;
  --color-n3-accent: #16211e;
  --font-n3-sans: 'Archivo', ui-sans-serif, system-ui, sans-serif;
}

.n3-root {
  --n3-gutter: 20px;
  --n3-nav-space: 76px;
}

@media (min-width: 640px) {
  .n3-root {
    --n3-gutter: 32px;
  }
}

@media (min-width: 1024px) {
  .n3-root {
    --n3-gutter: 56px;
  }
}

.n3-frame {
  padding-left: var(--n3-gutter);
  padding-right: var(--n3-gutter);
}

.n3-hero {
  min-height: calc(100dvh - var(--n3-nav-space));
}

@media (max-width: 1023px) {
  .n3-hero {
    min-height: 0;
  }
}

.n3-h1 {
  font-size: clamp(2.5rem, 1.1rem + 3.6vw, 4.35rem);
  line-height: 1.06;
  letter-spacing: -0.035em;
}

.n3-bg {
  background:
    radial-gradient(ellipse 60% 70% at 74% 52%, rgb(255 255 255 / 0.95) 0%, rgb(255 255 255 / 0) 70%),
    radial-gradient(ellipse 40% 45% at 78% 58%, rgb(22 33 30 / 0.05) 0%, rgb(22 33 30 / 0) 70%),
    radial-gradient(ellipse 50% 40% at 100% 0%, rgb(206 216 210 / 0.7) 0%, rgb(206 216 210 / 0) 70%),
    var(--color-n3-page);
}

.n3-halo {
  border-radius: 9999px;
  background: radial-gradient(closest-side, rgb(255 255 255 / 0.9), rgb(255 255 255 / 0));
}

.n3-floor,
.n3-floor-lit {
  --n3-floor-area: radial-gradient(ellipse 75% 80% at 72% 70%, #000 20%, transparent 100%);
  --n3-floor-edges:
    linear-gradient(to left, transparent 0%, #000 22%), linear-gradient(to top, transparent 0%, #000 28%);
  -webkit-mask-image: var(--n3-floor-area), var(--n3-floor-edges);
  mask-image: var(--n3-floor-area), var(--n3-floor-edges);
  -webkit-mask-composite: source-in;
  mask-composite: intersect;
}

.n3-floor {
  opacity: 0.35;
}

.n3-floor-spot {
  position: absolute;
  top: 0;
  left: 0;
  opacity: 0;
  transition: opacity 400ms ease;
  will-change: transform, opacity;
  -webkit-mask-image: radial-gradient(closest-side, #000 0%, transparent 100%);
  mask-image: radial-gradient(closest-side, #000 0%, transparent 100%);
}

.n3-floor-lit {
  position: absolute;
  top: 0;
  left: 0;
  will-change: transform;
}

[data-spot='on'] > .n3-floor-spot {
  opacity: 0.7;
}

@media (max-width: 1023px) {
  .n3-floor,
  .n3-floor-lit {
    --n3-floor-area: radial-gradient(ellipse 90% 60% at 50% 75%, #000 20%, transparent 100%);
  }
}

.n3-fade-edge {
  -webkit-mask-image: linear-gradient(to right, transparent 0%, #000 14%);
  mask-image: linear-gradient(to right, transparent 0%, #000 14%);
}

@media (max-width: 1023px) {
  .n3-fade-edge {
    -webkit-mask-image: none;
    mask-image: none;
  }
}

.n3-nav-bar {
  border-bottom: 1px solid transparent;
  transition:
    background-color 300ms ease,
    border-color 300ms ease;
}

.n3-nav-bar-scrolled {
  background-color: rgb(234 239 236 / 0.82);
  border-bottom-color: rgb(22 33 30 / 0.08);
  -webkit-backdrop-filter: blur(14px);
  backdrop-filter: blur(14px);
}

/* Buttons act like instrument keys: a 1px lift on hover with a soft shadow, a
   press-down on click, and the icon nudging with the action. */
.n3-btn {
  transition:
    transform 180ms cubic-bezier(0.22, 1, 0.36, 1),
    box-shadow 180ms ease,
    background-color 180ms ease,
    border-color 180ms ease;
}

.n3-btn:hover {
  transform: translateY(-1px);
  box-shadow: 0 10px 22px -12px rgb(22 33 30 / 0.45);
}

.n3-btn:active {
  transform: translateY(0) scale(0.985);
  box-shadow: 0 4px 10px -6px rgb(22 33 30 / 0.4);
}

.n3-btn-icon {
  transition: transform 220ms cubic-bezier(0.22, 1, 0.36, 1);
}

.n3-btn:hover .n3-btn-icon {
  transform: translateY(1.5px) scale(1.08);
}

@media (prefers-reduced-motion: reduce) {
  .n3-btn,
  .n3-btn-icon {
    transition: none;
  }

  .n3-btn:hover,
  .n3-btn:active {
    transform: none;
    box-shadow: none;
  }

  .n3-btn:hover .n3-btn-icon {
    transform: none;
  }
}

.n3-dot {
  box-shadow: 0 0 0 4px rgb(22 33 30 / 0.12);
}

.n3-rise {
  animation: n3-rise 700ms cubic-bezier(0.22, 1, 0.36, 1) backwards;
}

.n3-drop {
  animation: n3-drop 600ms cubic-bezier(0.22, 1, 0.36, 1) backwards;
}

.n3-enter {
  animation: n3-enter 1100ms cubic-bezier(0.22, 1, 0.36, 1) 250ms backwards;
}

@keyframes n3-rise {
  from {
    opacity: 0;
    translate: 0 18px;
  }
}

@keyframes n3-drop {
  from {
    opacity: 0;
    translate: 0 -12px;
  }
}

@keyframes n3-enter {
  from {
    opacity: 0;
    scale: 0.97;
  }
}

@media (prefers-reduced-motion: reduce) {
  .n3-rise,
  .n3-drop,
  .n3-enter {
    animation: none;
  }
}

@keyframes n3-float {
  from {
    transform: translateY(0);
  }
  to {
    transform: translateY(calc(var(--n3-float, 6) * -1px));
  }
}

.n3-float {
  animation: n3-float var(--n3-float-seconds, 6.5s) ease-in-out infinite alternate;
}

@keyframes n3-seam {
  from {
    opacity: 0.78;
  }
  to {
    opacity: 1;
  }
}

.n3-seam-pulse {
  animation: n3-seam 4.8s ease-in-out infinite alternate;
}

@media (prefers-reduced-motion: reduce) {
  .n3-float,
  .n3-seam-pulse {
    animation: none;
  }
}

.n3-stage-card {
  border: 1px solid rgb(22 33 30 / 0.08);
  border-radius: 1.3em;
  background: rgb(255 255 255 / 0.82);
  box-shadow:
    0 1px 2px rgb(30 45 40 / 0.05),
    0 14px 34px -18px rgb(45 62 55 / 0.3),
    inset 0 1px 0 rgb(255 255 255 / 0.9);
  -webkit-backdrop-filter: blur(12px);
  backdrop-filter: blur(12px);
  transition:
    border-color 300ms ease,
    box-shadow 300ms ease;
}

.n3-stage-card-lit {
  border-color: rgb(22 33 30 / 0.3);
  box-shadow:
    0 1px 2px rgb(30 45 40 / 0.05),
    0 18px 38px -18px rgb(22 33 30 / 0.35),
    inset 0 1px 0 rgb(255 255 255 / 0.9);
}

@container (max-width: 640px) {
  .n3-stage-apps {
    display: none;
  }
}
`

// ───── geometry ─────
// Everything on the core is a rounded square in a flat plane, projected with the same
// 2:1 isometric matrix, so faces, seams and inlays all agree.

const ISO_X = 0.866
const ISO_Y = 0.5

const CX = 500
const CY = 500

// Isometric ring flattening (true isometric circle height / width).
const RING_FLATTEN = 0.57

// Horizontal radii of the two orbits around the core, inner and outer.
const RING_RX = [400, 530]

// Stack, bottom to top. Each slab sits `gap` above the one below it; the gap is where the light lives.
const BASE = { half: 168, radius: 40, top: 475, depth: 34 }
const SEAM_GAP = 9
const MID = { half: 151, radius: 36, top: BASE.top - SEAM_GAP - 28, depth: 28 }
const CAP = { half: 101, radius: 28, top: MID.top - 14, depth: 14 }

// Server and browser can disagree in the last float digit, which React reports as a hydration mismatch.
const round = (n) => Math.round(n * 100) / 100

const project = (x, y, cx = CX, cy = CY) => {
  return [cx + (x - y) * ISO_X, cy + (x + y) * ISO_Y]
}

// Rounded square in plane coordinates, walked once around.
const footprint = (half, radius, steps = 14) => {
  const r = Math.min(radius, half)
  const c = half - r

  const corners = [
    [c, c, 0],
    [-c, c, 90],
    [-c, -c, 180],
    [c, -c, 270]
  ]

  const pts = []

  for (const [ox, oy, start] of corners) {
    for (let i = 0; i <= steps; i++) {
      const a = ((start + (90 * i) / steps) * Math.PI) / 180

      pts.push([ox + r * Math.cos(a), oy + r * Math.sin(a)])
    }
  }

  return pts
}

const toPath = (pts) => `M${pts.map(([x, y]) => `${x.toFixed(2)} ${y.toFixed(2)}`).join('L')}Z`

const topPath = (slab, inset = 0, cx = CX, dy = 0) => {
  const half = slab.half - inset
  const radius = Math.max(slab.radius - inset, 1)

  return toPath(footprint(half, radius).map(([x, y]) => project(x, y, cx, slab.top + dy)))
}

// A vertically extruded convex shape is the convex hull of its top and bottom outlines.
const hull = (points) => {
  const pts = [...points].sort((a, b) => a[0] - b[0] || a[1] - b[1])
  const cross = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

  const build = (list) => {
    const out = []

    for (const p of list) {
      while (out.length >= 2 && cross(out[out.length - 2], out[out.length - 1], p) <= 0) out.pop()
      out.push(p)
    }

    out.pop()

    return out
  }

  return [...build(pts), ...build([...pts].reverse())]
}

const bodyPath = (slab, inset = 0) => {
  const half = slab.half - inset
  const radius = Math.max(slab.radius - inset, 1)
  const outline = footprint(half, radius)
  const upper = outline.map(([x, y]) => project(x, y, CX, slab.top))
  const lower = outline.map(([x, y]) => project(x, y, CX, slab.top + slab.depth))

  return toPath(hull([...upper, ...lower]))
}

// A point on one of the two visible side faces. u runs along the face, v down its thickness.
const facePoint = (slab, side, u, v) => {
  const { half, top, depth } = slab

  if (side === 'left') {
    const [x, y] = project(-half + 2 * half * u, half, CX, top)

    return [x, y + depth * v]
  }

  const [x, y] = project(half, half - 2 * half * u, CX, top)

  return [x, y + depth * v]
}

// Transform that lays a local (x, y) rectangle onto a side face, or onto a top plane.
const FACE_MATRIX = {
  left: `${ISO_X} ${ISO_Y} 0 1`,
  right: `${ISO_X} ${-ISO_Y} 0 1`,
  top: `${ISO_X} ${ISO_Y} ${-ISO_X} ${ISO_Y}`
}

const ringPoint = (rx, angleDeg) => {
  const a = (angleDeg * Math.PI) / 180

  return [round(CX + rx * Math.cos(a)), round(CY + rx * RING_FLATTEN * Math.sin(a))]
}

const ringPath = (rx) => {
  const ry = rx * RING_FLATTEN

  return `M${CX + rx} ${CY}A${rx} ${ry} 0 1 1 ${CX - rx} ${CY}A${rx} ${ry} 0 1 1 ${CX + rx} ${CY}Z`
}

// ───── core parts ─────
// Each slab is a rounded square in an isometric plane; depth comes from the extruded side
// faces, bevels, and gradients that assume light from the upper left.

const ORANGE = '#16211E'

const FaceMark = ({ slab, side, u, v, w, h, fill, glow }) => {
  const [x, y] = facePoint(slab, side, u, v)

  return (
    <g transform={`translate(${x.toFixed(2)} ${y.toFixed(2)}) matrix(${FACE_MATRIX[side]} 0 0)`}>
      {glow && (
        <rect
          x={-w / 2 - 2}
          y={-h / 2 - 3}
          width={w + 4}
          height={h + 6}
          rx={h}
          fill={fill}
          filter='url(#n3-blur-sm)'
          opacity='0.75'
        />
      )}
      <rect x={-w / 2} y={-h / 2} width={w} height={h} rx={h / 2} fill={fill} />
    </g>
  )
}

// A thin panel line running along a side face.
const PanelLine = ({ slab, side, v, from = 0.14, to = 0.86 }) => {
  const [x1, y1] = facePoint(slab, side, from, v)
  const [x2, y2] = facePoint(slab, side, to, v)

  return <line x1={x1} y1={y1} x2={x2} y2={y2} stroke='#93A29B' strokeWidth='0.9' opacity='0.55' />
}

// A round fastener sunk into a top face, laid in the plane.
const Fastener = ({ slab, x, y }) => {
  const [px, py] = project(x, y, CX, slab.top)

  return (
    <g transform={`translate(${px.toFixed(2)} ${py.toFixed(2)}) matrix(${FACE_MATRIX.top} 0 0)`}>
      <circle r='6.5' fill='url(#n3-hole)' stroke='#B7C4BD' strokeWidth='1' />
    </g>
  )
}

// One RJ45 jack in a side face's local coordinates: shell, pin cavity, two link LEDs.
const Rj45 = ({ slab, side, u, v }) => {
  const [x, y] = facePoint(slab, side, u, v)

  return (
    <g transform={`translate(${x.toFixed(2)} ${y.toFixed(2)}) matrix(${FACE_MATRIX[side]} 0 0)`}>
      <rect x='-11' y='-5.5' width='22' height='11' rx='1.6' fill='#2A332E' />
      <rect x='-8' y='-2.4' width='16' height='6.2' rx='1' fill='#121916' />
      <rect x='-6.5' y='1.4' width='13' height='1.5' rx='0.7' fill='#45524C' />
      <circle cx='-6.8' cy='-8' r='1.15' fill='#21A05F' />
      <circle cx='6.8' cy='-8' r='1.15' fill='#FFB020' />
    </g>
  )
}

// One USB opening in a side face's local coordinates.
const UsbPort = ({ slab, side, u, v, w = 11, h = 5 }) => {
  const [x, y] = facePoint(slab, side, u, v)

  return (
    <g transform={`translate(${x.toFixed(2)} ${y.toFixed(2)}) matrix(${FACE_MATRIX[side]} 0 0)`}>
      <rect x={-w / 2} y={-h / 2} width={w} height={h} rx={1} fill='#2A332E' />
      <rect x={-w / 2 + 0.9} y={-h / 2 + 0.9} width={w - 1.8} height={h - 1.8} rx={0.6} fill='#121916' />
    </g>
  )
}

// Silkscreen text laid flat in a top face's plane.
const Silkscreen = ({ slab, x, y, text, size = 15, fill = '#6B7A72' }) => {
  const [px, py] = project(x, y, CX, slab.top)

  return (
    <g transform={`translate(${px.toFixed(2)} ${py.toFixed(2)}) matrix(${FACE_MATRIX.top} 0 0)`}>
      <text
        textAnchor='middle'
        dominantBaseline='central'
        fontSize={size}
        fontWeight={600}
        letterSpacing={size * 0.18}
        fill={fill}
      >
        {text}
      </text>
    </g>
  )
}

// The finned heatsink that covers the board: a rounded plate of parallel ridges
// laid in the top face's plane, with a small power LED at one corner.
const Heatsink = ({ slab }) => {
  const [px, py] = project(0, 0, CX, slab.top)

  return (
    <g transform={`translate(${px.toFixed(2)} ${py.toFixed(2)}) matrix(${FACE_MATRIX.top} 0 0)`}>
      <rect x='-50' y='-50' width='100' height='100' rx='13' fill='#DCE4DF' stroke='#B7C4BD' strokeWidth='1.2' />
      {[-40, -30, -20, -10, 0, 10, 20, 30, 40].map((fy) => (
        <g key={fy}>
          <rect x='-44' y={fy - 3} width='88' height='6' rx='3' fill='#C3CFC8' />
          <rect x='-44' y={fy - 3} width='88' height='2' rx='1' fill='#EAF0EC' />
        </g>
      ))}
      <circle cx='42' cy='42' r='2.8' fill='#FF7A2F' />
    </g>
  )
}

const SlabBody = ({ slab }) => {
  return (
    <g>
      <path d={bodyPath(slab)} fill='url(#n3-side)' />
      <path d={bodyPath(slab)} fill='url(#n3-side-shade)' />
      <path d={bodyPath(slab)} fill='none' stroke='#C2CCC6' strokeWidth='1' opacity='0.85' />
    </g>
  )
}

const SlabTop = ({ slab, groove }) => {
  return (
    <g>
      <path d={topPath(slab)} fill='url(#n3-bevel)' stroke='#C6CFC9' strokeWidth='1' />
      <path d={topPath(slab, 4.5)} fill='url(#n3-top)' stroke='#fff' strokeWidth='1' opacity='1' />
      {groove !== undefined && <path d={topPath(slab, groove)} fill='none' stroke='#DCE4DF' strokeWidth='1.2' />}
    </g>
  )
}

const CoreDefs = () => {
  return (
    <defs>
      <linearGradient id='n3-side' x1='0' y1='0' x2='1' y2='0'>
        <stop offset='0' stopColor='#F2F6F3' />
        <stop offset='0.46' stopColor='#E7EEE9' />
        <stop offset='0.5' stopColor='#D7E0DA' />
        <stop offset='1' stopColor='#E0E8E3' />
      </linearGradient>
      <linearGradient id='n3-side-shade' x1='0' y1='0' x2='0' y2='1'>
        <stop offset='0.45' stopColor='#46544D' stopOpacity='0' />
        <stop offset='1' stopColor='#46544D' stopOpacity='0.16' />
      </linearGradient>
      <linearGradient id='n3-bevel' x1='0' y1='0' x2='1' y2='1'>
        <stop offset='0' stopColor='#FFFFFF' />
        <stop offset='1' stopColor='#E4EBE7' />
      </linearGradient>
      <linearGradient id='n3-top' x1='0.1' y1='0' x2='0.9' y2='1'>
        <stop offset='0' stopColor='#FFFFFF' />
        <stop offset='1' stopColor='#EFF4F1' />
      </linearGradient>
      <linearGradient id='n3-inlay' x1='0' y1='0' x2='1' y2='1'>
        <stop offset='0' stopColor='#E7EEEA' />
        <stop offset='1' stopColor='#FFFFFF' />
      </linearGradient>
      <linearGradient id='n3-seam' x1='0' y1='0' x2='1' y2='0'>
        <stop offset='0' stopColor='#4A5852' />
        <stop offset='0.5' stopColor={ORANGE} />
        <stop offset='1' stopColor='#2E3B35' />
      </linearGradient>
      <linearGradient id='n3-slot' x1='0' y1='0' x2='0' y2='1'>
        <stop offset='0' stopColor='#45524C' />
        <stop offset='1' stopColor='#7A8A82' />
      </linearGradient>
      <radialGradient id='n3-hole' cx='0.5' cy='0.4' r='0.7'>
        <stop offset='0' stopColor='#C8D2CC' />
        <stop offset='1' stopColor='#EDF2EE' />
      </radialGradient>
      <filter id='n3-blur-sm' x='-40%' y='-40%' width='180%' height='180%'>
        <feGaussianBlur stdDeviation='3.5' />
      </filter>
      <filter id='n3-blur-glow' x='-40%' y='-40%' width='180%' height='180%'>
        <feGaussianBlur stdDeviation='9' />
      </filter>
      <filter id='n3-blur-spill' x='-40%' y='-40%' width='180%' height='180%'>
        <feGaussianBlur stdDeviation='18' />
      </filter>
    </defs>
  )
}

// ───── core object ─────
const ORANGE2 = '#16211E'

// The core at rest: three ceramic slabs (base, mid, cap), with a light seam between the base
// and mid slabs and the mark inlaid on the cap.
const SyntraCoreObject = (props) => {
  const seam = {
    half: MID.half - 2,
    radius: MID.radius - 2,
    top: MID.top + MID.depth - 1,
    depth: SEAM_GAP + 1
  }

  const spillDy = BASE.top - MID.top

  return (
    <svg viewBox='0 0 1000 1000' role='img' aria-label='The NanoPi R5S' {...props}>
      <CoreDefs />

      {/* Base slab: the case bottom with the port row. */}
      <SlabBody slab={BASE} />
      <SlabTop slab={BASE} groove={10} />
      <PanelLine slab={BASE} side='left' v={0.3} />
      <PanelLine slab={BASE} side='right' v={0.3} />
      {/* WAN plus two LAN jacks, each with its link LEDs. */}
      <Rj45 slab={BASE} side='right' u={0.22} v={0.75} />
      <Rj45 slab={BASE} side='right' u={0.5} v={0.75} />
      <Rj45 slab={BASE} side='right' u={0.78} v={0.75} />
      {/* USB and power on the back. */}
      <UsbPort slab={BASE} side='left' u={0.3} v={0.72} />
      <UsbPort slab={BASE} side='left' u={0.46} v={0.72} />
      <UsbPort slab={BASE} side='left' u={0.62} v={0.72} w={6.5} h={2.6} />
      <FaceMark slab={BASE} side='left' u={0.74} v={0.66} w={3} h={3} fill='#21A05F' />

      {/* Light from the seam spills onto the tray and out along its own edge. */}
      <g className='n3-seam-pulse'>
        <path d={topPath(MID, 4, CX, spillDy)} fill={ORANGE2} opacity='0.24' filter='url(#n3-blur-spill)' />
        <path d={bodyPath(seam)} fill={ORANGE2} opacity='0.6' filter='url(#n3-blur-glow)' />
      </g>
      <path d={bodyPath(seam)} fill='url(#n3-seam)' />

      {/* Mid slab. */}
      <SlabBody slab={MID} />
      <SlabTop slab={MID} groove={11} />
      <PanelLine slab={MID} side='left' v={0.78} />
      <PanelLine slab={MID} side='right' v={0.78} />
      <FaceMark slab={MID} side='left' u={0.62} v={0.42} w={34} h={7.5} fill='url(#n3-slot)' />
      <FaceMark slab={MID} side='left' u={0.34} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
      <FaceMark slab={MID} side='left' u={0.42} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
      <FaceMark slab={MID} side='right' u={0.44} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
      <FaceMark slab={MID} side='right' u={0.52} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
      <FaceMark slab={MID} side='right' u={0.68} v={0.42} w={22} h={4.5} fill='url(#n3-slot)' />
      <g className='n3-seam-pulse'>
        <FaceMark slab={MID} side='left' u={0.2} v={0.42} w={7} h={2.6} fill={ORANGE2} glow />
      </g>

      <Fastener slab={MID} x={0} y={-MID.half + 28} />
      <Fastener slab={MID} x={-MID.half + 28} y={0} />
      <Fastener slab={MID} x={MID.half - 28} y={0} />
      <Fastener slab={MID} x={0} y={MID.half - 28} />

      {/* Cap slab with the heatsink and the board name. */}
      <SlabBody slab={CAP} />
      <SlabTop slab={CAP} />
      <path d={topPath(CAP, 19)} fill='url(#n3-inlay)' stroke='#D3DCD6' strokeWidth='1.2' />
      <path d={topPath(CAP, 20.5)} fill='none' stroke='#fff' strokeWidth='1' opacity='0.9' />

      <Heatsink slab={CAP} />
      <Silkscreen slab={CAP} x={0} y={66} text='NanoPi R5S' size={13} />
    </svg>
  )
}

// ───── core shadow ─────
const SyntraCoreShadow = (props) => {
  const floor = { ...BASE, top: BASE.top + BASE.depth + 30 }

  return (
    <svg viewBox='0 0 1000 1000' aria-hidden {...props}>
      <defs>
        <filter id='n3-blur-lg' x='-30%' y='-30%' width='160%' height='160%'>
          <feGaussianBlur stdDeviation='26' />
        </filter>
        <filter id='n3-blur-md' x='-30%' y='-30%' width='160%' height='160%'>
          <feGaussianBlur stdDeviation='9' />
        </filter>
      </defs>
      <path d={topPath(floor, -14)} fill='#4E5C55' opacity='0.2' filter='url(#n3-blur-lg)' />
      <path d={topPath(floor, 6)} fill='#3A4640' opacity='0.22' filter='url(#n3-blur-md)' />
    </svg>
  )
}

// ───── depth backdrop ─────
const NODES = [
  [118, 132],
  [318, 82],
  [690, 96],
  [884, 188],
  [936, 470],
  [70, 512],
  [150, 858],
  [850, 846]
]

const SyntraDepthBackdrop = (props) => {
  return (
    <svg viewBox='0 0 1000 1000' aria-hidden {...props}>
      <defs>
        <linearGradient id='n3-glass' x1='0' y1='0' x2='1' y2='1'>
          <stop offset='0' stopColor='#fff' stopOpacity='0.7' />
          <stop offset='1' stopColor='#fff' stopOpacity='0.12' />
        </linearGradient>
      </defs>

      <g fill='url(#n3-glass)' stroke='#DCE4DF' strokeWidth='1' opacity='0.35'>
        <rect width='290' height='250' rx='22' transform='matrix(0.866 -0.5 0 1 232 300)' />
        <rect width='290' height='250' rx='22' transform='matrix(0.866 0.5 0 1 520 150)' />
      </g>

      <g fill='#fff' stroke='#C2CCC6' strokeWidth='1'>
        {NODES.map(([x, y], i) => (
          <circle key={i} cx={x} cy={y} r={i % 3 === 0 ? 4 : 3} opacity='0.85' />
        ))}
      </g>
    </svg>
  )
}

// ───── config ─────
// The dials: everything a designer is likely to tune lives here, in plain units.

const EASE = [0.22, 1, 0.36, 1]

const COPY = {
  eyebrow: 'Native seL4/Microkit firmware for NanoPi R5S',
  headline: ['A router that runs', 'on seL4, not Linux.'],
  body: 'A standalone non-Linux router: DHCP-configured native WAN, two independently routed RTL8125 LANs (10.44.1.0/24, 10.44.2.0/24), endpoint-preserving NAPT, and fixed stateful forwarding.',
  primaryCta: 'Download the image',
  secondaryCta: 'How to flash it'
}

const NAV_LINKS = [
  { label: 'Download', href: '#download' },
  { label: 'Flash', href: '#flash' },
  { label: 'Architecture', href: '#architecture' },
  { label: 'Performance', href: '#performance' }
]

const METRICS = [
  { icon: 'down', value: '~440 Mbps', label: 'Download' },
  { icon: 'up', value: '~385 Mbps', label: 'Upload' },
  { icon: 'boxes', value: '1024', label: 'Zero-copy DMA buffers, 2 KiB each' }
]

const MOTION = {
  // Vertical drift of the core, in px of the 1000-unit artboard. Larger = more bob.
  floatDistance: 10,

  // Seconds for one float cycle. Larger = calmer.
  floatSeconds: 6.5,

  // Maximum tilt of the core toward the cursor, in degrees. Keep under ~4.
  tiltDegrees: 3.5,

  // How far the rings and dots slide against the core, in px. 0 = no depth.
  ringParallax: 12,
  backdropParallax: 5,

  // Seconds for one lap of the inner (clockwise) and outer (counter-clockwise) orbit.
  orbitSeconds: [70, 95],

  // How visible the orbits and their beads stay while the stack is open. 0 = gone, 1 = unchanged.
  orbitOpenOpacity: 0.15
}

const ARTBOARD = 1000

// The exploded view: bottom to top, a packet's path through the router. Each stage
// names its own icon; the hot path is the main one.
const STAGES = [
  {
    title: 'WAN uplink',
    items: ['One 1 GbE uplink', 'Address from upstream DHCP', 'Hardware TSO and checksums'],
    icon: 'wan',
    side: 'right'
  },
  {
    title: 'Zero-copy pool',
    items: ['1024 × 2 KiB packet buffers', '16-byte Refs, never payloads', 'Strict single-owner handoff'],
    icon: 'pool',
    side: 'left'
  },
  {
    title: 'NAT workers',
    items: ['Endpoint-preserving NAPT', 'One flow pinned to one core', 'Frames prepared on that core'],
    side: 'right',
    main: true
  },
  {
    title: 'Dispatcher & control',
    items: ['DHCP, ARP, bridge learning', 'Copy fallback when a lane fills'],
    icon: 'dispatch',
    side: 'left'
  },
  {
    title: 'LAN 1 + LAN 2',
    items: ['Two 2.5 GbE client ports', '10.44.1.x and 10.44.2.x', 'GRO at the edge'],
    icon: 'lan',
    side: 'right'
  }
]

// The hot path is the main stage: its card carries a "Hot path" tag.
const isMain = (stage) => 'main' in stage && stage.main

const EXPLODE = {
  // Spring that drives the whole reveal. Lower stiffness = slower, softer.
  spring: { stiffness: 105, damping: 25, mass: 1 },

  // Distance between neighbouring plates once apart, in artboard units.
  gap: 94,

  // How far the main plate rides above the rest of the stack, in artboard units.
  aiLift: 12,

  // Height of the top plate's face when apart. Raise to move the whole stack down.
  topPlateY: 230,

  // Later plates start a beat behind the main plate. 0 = all move together.
  stagger: 0.05,

  // Size of each plate when apart, bottom to top, as a fraction of its stacked size.
  scales: [0.63, 0.62, 0.7, 1.07, 1.06]
}

// The stage cards, positioned in percentages of the visual's width per side.
const STAGE_CARDS = {
  left: { edge: -2, width: 31 },
  right: { edge: 0, width: 29 },

  // Cards only show while the stack is open. They start this many px toward the core
  // and ease out to this many px past their spot.
  restInset: 10,
  outward: 2,

  // Seconds for a bead to run from a card to its plate while the stack is open.
  beadSeconds: 1.6
}

// ───── explode layout ─────
// The five plates, one per stage, bottom to top, cut from the three stacked slabs so that,
// together, they are the resting object exactly.
const PLATE_SLABS = [
  { ...BASE, top: BASE.top + 17, depth: 17 },
  { ...BASE, top: BASE.top, depth: 17 },
  MID,
  { ...CAP, top: CAP.top + 7, depth: 7 },
  { ...CAP, top: CAP.top, depth: 7 }
]

const AI_PLATE = 2

// Where each plate's top face sits once apart, and how far it travels to get there.
const TARGET_TOPS = PLATE_SLABS.map(
  (_, i) => EXPLODE.topPlateY + (PLATE_SLABS.length - 1 - i) * EXPLODE.gap - (i === AI_PLATE ? EXPLODE.aiLift : 0)
)

const TRAVEL = PLATE_SLABS.map((slab, i) => TARGET_TOPS[i] - slab.top)

const clamp = (v) => Math.min(1, Math.max(0, v))

// Progress of one plate, 0 stacked to 1 apart. The main plate leads; the rest follow at a stagger.
const plateProgress = (i, p) => {
  const delay = Math.abs(i - AI_PLATE) * EXPLODE.stagger

  return clamp((p - delay) / (1 - delay))
}

const plateY = (i, p) => TRAVEL[i] * plateProgress(i, p)
const plateScale = (i, p) => 1 + (EXPLODE.scales[i] - 1) * plateProgress(i, p)

// Screen x of a plate's left/right corner tip once apart (rounded corners pull the tip in).
const tipOffset = (i) => {
  const { half, radius } = PLATE_SLABS[i]

  return 0.866 * (2 * half - 0.586 * radius) * EXPLODE.scales[i]
}

const tipX = (i, side) => CX + (side === 'right' ? 1 : -1) * tipOffset(i)

// Screen y of the side corners (half way down the plate) once apart.
const cornerY = (i) => TARGET_TOPS[i] + PLATE_SLABS[i].depth / 2

// ───── exploded core ─────
const ORANGE3 = '#16211E'

const clamp2 = (v) => Math.min(1, Math.max(0, v))

// Extra side thickness each plate grows once it is apart, bottom to top.
const SKIRT = [0, 4, 0, 10, 10]

const Plate = ({ index, progress, children }) => {
  const y = useTransform(progress, (v) => plateY(index, v))
  const scale = useTransform(progress, (v) => plateScale(index, v))
  const apart = useTransform(progress, (v) => plateProgress(index, v))
  const shadow = useTransform(apart, (v) => v * 0.16)

  const slab = PLATE_SLABS[index]
  const skirt = SKIRT[index]

  return (
    <m.g style={{ y, scale }}>
      {/* A soft shadow the plate casts on the one below it as they part. */}
      {index > 0 && (
        <m.path
          d={topPath(slab, -6, CX, slab.depth + 34)}
          fill='#4E5C55'
          filter='url(#n3-blur-spill)'
          style={{ opacity: shadow }}
        />
      )}
      {skirt > 0 && (
        <m.g style={{ opacity: apart }}>
          <SlabBody slab={{ ...slab, top: slab.top + slab.depth - 1, depth: skirt + 1 }} />
        </m.g>
      )}
      {children}
    </m.g>
  )
}

// A little light coming up on the main plate: a ring and core in its plane.
const AiEmphasis = ({ progress }) => {
  const on = useTransform(progress, (v) => clamp2((v - 0.5) / 0.4))
  const rim = useTransform(on, (v) => v * 0.9)
  const halo = useTransform(on, (v) => v * 0.55)
  const spill = useTransform(halo, (v) => v * 0.35)

  return (
    <g transform={`translate(${CX} ${MID.top}) matrix(${FACE_MATRIX.top} 0 0)`}>
      <m.circle r='86' fill={ORANGE3} filter='url(#n3-blur-spill)' style={{ opacity: spill }} />
      <m.circle r='56' fill='none' stroke={ORANGE3} strokeWidth='1.4' style={{ opacity: rim }} />
      <m.circle r='38' fill='none' stroke={ORANGE3} strokeWidth='1' strokeDasharray='3 6' style={{ opacity: halo }} />
      <m.circle r='9' fill={ORANGE3} style={{ opacity: rim }} />
    </g>
  )
}

// The core taken apart into its five stage plates, driven by `progress`.
const SyntraExplodedCore = ({ progress, ...props }) => {
  const strongGlow = useTransform(progress, (v) => clamp2((v - 0.4) / 0.5) * 0.6)

  const seam = {
    half: MID.half - 2,
    radius: MID.radius - 2,
    top: MID.top + MID.depth - 1,
    depth: SEAM_GAP + 1
  }

  const inlay = (slab) => (
    <>
      <path d={topPath(slab, 19)} fill='url(#n3-inlay)' stroke='#D3DCD6' strokeWidth='1.2' />
      <path d={topPath(slab, 20.5)} fill='none' stroke='#fff' strokeWidth='1' opacity='0.9' />
    </>
  )

  return (
    <svg viewBox='0 0 1000 1000' aria-hidden {...props}>
      <CoreDefs />

      <Plate index={0} progress={progress}>
        <SlabBody slab={PLATE_SLABS[0]} />
        <SlabTop slab={PLATE_SLABS[0]} groove={10} />
        <Rj45 slab={BASE} side='right' u={0.22} v={0.75} />
        <Rj45 slab={BASE} side='right' u={0.5} v={0.75} />
        <Rj45 slab={BASE} side='right' u={0.78} v={0.75} />
        <UsbPort slab={BASE} side='left' u={0.3} v={0.72} />
        <UsbPort slab={BASE} side='left' u={0.46} v={0.72} />
        <UsbPort slab={BASE} side='left' u={0.62} v={0.72} w={6.5} h={2.6} />
        <FaceMark slab={BASE} side='left' u={0.74} v={0.66} w={3} h={3} fill='#21A05F' />
      </Plate>

      <Plate index={1} progress={progress}>
        <SlabBody slab={PLATE_SLABS[1]} />
        <SlabTop slab={PLATE_SLABS[1]} groove={10} />
        <PanelLine slab={BASE} side='left' v={0.3} />
        <PanelLine slab={BASE} side='right' v={0.3} />
      </Plate>

      <Plate index={AI_PLATE} progress={progress}>
        <g className='n3-seam-pulse'>
          <path d={topPath(MID, 4, CX, BASE.top - MID.top)} fill={ORANGE3} opacity='0.24' filter='url(#n3-blur-spill)' />
          <path d={bodyPath(seam)} fill={ORANGE3} opacity='0.6' filter='url(#n3-blur-glow)' />
        </g>
        <m.path d={bodyPath(seam)} fill={ORANGE3} filter='url(#n3-blur-glow)' style={{ opacity: strongGlow }} />
        <path d={bodyPath(seam)} fill='url(#n3-seam)' />
        <SlabBody slab={MID} />
        <SlabTop slab={MID} groove={11} />
        <PanelLine slab={MID} side='left' v={0.78} />
        <PanelLine slab={MID} side='right' v={0.78} />
        <FaceMark slab={MID} side='left' u={0.62} v={0.42} w={34} h={7.5} fill='url(#n3-slot)' />
        <FaceMark slab={MID} side='left' u={0.34} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
        <FaceMark slab={MID} side='left' u={0.42} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
        <FaceMark slab={MID} side='right' u={0.44} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
        <FaceMark slab={MID} side='right' u={0.52} v={0.42} w={5} h={5} fill='url(#n3-slot)' />
        <FaceMark slab={MID} side='right' u={0.68} v={0.42} w={22} h={4.5} fill='url(#n3-slot)' />
        <g className='n3-seam-pulse'>
          <FaceMark slab={MID} side='left' u={0.2} v={0.42} w={7} h={2.6} fill={ORANGE3} glow />
        </g>
        <Fastener slab={MID} x={0} y={-MID.half + 28} />
        <Fastener slab={MID} x={-MID.half + 28} y={0} />
        <Fastener slab={MID} x={MID.half - 28} y={0} />
        <Fastener slab={MID} x={0} y={MID.half - 28} />
        <AiEmphasis progress={progress} />
      </Plate>

      <Plate index={3} progress={progress}>
        <SlabBody slab={PLATE_SLABS[3]} />
        <SlabTop slab={PLATE_SLABS[3]} />
        {inlay(PLATE_SLABS[3])}
      </Plate>

      <Plate index={4} progress={progress}>
        <SlabBody slab={PLATE_SLABS[4]} />
        <SlabTop slab={PLATE_SLABS[4]} />
        {inlay(CAP)}
        <Heatsink slab={CAP} />
        <Silkscreen slab={CAP} x={0} y={66} text='NanoPi R5S' size={13} />
      </Plate>
    </svg>
  )
}

// ───── orbital system ─────
const SyntraOrbitalSystem = (props) => {
  return (
    <svg viewBox='0 0 1000 1000' aria-hidden {...props}>
      <g fill='none' stroke='#C6CFC9' strokeWidth='1'>
        <ellipse cx={CX} cy={CY} rx={RING_RX[0]} ry={RING_RX[0] * RING_FLATTEN} opacity='0.8' />
        <ellipse cx={CX} cy={CY} rx={RING_RX[1]} ry={RING_RX[1] * RING_FLATTEN} opacity='0.65' />
      </g>
    </svg>
  )
}

// ───── particles ─────
// Small crystal beads spaced evenly around each orbit: four on the inner, five on the outer.
const NODES2 = [
  { ring: 0, angle: 20, size: 7 },
  { ring: 0, angle: 110, size: 5.5 },
  { ring: 0, angle: 200, size: 8 },
  { ring: 0, angle: 290, size: 6 },
  { ring: 1, angle: 60, size: 6 },
  { ring: 1, angle: 132, size: 8 },
  { ring: 1, angle: 204, size: 5.5 },
  { ring: 1, angle: 276, size: 7 },
  { ring: 1, angle: 348, size: 6.5 }
]

// A small sphere of amber glass.
const Crystal = ({ size }) => {
  return (
    <g filter='url(#n3-crystal-shadow)'>
      <circle r={size * 1.55} fill='url(#n3-crystal-halo)' />
      <circle r={size} fill='url(#n3-crystal-body)' />
      <circle r={size} fill='url(#n3-crystal-refract)' />
      <circle r={size - 0.5} fill='none' stroke='url(#n3-crystal-rim)' strokeWidth='1' />
      <ellipse
        cx={-size * 0.32}
        cy={-size * 0.4}
        rx={size * 0.36}
        ry={size * 0.2}
        transform={`rotate(-30 ${-size * 0.32} ${-size * 0.4})`}
        fill='#FFFFFF'
        opacity='0.95'
      />
      <circle cx={size * 0.34} cy={-size * 0.5} r={Math.max(0.7, size * 0.07)} fill='#FFFFFF' opacity='0.85' />
    </g>
  )
}

// The nodes are drawn twice, once behind the core and once in front, each clipped to its half
// of the artboard, so a node passing the back of an orbit slips behind the object.
const SyntraParticles = ({ half, animate, paused = false, ...props }) => {
  const ref = useRef(null)

  const clipId = `n3-clip-${half}`

  // The beads run on this SVG's own animation clock, so pausing the clock stops them all.
  useEffect(() => {
    const svg = ref.current

    if (!svg) return

    if (paused) svg.pauseAnimations()
    else svg.unpauseAnimations()
  }, [paused])

  return (
    <svg ref={ref} viewBox='0 0 1000 1000' aria-hidden {...props}>
      <defs>
        <clipPath id={clipId}>
          {half === 'back'
            ? (
              <rect x='-200' y='-200' width='1400' height={CY + 200} />
              )
            : (
              <rect x='-200' y={CY} width='1400' height='700' />
              )}
        </clipPath>
        <linearGradient id='n3-crystal-body' x1='0.15' y1='0.1' x2='0.85' y2='0.95'>
          <stop offset='0' stopColor='#D7E0DA' stopOpacity='0.9' />
          <stop offset='0.5' stopColor='#5C6B64' stopOpacity='0.88' />
          <stop offset='1' stopColor='#121916' stopOpacity='0.95' />
        </linearGradient>
        <radialGradient id='n3-crystal-refract' cx='0.6' cy='0.78' r='0.45'>
          <stop offset='0' stopColor='#FFF4EC' stopOpacity='0.85' />
          <stop offset='1' stopColor='#FFF4EC' stopOpacity='0' />
        </radialGradient>
        <linearGradient id='n3-crystal-rim' x1='0' y1='0' x2='0' y2='1'>
          <stop offset='0' stopColor='#FFFFFF' stopOpacity='0.95' />
          <stop offset='1' stopColor='#2E3B35' stopOpacity='0.55' />
        </linearGradient>
        <radialGradient id='n3-crystal-halo' cx='0.5' cy='0.5' r='0.5'>
          <stop offset='0.5' stopColor='#16211E' stopOpacity='0.12' />
          <stop offset='1' stopColor='#16211E' stopOpacity='0' />
        </radialGradient>
        <filter id='n3-crystal-shadow' x='-80%' y='-80%' width='260%' height='260%'>
          <feDropShadow dx='0' dy='2.5' stdDeviation='2.5' floodColor='#2E3B35' floodOpacity='0.25' />
        </filter>
      </defs>

      <g clipPath={`url(#${clipId})`}>
        {NODES2.map((node, i) => {
          const rx = RING_RX[node.ring]
          const [x, y] = ringPoint(rx, node.angle)
          const dur = MOTION.orbitSeconds[node.ring]
          const reverse = node.ring === 1

          // The ring path starts at angle 0 and runs clockwise; a reversed lap starts from the far end.
          const start = node.angle / 360
          const begin = -dur * (reverse ? 1 - start : start)

          return (
            <g key={i} transform={animate ? undefined : `translate(${x.toFixed(1)} ${y.toFixed(1)})`}>
              <Crystal size={node.size} />
              {animate && (
                <animateMotion
                  dur={`${dur}s`}
                  begin={`${begin}s`}
                  repeatCount='indefinite'
                  path={ringPath(rx)}
                  {...(reverse && { keyPoints: '1;0', keyTimes: '0;1', calcMode: 'linear' })}
                />
              )}
            </g>
          )
        })}
      </g>
    </svg>
  )
}

// ───── technical backdrop ─────
const DOTS = Array.from({ length: 26 }, (_, i) => {
  const a = (i * 137.5 * Math.PI) / 180
  const r = 160 + ((i * 53) % 340)

  return [
    round(CX + r * Math.cos(a) * 1.02),
    round(CY + r * Math.sin(a) * RING_FLATTEN * 1.6),
    0.8 + (i % 3) * 0.5
  ]
})

const SyntraTechnicalBackdrop = (props) => {
  return (
    <svg viewBox='0 0 1000 1000' aria-hidden {...props}>
      <defs>
        <linearGradient id='n3-axis-v' x1='0' y1='0' x2='0' y2='1'>
          <stop offset='0' stopColor='#A8B5AD' stopOpacity='0' />
          <stop offset='0.5' stopColor='#A8B5AD' stopOpacity='0.45' />
          <stop offset='1' stopColor='#A8B5AD' stopOpacity='0' />
        </linearGradient>
        <linearGradient id='n3-axis-h' x1='0' y1='0' x2='1' y2='0'>
          <stop offset='0' stopColor='#A8B5AD' stopOpacity='0' />
          <stop offset='0.5' stopColor='#A8B5AD' stopOpacity='0.7' />
          <stop offset='1' stopColor='#A8B5AD' stopOpacity='0' />
        </linearGradient>
      </defs>

      <rect x={CX - 0.5} y='110' width='1' height='780' fill='url(#n3-axis-v)' />
      <rect x='70' y={CY - 0.5} width='860' height='1' fill='url(#n3-axis-h)' />

      <g fill='#A8B5AD'>
        {DOTS.map(([x, y, r], i) => (
          <circle key={i} cx={x} cy={y} r={r} opacity={0.35 + (i % 4) * 0.12} />
        ))}
      </g>

      <g stroke='#A8B5AD' strokeWidth='1' opacity='0.5'>
        {(
          [
            [212, 292],
            [790, 690],
            [860, 430],
            [150, 560]
          ]
        ).map(([x, y]) => (
          <path key={`${x}-${y}`} d={`M${x - 5} ${y}h10M${x} ${y - 5}v10`} />
        ))}
      </g>
    </svg>
  )
}

// ───── stage cards ─────
const STAGE_ICONS = {
  wan: Globe,
  pool: Layers,
  dispatch: Workflow,
  lan: Network
}

// Title, tech lines and an optional icon. Sized in em, so the card scales
// with its font size.
const StageCardBody = ({ index }) => {
  const stage = STAGES[index]
  const main = isMain(stage)
  const Icon = stage.icon ? STAGE_ICONS[stage.icon] : null

  return (
    <div>
      <div className='flex flex-wrap items-baseline justify-between gap-x-[0.6em]'>
        <p className='text-(color:--color-n3-ink) text-[1.05em] leading-[1.25] font-semibold tracking-[-0.015em] md:whitespace-nowrap'>
          {stage.title}
        </p>
        {main && (
          <span className='text-(color:--color-n3-accent) shrink-0 text-[0.72em] font-semibold tracking-[0.12em] uppercase'>
            Hot path
          </span>
        )}
      </div>

      <div className='mt-[0.4em] flex items-start gap-[0.55em]'>
        {Icon && (
          <Icon
            className='text-(color:--color-n3-accent) mt-[0.12em] size-[1.25em] shrink-0'
            strokeWidth={1.7}
            aria-hidden
          />
        )}
        <ul className='text-(color:--color-n3-grey) m-0 list-none space-y-[0.15em] p-0 text-[0.88em] leading-[1.45]'>
          {stage.items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      </div>
    </div>
  )
}

const pct = (n) => `${(n / ARTBOARD) * 100}%`

// Cards ease away from the core: left cards to the left, right cards to the right.
const cardDirection = (index) => (STAGES[index].side === 'left' ? -1 : 1)

// One stage card beside its plate. Hidden at rest; fades in and eases outward as the stack opens.
const StageCard = ({ index, progress, open, lit, onHover }) => {
  const opacity = useTransform(progress, [0.35, 0.9], [0, 1])

  const x = useTransform(
    progress,
    [0, 1],
    [-cardDirection(index) * STAGE_CARDS.restInset, cardDirection(index) * STAGE_CARDS.outward]
  )

  const stage = STAGES[index]
  const { edge, width } = STAGE_CARDS[stage.side]
  const left = stage.side === 'left' ? edge : 100 - edge - width

  const handlePointerEnter = (e) => {
    if (e.pointerType === 'mouse') onHover(index)
  }

  const handlePointerLeave = () => {
    onHover(null)
  }

  return (
    <li
      style={{ top: pct(cornerY(index)), left: `${left}%`, width: `${width}%` }}
      className='absolute -translate-y-1/2'
    >
      <m.div
        style={{ opacity, x }}
        onPointerEnter={handlePointerEnter}
        onPointerLeave={handlePointerLeave}
        className={cn(
          'n3-stage-card px-[0.9em] py-[0.85em]',
          open && 'pointer-events-auto',
          lit && 'n3-stage-card-lit'
        )}
      >
        <StageCardBody index={index} />
      </m.div>
    </li>
  )
}

// The five stages as floating cards beside their plates, from tablet up. Font size follows the
// visual's width (cqw), so the cards keep their place against the plates at every size.
const StageCards = ({ progress, open, hovered, onHover }) => {
  return (
    <ul
      aria-label='How the router moves a packet'
      aria-hidden={!open}
      className='pointer-events-none absolute inset-0 m-0 hidden list-none p-0 text-[clamp(10px,1.7cqw,15px)] md:block'
    >
      {STAGES.map((stage, i) => (
        <StageCard key={stage.title} index={i} progress={progress} open={open} lit={hovered === i} onHover={onHover} />
      ))}
    </ul>
  )
}

// ───── stage connectors ─────
const ORANGE4 = '#16211E'

// Where the line meets the plate's corner and the card's node, in artboard units.
const lineEnds = (plate, card, side) => {
  const dir = side === 'right' ? 1 : -1

  return { plateX: plate + dir * 7, nodeX: card - dir * 10 }
}

// Card → orange node → hairline → the corner of its plate. A bead runs the line while open.
const SyntraStageConnector = ({ plate, card, y, side, index, main, lit, active, progress, className }) => {
  const show = useTransform(progress, [0.55, 0.9], [0, 1])
  const t = useMotionValue(0)

  const beadX = useTransform(t, (k) => {
    const ends = lineEnds(plate, card, side)

    return ends.nodeX + (ends.plateX - ends.nodeX) * k
  })

  const beadOpacity = useTransform([t, show], ([k, s]) => s * Math.sin(Math.PI * k))

  const { plateX, nodeX } = lineEnds(plate, card, side)

  useEffect(() => {
    if (!active) return

    const controls = animate(t, [0, 1], {
      duration: STAGE_CARDS.beadSeconds,
      ease: 'easeInOut',
      repeat: Infinity,
      repeatDelay: 1.4,
      delay: 0.5 + index * 0.25
    })

    return () => {
      controls.stop()
      t.set(0)
    }
  }, [active, index, t])

  return (
    <m.svg aria-hidden viewBox={`0 0 ${ARTBOARD} ${ARTBOARD}`} style={{ opacity: show }} className={className}>
      <line x1={plateX} x2={nodeX} y1={y} y2={y} stroke='#B9C4BE' strokeWidth='1' />
      <m.line
        x1={plateX}
        x2={nodeX}
        y1={y}
        y2={y}
        stroke={ORANGE4}
        strokeWidth='1.2'
        initial={false}
        animate={{ opacity: lit ? 0.9 : 0 }}
        transition={{ duration: 0.25 }}
      />
      <circle cx={plateX} cy={y} r='2.6' fill='#8B9A92' />
      <m.circle
        cx={nodeX}
        cy={y}
        r={main ? 10 : 8}
        fill={ORANGE4}
        initial={false}
        animate={{ opacity: lit || main ? 0.22 : 0.12 }}
        transition={{ duration: 0.25 }}
      />
      <circle cx={nodeX} cy={y} r='3.6' fill={ORANGE4} />
      <m.circle cx={beadX} cy={y} r='2.6' fill={ORANGE4} style={{ opacity: beadOpacity }} />
    </m.svg>
  )
}

// Hairlines from each card to its plate, from tablet up. They tilt and float with the stack.
const StageConnectors = ({ progress, active, hovered }) => {
  return (
    <div className='pointer-events-none absolute inset-0 hidden md:block'>
      {STAGES.map((stage, i) => {
        const { edge, width } = STAGE_CARDS[stage.side]
        const cardEdge = stage.side === 'left' ? (edge + width) * 10 : (100 - edge - width) * 10

        return (
          <SyntraStageConnector
            key={stage.title}
            plate={tipX(i, stage.side)}
            card={cardEdge}
            y={cornerY(i)}
            side={stage.side}
            index={i}
            main={isMain(stage)}
            lit={hovered === i}
            active={active}
            progress={progress}
            className='pointer-events-none absolute inset-0 size-full overflow-visible'
          />
        )
      })}
    </div>
  )
}

// ───── stage list (phones) ─────
// On a phone the visual is too narrow for cards beside it, so they sit below it as a grid.
const StageList = ({ reduce }) => {
  return (
    <m.ul
      initial={reduce ? false : { opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.9, delay: 0.5, ease: EASE }}
      aria-label='How the router moves a packet'
      className='m-0 grid list-none grid-cols-1 gap-2.5 p-0 text-[13px] sm:grid-cols-2 md:hidden'
    >
      {STAGES.map((stage, i) => (
        <li key={stage.title} className='n3-stage-card px-[1.05em] py-[0.95em]'>
          <StageCardBody index={i} />
        </li>
      ))}
    </m.ul>
  )
}

// ───── core visual ─────
// Follows the tab being shown or hidden, for useSyncExternalStore
const subscribeToVisibility = (onChange) => {
  document.addEventListener('visibilitychange', onChange)

  return () => document.removeEventListener('visibilitychange', onChange)
}

const isPageVisible = () => !document.hidden

// The server has no tab to hide, so it renders as shown
const isPageVisibleOnServer = () => true

// Phones drop the animated visual: its SMIL beads and floating SVG stack are slow
// on mobile. Tied to the md breakpoint where the stage cards take over from it.
const MOBILE_QUERY = '(max-width: 767px)'

const subscribeToMobile = (onChange) => {
  const mq = window.matchMedia(MOBILE_QUERY)
  mq.addEventListener('change', onChange)

  return () => mq.removeEventListener('change', onChange)
}

const isMobileNow = () => window.matchMedia(MOBILE_QUERY).matches

// The server cannot see the viewport; prerender keeps the visual and CSS hides it on phones
const isMobileOnServer = () => false

// Each layer slides against the pointer by a different amount, which is what reads as depth.
const ParallaxLayer = ({ x, y, reach, children }) => {
  const tx = useTransform(x, (v) => v * reach)
  const ty = useTransform(y, (v) => v * reach)

  return (
    <m.div style={{ x: tx, y: ty }} className='pointer-events-none absolute inset-0'>
      {children}
    </m.div>
  )
}

// The tilt toward the cursor and the slow float, shared by every layer that moves with the stack.
const Tilted = ({ rotateX, rotateY, children }) => {
  return (
    <div className='pointer-events-none absolute inset-0 [perspective:1400px]'>
      <m.div style={{ rotateX, rotateY }} className='absolute inset-0'>
        <div className='n3-float absolute inset-0'>{children}</div>
      </m.div>
    </div>
  )
}

const CoreVisual = () => {
  const [open, setOpen] = useState(false)
  const [hovered, setHovered] = useState(null)

  const pointerType = useRef('mouse')
  const visualRef = useRef(null)

  const reduce = useReducedMotion() ?? false
  const mobile = useSyncExternalStore(subscribeToMobile, isMobileNow, isMobileOnServer)

  // The margin covers the outer orbit and its beads running a little past the visual's box
  const onScreen = useInView(visualRef, { initial: true, margin: '40px' })
  const pageVisible = useSyncExternalStore(subscribeToVisibility, isPageVisible, isPageVisibleOnServer)

  const px = useMotionValue(0)
  const py = useMotionValue(0)
  const x = useSpring(px, { stiffness: 55, damping: 18, mass: 0.9 })
  const y = useSpring(py, { stiffness: 55, damping: 18, mass: 0.9 })
  const rotateY = useTransform(x, (v) => v * MOTION.tiltDegrees)

  // 0 = stacked, 1 = apart. One spring drives the plates, connectors, cards and shadow together.
  const target = useMotionValue(0)
  const progress = useSpring(target, reduce ? { duration: 0.01 } : EXPLODE.spring)

  // While it is open the stack also leans back a little more, which is where the depth comes from.
  const rotateX = useTransform([y, progress], ([v, p]) => v * -MOTION.tiltDegrees + p * 4)
  const restOpacity = useTransform(progress, [0, 0.04], [1, 0])
  const explodedOpacity = useTransform(progress, [0, 0.03], [0, 1])
  const shadowOpacity = useTransform(progress, [0, 1], [1, 0.55])

  // The orbits step back while the stack is open, so the plates and cards have the stage.
  const orbitOpacity = useTransform(progress, [0, 0.6], [1, MOTION.orbitOpenOpacity])

  // The hint under the core bows out as soon as the stack starts to open.
  const hintOpacity = useTransform(progress, [0, 0.25], [1, 0])

  // The beads hold still while the stack is open, off screen or in a hidden tab.
  const beadsPaused = open || !onScreen || !pageVisible

  useEffect(() => {
    target.set(open ? 1 : 0)
  }, [open, target])

  const handlePointerMove = (e) => {
    if (reduce || e.pointerType !== 'mouse') return

    const box = e.currentTarget.getBoundingClientRect()

    px.set(((e.clientX - box.left) / box.width - 0.5) * 2)
    py.set(((e.clientY - box.top) / box.height - 0.5) * 2)
  }

  const handlePointerLeave = () => {
    px.set(0)
    py.set(0)
    setOpen(false)
  }

  // Hovering a card lights its connector and opens the stack, the same as hovering the core.
  const handleCardHover = (index) => {
    setHovered(index)
    if (index !== null && !reduce) setOpen(true)
  }

  const handleCorePointerDown = (e) => {
    pointerType.current = e.pointerType
  }

  const handleCorePointerEnter = (e) => {
    if (e.pointerType === 'mouse' && !reduce) setOpen(true)
  }

  // A mouse opens it by hovering the core. Touch and keyboard have no hover, so they toggle.
  const handleCoreClick = (e) => {
    if (e.detail === 0 || pointerType.current !== 'mouse') setOpen((v) => !v)
  }

  const handleCoreBlur = () => {
    setOpen(false)
  }

  return (
    <LazyMotion features={domAnimation}>
      <>
        {/* Fades and scales in with CSS (n3-enter), from the first paint rather than after
            hydration. Hidden below md and not mounted on phones (see MOBILE_QUERY). */}
        {!mobile && (
          <div
            ref={visualRef}
            onPointerMove={handlePointerMove}
            onPointerLeave={handlePointerLeave}
            className='hidden md:block n3-enter @container relative -my-[6%] -ml-[12.5%] aspect-square w-[125%] max-w-none md:mx-auto md:my-0 md:w-full md:max-w-[840px] lg:mx-0 lg:w-[104%] lg:max-w-none'
          >
            <div className='absolute inset-0'>
              <div aria-hidden className='n3-halo pointer-events-none absolute inset-[6%]' />

              <div className='n3-fade-edge absolute inset-0'>
                <ParallaxLayer x={x} y={y} reach={-MOTION.backdropParallax}>
                  <SyntraDepthBackdrop className='absolute inset-0 size-full overflow-visible' />
                  <SyntraTechnicalBackdrop className='absolute inset-0 size-full overflow-visible' />
                </ParallaxLayer>
              </div>

              {/* The orbits sit outside the faded layer: the outer ring runs past the visual's box. */}
              <ParallaxLayer x={x} y={y} reach={MOTION.ringParallax}>
                <m.div style={{ opacity: orbitOpacity }} className='absolute inset-0'>
                  <SyntraOrbitalSystem className='absolute inset-0 size-full overflow-visible' />
                  <SyntraParticles
                    half='back'
                    animate={!reduce}
                    paused={beadsPaused}
                    className='absolute inset-0 size-full overflow-visible'
                  />
                </m.div>
              </ParallaxLayer>

              <m.div style={{ opacity: shadowOpacity }} className='absolute inset-0'>
                <SyntraCoreShadow className='pointer-events-none absolute inset-0 size-full overflow-visible' />
              </m.div>

              <Tilted rotateX={rotateX} rotateY={rotateY}>
                <m.div style={{ opacity: restOpacity }} className='absolute inset-0'>
                  <SyntraCoreObject className='absolute inset-0 size-full overflow-visible' />
                </m.div>
                <m.div style={{ opacity: explodedOpacity }} className='absolute inset-0'>
                  <SyntraExplodedCore progress={progress} className='absolute inset-0 size-full overflow-visible' />
                </m.div>
                <StageConnectors progress={progress} active={open && !reduce} hovered={hovered} />
              </Tilted>

              <ParallaxLayer x={x} y={y} reach={MOTION.ringParallax}>
                <m.div style={{ opacity: orbitOpacity }} className='absolute inset-0'>
                  <SyntraParticles
                    half='front'
                    animate={!reduce}
                    paused={beadsPaused}
                    className='absolute inset-0 size-full overflow-visible'
                  />
                </m.div>
              </ParallaxLayer>

              {/* The cards ride the same tilt and float in a layer of their own, above the front orbit. */}
              <Tilted rotateX={rotateX} rotateY={rotateY}>
                <StageCards progress={progress} open={open} hovered={hovered} onHover={handleCardHover} />
              </Tilted>
            </div>

            {/* A quiet invitation under the core. Touch screens have no hover, so they are told to tap. */}
            <m.p
              aria-hidden
              style={{ opacity: hintOpacity }}
              className='text-(color:--color-n3-grey) pointer-events-none absolute top-[84%] left-1/2 m-0 flex -translate-x-1/2 items-center gap-1.5 text-[12.5px] whitespace-nowrap'
            >
              <MousePointer2 className='text-(color:--color-n3-accent) size-3.5' strokeWidth={1.8} />
              <span className='[@media(hover:none)]:hidden'>Hover to see the packet path</span>
              <span className='hidden [@media(hover:none)]:inline'>Tap to see the packet path</span>
            </m.p>

            <button
              type='button'
              aria-expanded={open}
              aria-label='Show how the router moves a packet'
              onPointerDown={handleCorePointerDown}
              onPointerEnter={handleCorePointerEnter}
              onClick={handleCoreClick}
              onBlur={handleCoreBlur}
              className='focus-visible:outline-(color:--color-n3-accent) absolute top-[30%] left-[22%] h-[44%] w-[56%] cursor-default rounded-[3rem] focus-visible:outline-2 focus-visible:outline-offset-4'
            />
          </div>
        )}
        <StageList reduce={reduce} />
      </>
    </LazyMotion>
  )
}

// The GitHub mark, since lucide dropped brand icons.
const GithubMark = ({ className }) => {
  return (
    <svg viewBox='0 0 24 24' fill='currentColor' aria-hidden className={className}>
      <path d='M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12' />
    </svg>
  )
}

// ───── hero copy ─────
const ICONS = { down: ArrowDownToLine, up: ArrowUpFromLine, boxes: Boxes }

// The rise is a CSS animation (n3-rise), so the copy plays in from the first paint.
const rise = (delay) => ({ style: { animationDelay: `${delay}s` } })

const HeroCopy = () => {
  return (
    <div className='flex flex-col justify-center'>
      <p
        {...rise(0.1)}
        className='n3-rise text-(color:--color-n3-grey) flex items-center gap-3 text-[13px] font-medium'
      >
        <span aria-hidden className='n3-dot bg-(color:--color-n3-accent) size-2 rounded-full' />
        {COPY.eyebrow}
      </p>

      <h1 id='n3-title' {...rise(0.18)} className='n3-rise n3-h1 text-(color:--color-n3-ink) mt-7 font-normal'>
        {COPY.headline.map((line, i) => (
          <span
            key={line}
            className={
              i === COPY.headline.length - 1
                ? 'block font-extrabold [font-stretch:125%]'
                : 'block'
            }
          >
            {line}
          </span>
        ))}
      </h1>

      <p
        {...rise(0.3)}
        className='n3-rise text-(color:--color-n3-grey) mt-7 max-w-[32rem] text-[17px] leading-[1.6] lg:text-[19px]'
      >
        {COPY.body}
      </p>

      <div {...rise(0.4)} className='n3-rise mt-9 flex flex-wrap items-center gap-3'>
        <a
          href='#download'
          className='n3-btn bg-(color:--color-n3-accent) focus-visible:outline-(color:--color-n3-ink) inline-flex h-13 items-center gap-2.5 rounded-full px-7 text-[16px] font-medium text-white focus-visible:outline-2 focus-visible:outline-offset-2 sm:h-14'
        >
          {COPY.primaryCta}
          <ArrowDownToLine className='n3-btn-icon size-4' strokeWidth={1.8} aria-hidden />
        </a>
        <a
          href='#flash'
          className='n3-btn border-(color:--color-n3-line) text-(color:--color-n3-ink) focus-visible:outline-(color:--color-n3-accent) inline-flex h-13 items-center rounded-full border bg-white/50 px-7 text-[16px] focus-visible:outline-2 focus-visible:outline-offset-2 sm:h-14'
        >
          {COPY.secondaryCta}
        </a>
      </div>

      <dl {...rise(0.58)} className='n3-rise mt-12 grid grid-cols-3 sm:flex sm:items-stretch lg:mt-20'>
        {METRICS.map(({ value, label, icon }, i) => {
          const Icon = ICONS[icon]

          return (
            <div
              key={label}
              className={cn(
                'flex flex-col items-start gap-2 pr-3 sm:flex-row sm:items-center sm:gap-3 sm:pr-8',
                i > 0 && 'border-(color:--color-n3-line) border-l pl-4 sm:pl-8'
              )}
            >
              <Icon className='text-(color:--color-n3-ink) size-8 shrink-0' strokeWidth={1.25} aria-hidden />
              <div>
                <dd className='text-(color:--color-n3-ink) text-[14px] leading-5 font-medium'>{value}</dd>
                <dt className='text-(color:--color-n3-grey) text-[13px] leading-5'>{label}</dt>
              </div>
            </div>
          )
        })}
      </dl>
    </div>
  )
}

// ───── floor grid ─────
// One diamond of the isometric floor: the same 30° axes the core is drawn on.
const CELL = { w: 84, h: 48 }

const SyntraFloorGrid = ({ id, stroke = '#C6CFC9', strokeWidth = 0.8, ...props }) => {
  return (
    <svg aria-hidden width='100%' height='100%' {...props}>
      <defs>
        <pattern id={id} width={CELL.w} height={CELL.h} patternUnits='userSpaceOnUse'>
          <path
            d={`M0 ${CELL.h / 2}L${CELL.w / 2} 0L${CELL.w} ${CELL.h / 2}M0 ${CELL.h / 2}L${CELL.w / 2} ${CELL.h}L${CELL.w} ${CELL.h / 2}`}
            fill='none'
            stroke={stroke}
            strokeWidth={strokeWidth}
          />
        </pattern>
      </defs>
      <rect width='100%' height='100%' fill={`url(#${id})`} />
    </svg>
  )
}

// ───── hero floor ─────
// Diameter of the cursor spotlight, in px.
const SPOT = 340

// The floor grid, plus an amber copy seen through a round window that follows the cursor.
const HeroFloor = () => {
  const ref = useRef(null)
  const spotRef = useRef(null)
  const litRef = useRef(null)

  useEffect(() => {
    const el = ref.current
    const spot = spotRef.current
    const lit = litRef.current
    const section = el?.parentElement

    if (!el || !spot || !lit || !section) return

    // The amber grid matches the section's size, so its pattern lines up with the grey one.
    const resize = new ResizeObserver(([entry]) => {
      lit.style.width = `${entry.contentRect.width}px`
      lit.style.height = `${entry.contentRect.height}px`
    })

    resize.observe(section)

    let frame = 0

    const handlePointerMove = (e) => {
      if (e.pointerType !== 'mouse') return

      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        const box = section.getBoundingClientRect()
        const x = e.clientX - box.left - SPOT / 2
        const y = e.clientY - box.top - SPOT / 2

        spot.style.transform = `translate3d(${x}px, ${y}px, 0)`
        lit.style.transform = `translate3d(${-x}px, ${-y}px, 0)`
        el.dataset.spot = 'on'
      })
    }

    const handlePointerLeave = () => {
      cancelAnimationFrame(frame)
      el.dataset.spot = 'off'
    }

    section.addEventListener('pointermove', handlePointerMove)
    section.addEventListener('pointerleave', handlePointerLeave)

    return () => {
      cancelAnimationFrame(frame)
      resize.disconnect()
      section.removeEventListener('pointermove', handlePointerMove)
      section.removeEventListener('pointerleave', handlePointerLeave)
    }
  }, [])

  return (
    <div ref={ref} aria-hidden data-spot='off' className='pointer-events-none absolute inset-0 -z-10 overflow-hidden'>
      <SyntraFloorGrid id='n3-floor-cell' className='n3-floor absolute inset-0' />
      <div ref={spotRef} style={{ width: SPOT, height: SPOT }} className='n3-floor-spot'>
        <div ref={litRef} className='n3-floor-lit'>
          <SyntraFloorGrid id='n3-floor-cell-lit' stroke='#16211E' strokeWidth={1} className='block size-full' />
        </div>
      </div>
    </div>
  )
}

// ───── hero ─────
const Hero = () => {
  return (
    <section id='top' aria-labelledby='n3-title' className='n3-hero relative isolate overflow-x-clip'>
      <HeroFloor />
      <div className='n3-frame mx-auto grid max-w-360 items-center gap-6 pt-6 pb-12 lg:min-h-[inherit] lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] lg:gap-12 lg:py-10 xl:grid-cols-[minmax(0,0.92fr)_minmax(0,1.08fr)] xl:gap-20'>
        <HeroCopy />
        <CoreVisual />
      </div>
    </section>
  )
}

// ───── navbar ─────
const Navbar = () => {
  const [scrolled, setScrolled] = useState(false)
  const [open, setOpen] = useState(false)

  const wrapRef = useRef(null)

  const reduce = useReducedMotion()

  useEffect(() => {
    const handleScroll = () => setScrolled(window.scrollY > 12)

    handleScroll()
    window.addEventListener('scroll', handleScroll, { passive: true })

    return () => window.removeEventListener('scroll', handleScroll)
  }, [])

  // The hero fills what the nav leaves, so the nav's real height is measured and re-measured
  // whenever it wraps, resizes or the web font lands.
  useEffect(() => {
    const el = wrapRef.current
    const root = el?.closest('.n3-root')

    if (!el || !root) return

    const measure = () => root.style.setProperty('--n3-nav-space', `${el.getBoundingClientRect().height}px`)
    const observer = new ResizeObserver(measure)

    measure()
    observer.observe(el)
    document.fonts?.ready.then(measure)

    return () => observer.disconnect()
  }, [])

  const handleMenuToggle = () => {
    setOpen((v) => !v)
  }

  const handleMenuClose = () => {
    setOpen(false)
  }

  return (
    <LazyMotion features={domAnimation}>
      <header ref={wrapRef} className='n3-drop sticky top-0 z-50'>
        <div className={cn('n3-nav-bar', (scrolled || open) && 'n3-nav-bar-scrolled')}>
          <nav
            aria-label='Primary'
            className='n3-frame mx-auto flex h-18 max-w-360 items-center justify-between lg:h-24'
          >
            <a href='#top' aria-label='Native seL4 IPv4 router home' className='text-(color:--color-n3-ink) flex items-center gap-2.5'>
              <Router className='size-7 shrink-0' strokeWidth={1.8} aria-hidden />
              <span className='text-[13px] leading-[1.25] font-semibold tracking-[-0.01em] sm:text-[15px] lg:text-[16px]'>
                seL4 router on NanoPi R5S
              </span>
            </a>

            <ul className='hidden items-center gap-8 lg:flex'>
              {NAV_LINKS.map(({ label, href }) => (
                <li key={label}>
                  <a
                    href={href}
                    className='text-(color:--color-n3-ink) hover:opacity-60 focus-visible:opacity-60 inline-flex items-center py-2 text-[16px] transition-opacity duration-200 focus-visible:outline-none'
                  >
                    {label}
                  </a>
                </li>
              ))}
            </ul>

            <div className='flex items-center gap-2 sm:gap-4'>
              <a
                href='https://github.com/cryptofuture/nanopi_r5s_sel4'
                target='_blank'
                rel='noopener noreferrer'
                aria-label='GitHub repository'
                className='n3-btn text-(color:--color-n3-ink) hover:opacity-60 focus-visible:opacity-60 grid size-11 place-items-center rounded-full transition-opacity duration-200 focus-visible:outline-none'
              >
                <GithubMark className='n3-btn-icon size-5' />
              </a>
              <a
                href='#download'
                className='n3-btn bg-(color:--color-n3-ink) focus-visible:outline-(color:--color-n3-accent) hidden h-11 items-center rounded-full px-5 text-[15px] font-medium text-white focus-visible:outline-2 focus-visible:outline-offset-2 sm:inline-flex sm:h-13 sm:px-6 sm:text-[16px]'
              >
                Download
              </a>
              <button
                type='button'
                aria-label={open ? 'Close menu' : 'Open menu'}
                aria-expanded={open}
                aria-controls='n3-mobile-menu'
                onClick={handleMenuToggle}
                className='text-(color:--color-n3-ink) hover:bg-black/5 grid size-11 place-items-center rounded-full transition-colors duration-200 lg:hidden'
              >
                {open ? <X className='size-5' strokeWidth={1.6} /> : <Menu className='size-5' strokeWidth={1.6} />}
              </button>
            </div>
          </nav>

          <AnimatePresence initial={false}>
            {open && (
              <m.div
                id='n3-mobile-menu'
                initial={reduce ? false : { opacity: 0, y: -6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduce ? { opacity: 0 } : { opacity: 0, y: -6 }}
                transition={{ duration: 0.22, ease: EASE }}
                className='n3-frame pb-5 lg:hidden'
              >
                <ul className='border-(color:--color-n3-ink)/8 flex flex-col border-t pt-2'>
                  {NAV_LINKS.map(({ label, href }) => (
                    <li key={label}>
                      <a
                        href={href}
                        onClick={handleMenuClose}
                        className='text-(color:--color-n3-ink) flex h-12 items-center text-[16px]'
                      >
                        {label}
                      </a>
                    </li>
                  ))}
                </ul>
              </m.div>
            )}
          </AnimatePresence>
        </div>
      </header>
    </LazyMotion>
  )
}

// ───── root ─────
const motionVars = {
  '--n3-float': MOTION.floatDistance,
  '--n3-float-seconds': `${MOTION.floatSeconds}s`
}

const CoreHero = () => {
  return (
    <>
      <style>{BLOCK_CSS}</style>
      <div
        style={motionVars}
        className='n3-root n3-bg font-(family-name:--font-n3-sans) text-(color:--color-n3-ink) w-full antialiased'
      >
        <Navbar />
        <Hero />
      </div>
    </>
  )
}

export default CoreHero
