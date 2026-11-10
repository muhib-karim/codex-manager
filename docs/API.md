# Proxy API

The backend contains a local HTTP proxy that speaks the OpenAI API. It is started through the
`start_proxy_server` Tauri command; the UI cannot start it yet (see the
[project status](../README.md#project-status)). This document describes what the proxy code in
`src-tauri/src/proxy.rs` does.

## Base URL

```
http://127.0.0.1:8080/v1
```

The bind address comes from the proxy configuration (`bind_addr`, default `127.0.0.1:8080`).

## Authentication

Every request except the health check must carry the proxy key:

```
Authorization: Bearer sk-codex-manager
```

`sk-codex-manager` is the default value of `api_key` in the proxy configuration. It is not an
OpenAI key, and it should be changed before real accounts are used: any local program that knows
it can send requests through your accounts. The proxy sends no CORS headers, so browsers cannot
call it from web pages.

## Health check

```http
GET /health
GET /healthz
```

No key required.

```json
{
  "status": "ok",
  "version": "0.3.0",
  "uptime_seconds": 3600
}
```

`status` is `ok` when at least one account can take requests and `degraded` otherwise.

## Everything else is passed through

Any other path is forwarded to the upstream API (`openai_base_url`, default
`https://api.openai.com`) with the same method, path, query string, body and content type. There
is no fixed list of endpoints: `/v1/chat/completions`, `/v1/embeddings`, `/v1/models`,
`/v1/files/...` and anything else the upstream offers behave the same way.

What the proxy changes:

- `Authorization` is replaced with the key of the account the routing engine selected, and
  `OpenAI-Organization` is set from that account when it has an organization.
- Only these request headers are forwarded: `content-type`, `accept`, `openai-beta`,
  `openai-project`, `idempotency-key`, `user-agent`.
- The response keeps the upstream status code and body. The body is streamed, so server-sent
  events arrive as they are produced. Only these response headers are returned: `content-type`,
  `cache-control`, `retry-after`, `x-request-id`, and any `x-ratelimit-*` or `openai-*` header.
- Request bodies larger than 64 MiB are rejected.

The account is chosen per request from the `model` field of a JSON body (accounts can be limited
to certain models) and, for the `sticky` strategy, from a hash of the first chat message. See
[routing strategies](../README.md#routing-strategies).

## Errors produced by the proxy

Upstream errors are returned unchanged. The proxy's own errors use this shape:

```json
{
  "error": {
    "message": "Missing or invalid proxy API key",
    "type": "codex_manager_proxy_error",
    "code": "invalid_api_key"
  }
}
```

| Status | `code` | Meaning |
|---|---|---|
| 401 | `invalid_api_key` | Missing or wrong proxy key |
| 405 | `unsupported_method` | HTTP method cannot be forwarded |
| 413 | `request_too_large` | Body could not be read or exceeds 64 MiB |
| 502 | `upstream_unreachable` | The upstream API could not be reached |
| 502 | `invalid_upstream_response` | The upstream response could not be forwarded |
| 503 | `no_available_account` | No enabled account within its limits can serve the request |

There is no retry on another account within a request. Three consecutive account-level failures
(401, 403, 429, 5xx or a connection error) take an account out of rotation for 60 seconds.

## Client examples

```bash
curl http://127.0.0.1:8080/health

curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-codex-manager" \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "Hello"}]}'
```

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="sk-codex-manager")
reply = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Hello"}],
)
```

```ts
import OpenAI from 'openai';

const client = new OpenAI({ baseURL: 'http://127.0.0.1:8080/v1', apiKey: 'sk-codex-manager' });
```

## Not implemented

WebSocket endpoints, per-request routing headers (such as an account id in the response),
proxy-side rate limiting and automatic retries do not exist.
