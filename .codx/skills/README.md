# Skills + MCP (Minimal)

- Skills are local instruction folders (`SKILL.md`) that teach Codex how to operate this MVP.
- MCP (Model Context Protocol) is an optional tool gateway that exposes HTTP endpoints as model-callable tools.
- If you use MCP, map tools to the backend endpoints (for example: `/sponsored-apis`, `/sponsored-apis/:api_id/run`, `/proxy/:service/run`, `/tool/:service/run`).
- If you do not use MCP, call the HTTP endpoints directly from your client.
- Paid endpoints follow x402 flow: first request returns `402` + `PAYMENT-REQUIRED`, client retries with `PAYMENT-SIGNATURE`, server returns `PAYMENT-RESPONSE` on success.
