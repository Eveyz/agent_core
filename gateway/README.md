# Agverse Gateway

HTTP control plane for hosting Agverse agents remotely. Inspired by the
[Cursor Cloud Agents API](https://cursor.com/docs/cloud-agent/api/endpoints).

Domain mapping:

| Cursor | Agverse gateway |
|--------|-----------------|
| Agent (durable) | Session + sidecar metadata (`~/.agverse/gateway/agents/`) |
| Run (per prompt) | `RunManager` run |
| SSE stream | `Envelope` / `RunEvent` → Cursor-like SSE events |
| Artifacts | `{workspace}/artifacts/` |

## Quick start

```bash
export AGVERSE_API_KEY=dev-secret
# optional:
# export AGVERSE_GATEWAY_BIND=0.0.0.0:8787
# export AGVERSE_GATEWAY_PUBLIC_URL=http://127.0.0.1:8787
# export AGVERSE_CONFIG=~/.agverse/config.toml

cargo run -p agverse-gateway
```

Auth: `Authorization: Bearer <AGVERSE_API_KEY>` or HTTP Basic (key as username).

## Endpoints (`/v1`)

| Method | Path | Notes |
|--------|------|-------|
| `GET` | `/health` | Liveness (no auth) |
| `GET` | `/v1/me` | API key info |
| `GET` | `/v1/models` | Models from config |
| `POST` | `/v1/agents` | Create agent + initial run |
| `GET` | `/v1/agents` | List agents |
| `GET` | `/v1/agents/{id}` | Get agent |
| `POST` | `/v1/agents/{id}/archive` | Soft-delete |
| `POST` | `/v1/agents/{id}/unarchive` | Restore |
| `DELETE` | `/v1/agents/{id}` | Permanent delete |
| `POST` | `/v1/agents/{id}/runs` | Follow-up run (`409` if busy) |
| `GET` | `/v1/agents/{id}/runs` | List runs |
| `GET` | `/v1/agents/{id}/runs/{runId}` | Get run |
| `GET` | `/v1/agents/{id}/runs/{runId}/stream` | SSE |
| `POST` | `/v1/agents/{id}/runs/{runId}/cancel` | Cancel |
| `POST` | `/v1/agents/{id}/runs/{runId}/approve` | Resolve tool approval |
| `POST` | `/v1/agents/{id}/runs/{runId}/answer` | Resolve `ask_user` |
| `GET` | `/v1/agents/{id}/artifacts` | List `artifacts/` |
| `GET` | `/v1/agents/{id}/artifacts/download?path=` | Short-lived content URL |
| `GET` | `/v1/agents/{id}/artifacts/content?path=` | Raw bytes |

## Create agent — workspace modes

**Host cwd** (path already on the gateway machine):

```bash
curl -sS -X POST http://127.0.0.1:8787/v1/agents \
  -H "Authorization: Bearer $AGVERSE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": { "text": "Summarize this repo" },
    "env": { "type": "host", "cwd": "/absolute/path/to/repo" }
  }'
```

**Git clone** (cloned under `~/.agverse/gateway/workspaces/<agentId>/`):

```bash
curl -sS -X POST http://127.0.0.1:8787/v1/agents \
  -H "Authorization: Bearer $AGVERSE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": { "text": "Find the entrypoint" },
    "env": { "type": "git" },
    "repos": [{ "url": "https://github.com/org/repo.git", "startingRef": "main" }]
  }'
```

## SSE event types

Aligned with Cursor: `status`, `assistant`, `thinking`, `tool_call`,
`interaction_update`, `result`, `error`, `done`, plus keepalive heartbeats.

Approvals / clarifications arrive as `interaction_update` and are resolved via
the `approve` / `answer` endpoints.
