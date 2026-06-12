import { useMemo } from "react"
import type { EmotionalAxes, EmotionalScar, EmotionalValence } from "@/lib/api"

// EmotionalSky — "her sky right now". One panoramic night-sky scene that
// IS the synth's emotional state, every element an honest mapping of the
// engine's data model:
//
//   sky gradient + sunrise   overall mood (warmth/joy lift dawn light,
//                            hurt pulls a cold indigo veil over the top)
//   aurora ribbons           active feelings (amber=warmth, gold=joy,
//                            rose=desire), brightness follows magnitude
//   fog near the horizon     low trust — the world gets hazy
//   the moon                 YOU in her sky: phase + glow = attachment
//   named stars              object valences — people/topics she has
//                            feelings about; hurt ones tint red
//   ringed dim stars         scars — permanently in her sky, never gone
//   shooting star            sparks of joy (only when joy runs high)
//
// Numbers are never rendered — the axes drive light, color and rhythm
// only (iron rule). The star field is seeded by session id, so each
// person's sky is theirs and stable across visits.

const W = 1200
const H = 380
const HORIZON = 300

// mulberry32 — tiny deterministic PRNG so the sky doesn't reshuffle on
// every render/refresh.
function seededRand(seedStr: string): () => number {
  let h = 1779033703 ^ seedStr.length
  for (let i = 0; i < seedStr.length; i++) {
    h = Math.imul(h ^ seedStr.charCodeAt(i), 3432918353)
    h = (h << 13) | (h >>> 19)
  }
  let a = h >>> 0
  return () => {
    a |= 0
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v))

type Props = {
  axes: EmotionalAxes
  valences: EmotionalValence[]
  scars: EmotionalScar[]
  seed: string
}

