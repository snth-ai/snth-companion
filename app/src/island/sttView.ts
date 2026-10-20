// What the island shows of on-device speech-to-text, apart from React so
// it can be tested (node --test src/island/*.test.mjs).

/** The live transcript fits two lines: the end of it, from a word. */
export const LIVE_CHARS = 110;

/** How long the sent text stays after "accepted". */
export const SENT_TEXT_FOR = 1500;

export function tailOf(text: string, max = LIVE_CHARS): string {
  const t = text.trim();
  if (t.length <= max) return t;
  const cut = t.slice(t.length - max);
  const space = cut.indexOf(" ");
  return "…" + (space >= 0 && space < 24 ? cut.slice(space + 1) : cut);
}

/** "about 550 MB": the download in MiB, to the ten. */
export function aboutMb(bytes: number): number {
  return Math.round(bytes / (1024 * 1024) / 10) * 10;
}

/** The words show while the key is held, while the text goes, and a
 * moment after the synth accepted it. */
export function transcriptVisible(
  v: { transcript: string | null; hold: string | null; sending: boolean; message: { stage: string; at_ms: number } | null },
  now: number,
): boolean {
  if (!v.transcript?.trim()) return false;
  const sentFresh = Boolean(v.message && v.message.stage === "accepted" && now - v.message.at_ms < SENT_TEXT_FOR);
  return Boolean(v.hold || v.sending || sentFresh);
}
