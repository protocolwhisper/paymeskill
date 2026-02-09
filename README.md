# Payloadex MVP (Rust + React)

Minimal full-stack MVP for a sponsored x402-style payment flow:

- User profiles with role/tool attributes
- Sponsor campaigns with targeting and budgets
- Task gating before sponsor subsidy is unlocked
- Paywalled tool endpoint returning HTTP `402` without payment proof
- Proxy endpoint that pays on behalf of eligible users
- Creator telemetry endpoints for skill monitoring
- Prometheus metrics endpoint
- x402scan settlement ingestion webhook
- React operator dashboard (`payloadex`) inspired by x402scan visual structure

## Stack

- Rust + `axum`
- React + Vite + TypeScript
- In-memory state (`RwLock<HashMap<...>>`)
- Prometheus metrics (`/metrics`)

## Run Backend

```bash
cargo run
```

Server defaults to `http://localhost:3000`.

## Run Frontend

From `/frontend`:

```bash
npm install
npm run dev
```

Frontend runs at `http://localhost:5173` and proxies `/api/*` to backend `http://127.0.0.1:3000`.

## Why This Enforces Payment

Hard enforcement rule for other agents:

1. Expose only your paid tool bridge (for example `scrape_url`) to calling agents.
2. Tool bridge must route to `/proxy/:service/run` or `/tool/:service/run`.
3. If payment is missing or invalid, backend returns `402`, no payload data.
4. Data is returned only after payment proof verification or sponsor eligibility checks.

## API Flow (MVP)

1. Create user profile

```bash
curl -s -X POST http://localhost:3000/profiles \
  -H 'content-type: application/json' \
  -d '{
    "email":"dev@example.com",
    "region":"US",
    "roles":["developer"],
    "tools_used":["scraping","storage"],
    "attributes":{"experience":"indie"}
  }'
```

2. Create sponsor campaign

```bash
curl -s -X POST http://localhost:3000/campaigns \
  -H 'content-type: application/json' \
  -d '{
    "name":"Infra Adoption Push",
    "sponsor":"Acme Infra",
    "target_roles":["developer"],
    "target_tools":["scraping"],
    "required_task":"signup_acme",
    "subsidy_per_call_cents":5,
    "budget_cents":500
  }'
```

3. Mark sponsor task completion

```bash
curl -s -X POST http://localhost:3000/tasks/complete \
  -H 'content-type: application/json' \
  -d '{
    "campaign_id":"<CAMPAIGN_ID>",
    "user_id":"<USER_ID>",
    "task_name":"signup_acme",
    "details":"completed onboarding"
  }'
```

4. Run sponsored request via proxy

```bash
curl -s -X POST http://localhost:3000/proxy/scraping/run \
  -H 'content-type: application/json' \
  -d '{"user_id":"<USER_ID>","input":"collect top 20 AI tool prices"}'
```

5. Direct user payment flow (no sponsor)

```bash
curl -s -X POST http://localhost:3000/payments/mock/direct \
  -H 'content-type: application/json' \
  -d '{"payer":"dev@example.com","service":"design"}'
```

Use returned `payment_signature` as `payment-signature` header:

```bash
curl -s -X POST http://localhost:3000/tool/design/run \
  -H 'content-type: application/json' \
  -H 'payment-signature: <PAYMENT_SIGNATURE>' \
  -d '{"user_id":"<USER_ID>","input":"generate landing page options"}'
```

## Creator Metrics (Skill Monitoring)

Record skill lifecycle events:

```bash
curl -s -X POST http://localhost:3000/creator/metrics/event \
  -H 'content-type: application/json' \
  -d '{
    "skill_name":"payloadexchange-operator",
    "platform":"codex",
    "event_type":"invoked",
    "duration_ms":320,
    "success":true
  }'
```

Read summary:

```bash
curl -s http://localhost:3000/creator/metrics
```

Prometheus scrape:

```bash
curl -s http://localhost:3000/metrics
```

## x402scan: Does It Help?

Yes, useful for MVP ops:

- Discover x402-enabled endpoints and monitor them externally
- Track settlement/transaction activity outside your app
- Reconcile external settlement updates back into this service

Ingest updates into this endpoint:

```bash
curl -s -X POST http://localhost:3000/webhooks/x402scan/settlement \
  -H 'content-type: application/json' \
  -d '{
    "tx_hash":"0xabc",
    "service":"scraping",
    "amount_cents":5,
    "payer":"Acme Infra",
    "source":"sponsor",
    "status":"settled",
    "campaign_id":"<CAMPAIGN_ID>"
  }'
```

## Skill Included

Local skill folder:

- `skills/payloadexchange-operator/SKILL.md`
- `skills/payloadexchange-operator/agents/openai.yaml`

## Frontend Surface

The `payloadex` React app includes:

- Dark-mode overview page
- Live campaign table from `/api/campaigns`
- Live creator telemetry summary from `/api/creator/metrics`
- Campaign creation form that posts to `/api/campaigns`
- Integration path panel documenting paid-tool runtime flow

## Install Skill into Codex

1. Copy the skill folder into your Codex skills directory.

```bash
mkdir -p "${CODEX_HOME:-$HOME/.codex}/skills"
cp -R skills/payloadexchange-operator "${CODEX_HOME:-$HOME/.codex}/skills/"
```

2. Restart Codex to load new skills.

## Add Equivalent Setup in Claude

Claude does not use Codex `SKILL.md` directly. Use one of these:

1. Create a project/system prompt using the workflow from `skills/payloadexchange-operator/SKILL.md`.
2. Connect this Rust API as an external tool layer (for example MCP gateway or API action layer) and call:
   - `/proxy/:service/run`
   - `/tool/:service/run`
   - `/creator/metrics/event`