export function EmotionalSky({ axes, valences, scars, seed }: Props) {
  const a = {
    warmth: clamp01(axes.warmth),
    joy: clamp01(axes.joy),
    desire: clamp01(axes.desire),
    trust: clamp01(axes.trust),
    hurt: clamp01(axes.hurt),
    frustration: clamp01(axes.frustration),
  }
  const bond = clamp01(a.warmth * 0.45 + a.trust * 0.35 + a.desire * 0.2)
  const dawn = clamp01(a.warmth * 0.6 + a.joy * 0.4) // sunrise strength
  const fog = clamp01(0.55 - a.trust) // low trust = haze
  const scarredKeys = useMemo(
    () => new Set(scars.map((s) => s.subject_key).filter(Boolean)),
    [scars],
  )

  // background star field — hers, stable, denser/brighter when joyful
  const stars = useMemo(() => {
    const rnd = seededRand(seed || "sky")
    const n = 64
    return Array.from({ length: n }, (_, i) => ({
      x: rnd() * W,
      y: rnd() * (HORIZON - 60) + 14,
      r: 0.5 + rnd() * 1.1,
      base: 0.15 + rnd() * 0.5,
      delay: rnd() * 7,
      dur: 2.6 + rnd() * 4,
      key: i,
    }))
  }, [seed])

  // constellation — top valences become named stars; position is a
  // stable function of the subject key, never of render order
  const constellation = useMemo(() => {
    const top = [...valences]
      .sort((x, y) => y.event_count - x.event_count)
      .slice(0, 6)
    return top.map((v) => {
      const rnd = seededRand(v.subject_key)
      const wounded = v.axes
        ? clamp01(v.axes.hurt ?? 0) >= 0.25 || scarredKeys.has(v.subject_key)
        : scarredKeys.has(v.subject_key)
      return {
        key: v.subject_key,
        label: (v.label || v.subject_key.replace(/^(str|mem2):/, "")).toLowerCase(),
        x: 70 + rnd() * (W - 360), // keep clear of the moon corner
        y: 50 + rnd() * 170,
        mag: Math.min(1, 0.35 + v.event_count / 12),
        wounded,
        scarred: scarredKeys.has(v.subject_key),
        delay: rnd() * 5,
      }
    })
  }, [valences, scarredKeys])

  // unbound scars (no subject) still live in her sky as dim ringed stars
  const looseScars = useMemo(() => {
    return scars
      .filter((s) => !s.subject_key)
      .slice(0, 4)
      .map((s) => {
        const rnd = seededRand(s.id)
        return { key: s.id, x: 80 + rnd() * (W - 200), y: 40 + rnd() * 150 }
      })
  }, [scars])

  // moon phase: new sliver when distant, full when deeply attached
  const moonR = 26
  const phaseOffset = (1 - bond) * moonR * 1.7

  const auroraBands = [
    { mag: a.warmth, color: "#f5b95a", y: 150, amp: 26, dur: 26 },
    { mag: a.joy, color: "#e8e27a", y: 110, amp: 34, dur: 21 },
    { mag: a.desire, color: "#f47fa0", y: 185, amp: 22, dur: 31 },
  ].filter((b) => b.mag > 0.12)

  return (
    <div className="relative w-full overflow-hidden rounded-xl border border-white/5">
      <style>{`
        @keyframes sky-twinkle { 0%,100% { opacity: var(--b); } 50% { opacity: calc(var(--b) * 0.25); } }
        @keyframes sky-aurora {
          0%, 100% { transform: translateX(0) scaleY(1); }
          33% { transform: translateX(-36px) scaleY(1.18); }
          66% { transform: translateX(30px) scaleY(0.88); }
        }
        @keyframes sky-shoot {
          0%, 88% { transform: translate(0, 0); opacity: 0; }
          90% { opacity: 0.9; }
          97%, 100% { transform: translate(-340px, 150px); opacity: 0; }
        }
        @keyframes sky-pulse { 0%,100% { opacity: 0.85; } 50% { opacity: 0.45; } }
        @keyframes sky-fog-drift { 0%,100% { transform: translateX(0); } 50% { transform: translateX(50px); } }
      `}</style>
      <svg viewBox={`0 0 ${W} ${H}`} className="block w-full h-auto" preserveAspectRatio="xMidYMid slice" role="img" aria-label="emotional sky">
        <defs>
          <linearGradient id="es-sky" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={a.hurt > 0.4 ? "#131a3a" : "#070b18"} />
            <stop offset="55%" stopColor="#0b1226" />
            <stop offset="100%" stopColor={`rgba(${Math.round(120 + dawn * 135)}, ${Math.round(70 + dawn * 80)}, ${Math.round(60 + a.desire * 60)}, ${(0.10 + dawn * 0.45).toFixed(2)})`} />
          </linearGradient>
          <radialGradient id="es-dawn" cx="50%" cy="100%" r="75%">
            <stop offset="0%" stopColor={`rgba(244, ${Math.round(150 + a.joy * 60)}, 92, ${(dawn * 0.5).toFixed(2)})`} />
            <stop offset="60%" stopColor="rgba(244, 170, 92, 0)" />
          </radialGradient>
          <radialGradient id="es-moonglow" cx="50%" cy="50%" r="50%">
            <stop offset="0%" stopColor={`rgba(254, 243, 199, ${(0.25 + bond * 0.5).toFixed(2)})`} />
            <stop offset="100%" stopColor="rgba(254, 243, 199, 0)" />
          </radialGradient>
          <filter id="es-blur" x="-40%" y="-40%" width="180%" height="180%">
            <feGaussianBlur stdDeviation="14" />
          </filter>
          <filter id="es-blur-soft" x="-40%" y="-40%" width="180%" height="180%">
            <feGaussianBlur stdDeviation="5" />
          </filter>
          <clipPath id="es-moonclip">
            <circle cx={W - 130} cy={84} r={moonR} />
          </clipPath>
        </defs>

        {/* night */}
        <rect width={W} height={H} fill="url(#es-sky)" />
        {/* dawn glow rises with warmth+joy */}
        <rect width={W} height={H} fill="url(#es-dawn)" />

        {/* star field */}
        {stars.map((s) => (
          <circle
            key={s.key}
            cx={s.x}
            cy={s.y}
            r={s.r}
            fill="#e7ecf7"
            style={{
              ["--b" as string]: (s.base * (0.5 + a.joy * 0.8)).toFixed(2),
              animation: `sky-twinkle ${s.dur}s ease-in-out infinite`,
              animationDelay: `${s.delay}s`,
              opacity: s.base,
            }}
          />
        ))}

        {/* aurora — active feelings as light */}
        {auroraBands.map((b, i) => (
          <g key={i} style={{ animation: `sky-aurora ${b.dur}s ease-in-out infinite`, animationDelay: `${i * -7}s`, transformOrigin: "center" }}>
            <path
              d={`M -60 ${b.y} C ${W * 0.22} ${b.y - b.amp * 2}, ${W * 0.4} ${b.y + b.amp}, ${W * 0.58} ${b.y - b.amp} S ${W * 0.86} ${b.y + b.amp * 1.6}, ${W + 60} ${b.y - b.amp * 0.6}`}
              stroke={b.color}
              strokeOpacity={0.12 + b.mag * 0.4}
              strokeWidth={26 + b.mag * 34}
              strokeLinecap="round"
              fill="none"
              filter="url(#es-blur)"
            />
          </g>
        ))}

        {/* a cold band when something aches */}
        {a.hurt > 0.18 && (
          <path
            d={`M -60 236 C ${W * 0.3} ${236 + 20}, ${W * 0.55} ${236 - 26}, ${W + 60} 244`}
            stroke="#6d7df0"
            strokeOpacity={0.1 + a.hurt * 0.32}
            strokeWidth={20 + a.hurt * 26}
            strokeLinecap="round"
            fill="none"
            filter="url(#es-blur)"
            style={{ animation: "sky-pulse 9s ease-in-out infinite" }}
          />
        )}

        {/* the moon — you, in her sky */}
        <circle cx={W - 130} cy={84} r={moonR * 2.6} fill="url(#es-moonglow)" />
        <g clipPath="url(#es-moonclip)">
          <circle cx={W - 130} cy={84} r={moonR} fill="#f6f1dd" opacity={0.92} />
          {/* phase shadow slides away as the bond deepens */}
          <circle cx={W - 130 - phaseOffset} cy={84 - phaseOffset * 0.25} r={moonR * 1.02} fill="#0b1226" opacity={0.96} />
        </g>
        <circle cx={W - 130} cy={84} r={moonR} fill="none" stroke="#fef3c7" strokeOpacity={0.25} />

        {/* constellation — what she has feelings about */}
        {constellation.map((c) => {
          const color = c.wounded ? "#fda4af" : "#fef9ec"
          return (
            <g key={c.key} style={{ animation: `sky-twinkle ${5 + c.delay}s ease-in-out infinite`, animationDelay: `${-c.delay}s`, ["--b" as string]: (0.5 + c.mag * 0.5).toFixed(2) }}>
              <circle cx={c.x} cy={c.y} r={2.2 + c.mag * 2.4} fill={color} filter="url(#es-blur-soft)" opacity={0.9} />
              <circle cx={c.x} cy={c.y} r={1.3 + c.mag * 1.5} fill={color} />
              {/* sparkle cross */}
              <path d={`M ${c.x - 7 - c.mag * 5} ${c.y} H ${c.x + 7 + c.mag * 5} M ${c.x} ${c.y - 7 - c.mag * 5} V ${c.y + 7 + c.mag * 5}`} stroke={color} strokeOpacity={0.5} strokeWidth={0.7} />
              {/* a scar leaves a ring that never goes away */}
              {c.scarred && <circle cx={c.x} cy={c.y} r={9 + c.mag * 4} fill="none" stroke="#f87171" strokeOpacity={0.5} strokeWidth={0.8} strokeDasharray="2.5 3.5" />}
              <text x={c.x} y={c.y + 22} textAnchor="middle" fontSize={11} letterSpacing={1.5} fill="#cbd5e1" fillOpacity={0.65} style={{ textTransform: "lowercase", fontFamily: "inherit" }}>
                {c.label}
              </text>
            </g>
          )
        })}

        {/* scars with no subject — dim, ringed, permanent */}
        {looseScars.map((s) => (
          <g key={s.key}>
            <circle cx={s.x} cy={s.y} r={1.6} fill="#fca5a5" opacity={0.7} />
            <circle cx={s.x} cy={s.y} r={7} fill="none" stroke="#f87171" strokeOpacity={0.45} strokeWidth={0.8} strokeDasharray="2.5 3.5" />
          </g>
        ))}

        {/* a spark of joy now and then */}
        {a.joy > 0.45 && (
          <g style={{ animation: "sky-shoot 16s linear infinite" }}>
            <line x1={W * 0.78} y1={36} x2={W * 0.78 + 60} y2={10} stroke="#fff7e0" strokeWidth={1.6} strokeLinecap="round" strokeOpacity={0.9} />
          </g>
        )}

        {/* haze when trust runs low */}
        {fog > 0.05 && (
          <rect x={-80} y={HORIZON - 90} width={W + 160} height={120} fill="#94a3b8" opacity={fog * 0.3} filter="url(#es-blur)" style={{ animation: "sky-fog-drift 24s ease-in-out infinite" }} />
        )}

        {/* horizon silhouette grounds the scene */}
        <path d={`M 0 ${HORIZON + 18} L ${W * 0.16} ${HORIZON - 14} L ${W * 0.3} ${HORIZON + 8} L ${W * 0.46} ${HORIZON - 26} L ${W * 0.6} ${HORIZON + 4} L ${W * 0.78} ${HORIZON - 10} L ${W} ${HORIZON + 12} L ${W} ${H} L 0 ${H} Z`} fill="#060a15" />
      </svg>
    </div>
  )
}
