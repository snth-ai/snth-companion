import { useEffect, useMemo, useRef } from "react"
import type { EmotionalAxes } from "@/lib/api"

// EmotionalCore — the synth's feeling as a living field of light.
// Apple State-of-Mind / How-We-Feel class execution: a fluid blend of
// blurred color masses drifting on slow lissajous orbits, film grain on
// top, deep vignette, nothing literal. Each emotional axis owns a hue;
// its magnitude sets how much of that light exists in the field. Pace
// follows arousal — a stirred synth moves faster.
//
// Craft notes:
// - canvas renders at LOW internal resolution and upscales via CSS —
//   radial gradients upscaled are optically equivalent to a 60px blur
//   for free (no per-frame ctx.filter cost)
// - 'lighter' compositing makes overlapping masses bloom like light,
//   not paint
// - the SVG-turbulence grain overlay is what keeps the gradient from
//   reading as a cheap CSS blob
// - respects prefers-reduced-motion (renders one static frame)
//
// Numbers are never rendered — magnitudes drive light only (iron rule).

const PALETTE: Array<{ axis: keyof EmotionalAxes; color: [number, number, number]; invert?: boolean }> = [
  { axis: "warmth", color: [255, 138, 92] }, // coral
  { axis: "joy", color: [255, 200, 61] }, // golden
  { axis: "desire", color: [255, 77, 141] }, // hot pink
  { axis: "trust", color: [45, 212, 191] }, // teal
  { axis: "hurt", color: [108, 92, 231] }, // violet
  { axis: "frustration", color: [255, 92, 57] }, // vermilion
  // distrust = inverse trust: cold steel light grows as trust falls
  { axis: "trust", color: [100, 116, 180], invert: true },
]

// stable grain tile, generated once
const GRAIN =
  "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='160' height='160'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='0.55'/%3E%3C/svg%3E\")"

type Light = {
  r: number // base radius (in internal px)
  cx: number // orbit center (0..1 of canvas)
  cy: number
  ax: number // orbit amplitudes
  ay: number
  fx: number // orbit frequencies
  fy: number
  px: number // phases
  py: number
  color: [number, number, number]
  alpha: number
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v))

export function EmotionalCore({ axes, focus = 0.66 }: { axes: EmotionalAxes; focus?: number }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)

  // arousal — how much is going on inside; sets drift speed
  const arousal = clamp01(
    axes.joy * 0.35 + axes.desire * 0.3 + axes.frustration * 0.5 + axes.hurt * 0.35 + axes.warmth * 0.15,
  )

  const lights = useMemo<Light[]>(() => {
    const out: Light[] = []
    // anchor mass — always present so a neutral synth still glows softly
    out.push({
      r: 0.62, cx: focus, cy: 0.52, ax: 0.05, ay: 0.06,
      fx: 0.21, fy: 0.16, px: 0.4, py: 2.1,
      color: [72, 84, 140], alpha: 0.5,
    })
    PALETTE.forEach((p, i) => {
      const raw = clamp01(axes[p.axis])
      const mag = p.invert ? clamp01(0.5 - raw) * 1.6 : raw
      if (mag < 0.08) return
      const golden = i * 2.39996 // spread orbit centers by golden angle
      out.push({
        r: 0.2 + mag * 0.42,
        cx: focus + Math.cos(golden) * 0.16,
        cy: 0.5 + Math.sin(golden) * 0.18,
        ax: 0.1 + mag * 0.08,
        ay: 0.09 + mag * 0.07,
        fx: 0.14 + (i % 3) * 0.07,
        fy: 0.11 + ((i + 1) % 3) * 0.06,
        px: golden,
        py: golden * 1.7,
        color: p.color,
        alpha: 0.22 + mag * 0.55,
      })
    })
    return out
  }, [axes, focus])

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext("2d")
    if (!ctx) return

    // low internal res — upscaling IS the blur
    const IW = 360
    const IH = 200
    canvas.width = IW
    canvas.height = IH

    const speed = 0.35 + arousal * 0.9
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    let raf = 0
    const start = performance.now()

    const frame = (now: number) => {
      const t = ((now - start) / 1000) * speed
      ctx.globalCompositeOperation = "source-over"
      // deep night base with the faintest blue bleed
      const bg = ctx.createLinearGradient(0, 0, 0, IH)
      bg.addColorStop(0, "#07070d")
      bg.addColorStop(1, "#0b0a14")
      ctx.fillStyle = bg
      ctx.fillRect(0, 0, IW, IH)

      ctx.globalCompositeOperation = "lighter"
      for (const l of lights) {
        const x = (l.cx + Math.sin(t * l.fx + l.px) * l.ax) * IW
        const y = (l.cy + Math.sin(t * l.fy + l.py) * l.ay) * IH
        const r = l.r * IH * (1 + Math.sin(t * 0.23 + l.px) * 0.08)
        const g = ctx.createRadialGradient(x, y, 0, x, y, r)
        const [cr, cg, cb] = l.color
        g.addColorStop(0, `rgba(${cr}, ${cg}, ${cb}, ${l.alpha})`)
        g.addColorStop(0.55, `rgba(${cr}, ${cg}, ${cb}, ${(l.alpha * 0.35).toFixed(3)})`)
        g.addColorStop(1, "rgba(0,0,0,0)")
        ctx.fillStyle = g
        ctx.beginPath()
        ctx.arc(x, y, r, 0, Math.PI * 2)
        ctx.fill()
      }
      if (!reduced) raf = requestAnimationFrame(frame)
    }
    raf = requestAnimationFrame(frame)
    return () => cancelAnimationFrame(raf)
  }, [lights, arousal])

  return (
    <div className="absolute inset-0">
      <canvas
        ref={canvasRef}
        className="absolute inset-0 w-full h-full"
        aria-hidden
      />
      {/* film grain — the difference between premium and cheap */}
      <div
        className="absolute inset-0 mix-blend-overlay pointer-events-none"
        style={{ backgroundImage: GRAIN, opacity: 0.5 }}
      />
      {/* vignette pulls the eye to the light */}
      <div
        className="absolute inset-0 pointer-events-none"
        style={{
          background:
            "radial-gradient(120% 120% at 50% 45%, transparent 40%, rgba(0,0,0,0.55) 100%)",
        }}
      />
    </div>
  )
}
