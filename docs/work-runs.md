# Work Run worker

Status: disabled-by-default foundation for account-level Portal Work Runs.

The Hub owns Projects, Tasks, immutable Run attempts, context manifests,
leases, fencing tokens, and the normalized event log. The Companion owns the
local process boundary on the user's Mac. OpenPaw remains the synth supervisor
and Node API/Files authority; it does not spawn a second coding process.

## Enablement

The worker starts only when all of the following are present:

```text
SNTH_WORK_RUNNER_ENABLED=true
SNTH_WORK_HUB_URL=https://hub.example
SNTH_WORK_NODE_ID=node_...
SNTH_WORK_NODE_TOKEN=dedicated scoped node credential
SNTH_WORK_ACPX_BIN=/absolute/path/to/acpx
```

`SNTH_WORK_HUB_URL` may fall back to `HUB_URL`. The node token intentionally
does not fall back to `HUB_INTERNAL_TOKEN`: a desktop companion must not hold
the fleet-wide synth credential.

The Hub accepts `WORK_NODE_INTERNAL_TOKEN`; its legacy `HUB_INTERNAL_TOKEN`
fallback exists only for migration and local contract tests.

## v1 safety boundary

- Claims are serialized per Companion and support only `executor_type=codex`.
- Every Run requires a Companion approval prompt.
- The worker heartbeats a 30-second Hub lease and includes its fencing token
  in every event. Losing the lease cancels local execution.
- Hub supplies no command string. The adapter invokes a fixed `acpx` argv with
  `--deny-all`, `--no-fs`, `--no-terminal`, and non-interactive prompts set to
  fail. Codex ACP starts in `read-only` mode.
- The child receives a small allowlisted environment instead of the Companion
  process environment; arbitrary provider/connection secrets are not inherited.
- The Run's duration budget becomes both the local context deadline and acpx
  timeout.
- Workspaces live under the Companion support directory and are mode `0700`;
  prompt/transcript files are `0600`.
- Runs whose manifest requests repository/workspace/network access fail before
  execution.
- Agent-backed Runs carry the exact immutable Agent definition selected by
  Hub. The worker verifies its identity, version, executor, and budget, then
  places the frozen owner-authored instructions in the bounded prompt.
- Non-empty Agent tool grants fail closed with `tool_grants_unsupported` until
  a scoped connector grant broker exists. A definition may describe future
  Composio or Work tools without accidentally exposing them to today's Codex
  process.
- After owner approval, the worker redeems its active lease for a bounded
  context bundle. Hub supplies the exact frozen Work Document revisions and
  asks the owning Node API to materialize Finder text by stable ID plus frozen
  revision. A mismatch fails before execution; selected context is never
  silently dropped. Context blocks are explicitly marked as untrusted data in
  the executor prompt.

The final permission point deliberately makes this checkpoint useful for
read-only task/context analysis while keeping broader agent work unavailable
rather than unsafe.

Raw ACP NDJSON is retained locally, but Hub receives only owner-facing agent
message chunks in this checkpoint; initialization metadata, model inventory,
and tool catalog frames are filtered out.

## Executor prerequisite

`acpx` is an external runtime prerequisite, not downloaded by the daemon. The
configured binary must pass `acpx --version`; otherwise the Run receives a
terminal `executor_unavailable` event. Pin and provision the binary separately
before enabling the worker on a real node.

## Next contract slice

1. Add a short-lived signed per-Run grant bound to node, Run, manifest hash,
   operations, budget, and expiry.
2. Replace the current Hub-to-Node internal token on materialization with the
   signed per-Run grant and add artifact-safe binary extraction where needed.
3. Parse preserved ACP NDJSON into typed progress/tool/artifact events while
   retaining the raw transcript.
4. Enforce step/cost budgets and introduce explicit approval/answer actions.
5. Add artifact sealing and terminal controls independently of Run scheduling.
