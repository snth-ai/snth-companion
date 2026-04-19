# SNTH Companion

Local sidecar for [SNTH](https://snth.ai) synthetic companions. Runs on the
user's Mac, pairs 1:1 with a synth, and exposes a narrow set of local
capabilities (bash, filesystem, Apple Shortcuts) over a secure WebSocket to
the synth's backend.

## Status

Phase 1 (MVP) — skeleton.

## Architecture

```
Synth (Hetzner) ─tool call─> Companion (user's Mac)
                             │
                ┌────────────┼────────────┐
                │            │            │
              Bash         FS R/W     Apple Shortcuts
             (sandbox)     (scoped)
```

- **Transport:** persistent `wss://<synth>/api/companion/ws` opened by the
  companion (outbound, NAT-friendly). Survives laptop sleep via reconnect
  loop.
- **Sandbox:** by default tools operate under `~/SNTH/<synth-slug>/`. User
  can grant additional folder roots from the menubar UI. Out-of-sandbox
  operations require a native macOS approval prompt.
- **Pairing:** one companion = one synth. User runs `/pair_companion` in the
  synth's Telegram chat, gets a 6-digit code, pastes it into the companion
  UI. Hub mediates rendezvous; after claim the companion connects directly
  to the synth.

## Platforms

- macOS (ARM64 + Intel) — supported in MVP.
- Windows — planned for P3.

## Install

Not yet shipped. For now, build from source:

```bash
git clone https://github.com/snth-ai/snth-companion
cd snth-companion
go build -o snth-companion ./cmd/companion
./snth-companion
```

## License

MIT — see [LICENSE](LICENSE).
