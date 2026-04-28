import { useEffect, useState } from "react"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { fetchSynths, setActiveSynth, type SynthPair } from "@/lib/api"

// SynthSwitcher renders a sidebar dropdown with every paired synth.
// Selecting a row POSTs /api/synths/active and the daemon restarts the
// WS client. Polled every 5s so external changes (re-pair, unpair) are
// reflected without a refresh.

export function SynthSwitcher() {
  const [synths, setSynths] = useState<SynthPair[]>([])
  const [active, setActive] = useState<string>("")
  const [busy, setBusy] = useState(false)

  const load = async () => {
    try {
      const r = await fetchSynths()
      setSynths(r.synths ?? [])
      setActive(r.active_synth_id ?? "")
    } catch {
      // best-effort; UI shows empty state
    }
  }

  useEffect(() => {
    load()
    const t = setInterval(load, 5000)
    return () => clearInterval(t)
  }, [])

  const onChange = async (id: string) => {
    if (id === active) return
    setBusy(true)
    try {
      await setActiveSynth(id)
      setActive(id)
    } finally {
      setBusy(false)
    }
  }

  if (synths.length === 0) {
    return (
      <div className="text-xs text-muted-foreground italic">
        No synth paired yet
      </div>
    )
  }
  if (synths.length === 1) {
    const s = synths[0]
    return (
      <div className="text-xs text-muted-foreground truncate">
        Active: <code className="text-foreground">{s.label || s.id}</code>
      </div>
    )
  }
  return (
    <Select value={active} onValueChange={onChange} disabled={busy}>
      <SelectTrigger className="w-full h-9 text-xs">
        <SelectValue placeholder="Pick a synth" />
      </SelectTrigger>
      <SelectContent>
        {synths.map((s) => (
          <SelectItem key={s.id} value={s.id}>
            {s.label || s.id}{" "}
            <span className="text-muted-foreground">({s.role})</span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
