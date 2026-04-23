import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"

// Placeholder — until each page is ported over to the React UI the
// corresponding tab renders this card that links back to the old
// server-rendered page. The link opens in a new tab so the operator
// still has the Wave 7 HTML fallback.
export function Placeholder({
  title,
  legacy,
  note,
}: {
  title: string
  legacy: string
  note?: string
}) {
  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Not ported to the new UI yet.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Use the legacy view</CardTitle>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground space-y-3">
          <p>
            Open the old server-rendered page at{" "}
            <a
              className="text-primary hover:underline font-mono"
              href={legacy}
            >
              {legacy}
            </a>
            .
          </p>
          {note ? <p className="text-xs">{note}</p> : null}
        </CardContent>
      </Card>
    </div>
  )
}
