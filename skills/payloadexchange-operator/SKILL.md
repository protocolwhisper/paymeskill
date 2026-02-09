---
name: payloadexchange-operator
description: Operate sponsored x402-style campaigns for developer tools, including profile onboarding, sponsor campaign setup, task gating, proxy-paid usage, and creator telemetry logging. Use when users need to run or monitor PayloadExchange MVP flows and skill performance metrics.
---

# PayloadExchange Operator

## Workflow

1. Start the Rust API service.
2. Create user profiles with role/tool attributes.
3. Create sponsor campaigns with target roles, target tools, task gate, and budget.
4. Record sponsor task completion before allowing proxy-sponsored usage.
5. Use `/proxy/:service/run` for sponsored flows and `/tool/:service/run` for direct paid flows.
6. Log skill usage outcomes to `/creator/metrics/event`.
7. Read `/creator/metrics` and `/metrics` for operational monitoring.

## Metric Event Contract

Send one telemetry event per key skill action:

- `event_type=created` when skill definition is created
- `event_type=installed` when copied into Codex/other environment
- `event_type=invoked` when skill starts handling a request
- `event_type=completed` when task succeeds
- `event_type=failed` when task fails

Required fields: `skill_name`, `platform`, `event_type`, `success`.
Optional field: `duration_ms`.

## x402 Settlement Sync

Use `/webhooks/x402scan/settlement` to ingest external settlement updates and keep sponsored/direct ledger state consistent with the payment rail monitor.
