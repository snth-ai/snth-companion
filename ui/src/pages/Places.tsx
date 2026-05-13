import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Trash2, Pencil, MapPin, Plus } from "lucide-react"
import {
  fetchLandmarks,
  createLandmark,
  updateLandmark,
  deleteLandmark,
  type Landmark,
  type LandmarkInput,
  type LandmarkTag,
} from "@/lib/api"

// Places: define named geofences (home, gym, work, custom) that the
// paired iPhone monitors via CLCircularRegion. iOS gets these via WS
// push from hub on every CRUD and registers the closest 20 as regions.
// On entry/exit iOS sends ONLY the landmark name back to hub — never
// raw coordinates. See snth-mobile/docs/SPEC.md §13.
//
// MVP: form-based input (lat/lng manually). Wave 2 will add a Leaflet
// map widget for click-to-place. Until then the user copies coords
// from Apple Maps (right-click → Copy "lat, lng") or Google Maps.

const TAGS: { value: LandmarkTag; label: string }[] = [
  { value: "home", label: "Home" },
  { value: "gym", label: "Gym" },
  { value: "work", label: "Work" },
  { value: "store", label: "Store" },
  { value: "custom", label: "Custom" },
]

type DialogState = {
  open: boolean
  editing: Landmark | null
}

const emptyInput: LandmarkInput = {
  name: "",
  tag: "home",
  lat: 0,
  lng: 0,
  radius_m: 100,
}

