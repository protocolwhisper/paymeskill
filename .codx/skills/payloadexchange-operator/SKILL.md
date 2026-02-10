---
name: payloadexchange-operator
description: Operate sponsored APIs and x402-style campaigns for PayloadExchange, including profile onboarding, sponsor campaign setup, task gating, sponsored API creation, proxy-paid usage, and creator telemetry logging.
---

# PayloadExchange Operator

## Workflow

1. Start the Rust API service.
2. If using sponsored APIs, ensure Postgres is configured (`DATABASE_URL`) and migrations are applied.
3. Create user profiles with role/tool attributes.
4. Create sponsor campaigns with target roles, target tools, task gate, and budget.
5. Record sponsor task completion before allowing proxy-sponsored usage.
6. Create sponsored APIs via `POST /sponsored-apis`.
7. If `SPONSORED_API_CREATE_PRICE_CENTS` > 0, first call `POST /sponsored-apis` without payment, read `PAYMENT-REQUIRED`, then retry with `PAYMENT-SIGNATURE` per x402.
8. Call sponsored APIs via `POST /sponsored-apis/:api_id/run`. Calls are free while `budget_remaining_cents` covers the per-call price; once exhausted, server returns `402` with `PAYMENT-REQUIRED`, then retry with `PAYMENT-SIGNATURE`.
9. Use `/proxy/:service/run` for sponsored campaign flows and `/tool/:service/run` for direct paid flows.
10. Log skill usage outcomes to `/creator/metrics/event`.
11. Read `/campaigns/discovery` for agent campaign URL sources.
12. Read `/creator/metrics` and `/metrics` for operational monitoring.

## Sponsored API Tracking

Each sponsored API call inserts a row into `sponsored_api_calls` with payment mode, amount, and caller metadata for budget reconciliation.

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