export function PlacesPage() {
  const [landmarks, setLandmarks] = useState<Landmark[]>([])
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [dialog, setDialog] = useState<DialogState>({
    open: false,
    editing: null,
  })
  const [draft, setDraft] = useState<LandmarkInput>(emptyInput)
  const [pasteRaw, setPasteRaw] = useState("")

  const reload = async () => {
    setErr(null)
    try {
      const list = await fetchLandmarks()
      setLandmarks(list)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  useEffect(() => {
    void reload()
  }, [])

  const openCreate = () => {
    setDraft(emptyInput)
    setPasteRaw("")
    setDialog({ open: true, editing: null })
  }

  const openEdit = (lm: Landmark) => {
    setDraft({
      name: lm.name,
      tag: lm.tag,
      lat: lm.lat,
      lng: lm.lng,
      radius_m: lm.radius_m,
    })
    setPasteRaw(`${lm.lat}, ${lm.lng}`)
    setDialog({ open: true, editing: lm })
  }

  // Parse "37.7749, -122.4194" or "37.7749,-122.4194" (Apple Maps /
  // Google Maps copy format). Anything else falls back to manual entry.
  const applyPaste = (raw: string) => {
    setPasteRaw(raw)
    const m = raw.match(/^\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*$/)
    if (m) {
      setDraft((d) => ({ ...d, lat: parseFloat(m[1]), lng: parseFloat(m[2]) }))
    }
  }

  const submit = async () => {
    setBusy(true)
    setErr(null)
    try {
      const trimmedName = draft.name.trim()
      if (!trimmedName) throw new Error("Name is required")
      if (!Number.isFinite(draft.lat) || !Number.isFinite(draft.lng)) {
        throw new Error("Coordinates must be numeric")
      }
      if (Math.abs(draft.lat) > 90 || Math.abs(draft.lng) > 180) {
        throw new Error("Coordinates out of range")
      }
      const payload: LandmarkInput = {
        ...draft,
        name: trimmedName,
        radius_m: Math.max(50, Math.min(500, Math.round(draft.radius_m))),
      }
      if (dialog.editing) {
        await updateLandmark(dialog.editing.id, payload)
      } else {
        await createLandmark(payload)
      }
      setDialog({ open: false, editing: null })
      await reload()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const remove = async (lm: Landmark) => {
    if (!confirm(`Delete landmark "${lm.name}"? iPhone will stop monitoring.`)) {
      return
    }
    setBusy(true)
    setErr(null)
    try {
      await deleteLandmark(lm.id)
      await reload()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight flex items-center gap-2">
            <MapPin className="size-6" />
            Places
          </h1>
          <p className="text-sm text-muted-foreground mt-1">
            Named geofences the paired iPhone monitors. Synth sees only the
            landmark name on entry/exit — never raw coordinates.
          </p>
        </div>
        <Button onClick={openCreate}>
          <Plus className="size-4 mr-1.5" />
          Add landmark
        </Button>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Landmarks ({landmarks.length})</CardTitle>
          <CardDescription>
            iOS limit is 20 simultaneous regions; if you have more, the iPhone
            keeps the 20 closest to its current location.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {landmarks.length === 0 ? (
            <div className="text-sm text-muted-foreground py-8 text-center">
              No landmarks yet. Click <strong>Add landmark</strong> to define your
              first one.
            </div>
          ) : (
            <div className="divide-y divide-border">
              {landmarks.map((lm) => (
                <div
                  key={lm.id}
                  className="flex items-center gap-4 py-3 first:pt-0 last:pb-0"
                >
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-medium">{lm.name}</span>
                      <Badge variant="secondary">{lm.tag}</Badge>
                    </div>
                    <div className="text-xs text-muted-foreground mt-0.5 font-mono">
                      {lm.lat.toFixed(5)}, {lm.lng.toFixed(5)} · {lm.radius_m}m
                      radius
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => openEdit(lm)}
                    disabled={busy}
                  >
                    <Pencil className="size-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void remove(lm)}
                    disabled={busy}
                  >
                    <Trash2 className="size-4 text-destructive" />
                  </Button>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">How to get coordinates</CardTitle>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground space-y-2">
          <p>
            <strong>Apple Maps:</strong> right-click a location → "Copy Address
            of Pointed Location" gives <code>lat, lng</code>.
          </p>
          <p>
            <strong>Google Maps:</strong> right-click → click the coordinates at
            the top to copy.
          </p>
          <p>
            Paste either into the <strong>Coordinates</strong> field below; lat
            + lng auto-fill. Map widget coming in a later wave.
          </p>
        </CardContent>
      </Card>

      <Dialog
        open={dialog.open}
        onOpenChange={(o) => setDialog({ ...dialog, open: o })}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>
              {dialog.editing ? `Edit "${dialog.editing.name}"` : "Add landmark"}
            </DialogTitle>
            <DialogDescription>
              All paired iPhones will sync this within ~30s.
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4 py-2">
            <div className="space-y-1.5">
              <Label htmlFor="lm-name">Name</Label>
              <Input
                id="lm-name"
                value={draft.name}
                onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                placeholder="Home"
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="lm-tag">Tag</Label>
              <Select
                value={draft.tag}
                onValueChange={(v) =>
                  setDraft({ ...draft, tag: v as LandmarkTag })
                }
              >
                <SelectTrigger id="lm-tag">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {TAGS.map((t) => (
                    <SelectItem key={t.value} value={t.value}>
                      {t.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="lm-paste">Coordinates (paste "lat, lng")</Label>
              <Input
                id="lm-paste"
                value={pasteRaw}
                onChange={(e) => applyPaste(e.target.value)}
                placeholder="37.7749, -122.4194"
                className="font-mono"
              />
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label htmlFor="lm-lat">Latitude</Label>
                <Input
                  id="lm-lat"
                  type="number"
                  step="0.000001"
                  value={draft.lat}
                  onChange={(e) =>
                    setDraft({ ...draft, lat: parseFloat(e.target.value) || 0 })
                  }
                  className="font-mono"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="lm-lng">Longitude</Label>
                <Input
                  id="lm-lng"
                  type="number"
                  step="0.000001"
                  value={draft.lng}
                  onChange={(e) =>
                    setDraft({ ...draft, lng: parseFloat(e.target.value) || 0 })
                  }
                  className="font-mono"
                />
              </div>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="lm-radius">Radius (metres, 50-500)</Label>
              <Input
                id="lm-radius"
                type="number"
                min={50}
                max={500}
                step={10}
                value={draft.radius_m}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    radius_m: parseInt(e.target.value, 10) || 100,
                  })
                }
              />
              <p className="text-xs text-muted-foreground">
                Smaller = more precise entry/exit but more false triggers from
                GPS noise. 100m is a sensible default.
              </p>
            </div>
          </div>

          <DialogFooter>
            <Button
              variant="ghost"
              onClick={() => setDialog({ open: false, editing: null })}
              disabled={busy}
            >
              Cancel
            </Button>
            <Button onClick={() => void submit()} disabled={busy}>
              {dialog.editing ? "Save" : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
