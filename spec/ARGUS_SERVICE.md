# Argus Local Service API

Version: v1 (Argus 0.0.1, protocol 0.2)

`argus serve` makes observations available to any local program over HTTP,
without linking with Rust. The payloads are those of the
[Argus Observation Protocol](ARGUS_PROTOCOL.md).

The key words MUST, SHOULD and MAY are to be interpreted as described in
RFC 2119.

## 1. Transport

- HTTP/1.1 on `127.0.0.1`, port `7412` by default (`argus serve --port N`,
  or `ARGUS_PORT`; `--port 0` picks a free port). On start, the service
  prints `listening on http://127.0.0.1:<port>` on stdout.
- Only `GET` requests are served; anything else is answered with `405` and
  `Allow: GET`. Requests have no body.
- One request per connection: every response carries `Connection: close`.
- Every body is JSON (`Content-Type: application/json`), UTF-8, compact.
- Responses are never cached (`Cache-Control: no-store`).

## 2. Security

- The service binds `127.0.0.1` only; it is not reachable from other
  machines.
- Requests MUST carry `Host: 127.0.0.1:<port>` or `localhost:<port>` (or
  `[::1]:<port>`). Any other `Host` is refused with `403 forbidden`: a web
  page whose domain was made to resolve to 127.0.0.1 (DNS rebinding) cannot
  read the screen.
- Requests with an `Origin` header are refused with `403 forbidden`:
  browsers send it on requests from web pages. No CORS headers are sent.
- Observations are kept in memory only, in a bounded history
  (`--history N`, default 32). Nothing is written to disk. Frames are kept
  only as the last one per observed window (for incremental perception) and
  those of the latest observation of each session (for `/frame`, §5).
- Pixels leave the service only through `/v1/observation/{id}/frame`, on
  request.
- The service observes only when asked. It never accepts actions to execute.

Any local process of the user can read the screen through the service; the
service is as trusted as the user's session.

## 3. Errors

Every error is answered with a status and a body:

```json
{ "error": { "code": "observation_not_found", "message": "observation `obs_x` is not in the service's history" } }
```

`code` is stable and machine-readable; `message` is for people and never
contains screen content.

| Status | Code                    | Meaning                                                                 |
| ------ | ----------------------- | ----------------------------------------------------------------------- |
| 400    | `bad_request`           | Malformed request, unknown or repeated query parameter, invalid value.  |
| 400    | `no_sources`            | No evidence source selected.                                            |
| 403    | `forbidden`             | Foreign `Host` or an `Origin` header (§2).                              |
| 403    | `permission_denied`     | macOS denied Accessibility or Screen Recording to the service.          |
| 404    | `not_found`             | No such endpoint, or the application/window to observe does not exist. |
| 404    | `observation_not_found` | The observation is not (or no longer) in the history.                   |
| 404    | `element_not_found`     | The observation has no such element.                                    |
| 405    | `method_not_allowed`    | Not a `GET` request.                                                    |
| 409    | `session_changed`       | `/v1/changes`: the new observation starts a new tracking session.       |
| 409    | `window_mismatch`       | The accessibility window and the captured window disagree.             |
| 431    | `request_too_large`     | The request head exceeds 16 KiB.                                        |
| 500    | `platform_error`, `invalid_frame`, `internal` | An internal failure.                           |
| 501    | `unsupported`           | A requested capability is unavailable on this platform.                 |
| 503    | `busy`                  | Too many requests are waiting; retry later.                             |
| 504    | `timeout`               | The operating system did not answer in time.                            |

Clients SHOULD branch on `code`, not on `message`.

## 4. Observing and sessions

Endpoints that observe accept:

| Parameter | Meaning                                                                            |
| --------- | ---------------------------------------------------------------------------------- |
| `app`     | Application name or bundle identifier (case-insensitive), e.g. `Calculator`.       |
| `pid`     | Process identifier. Cannot be combined with `app`.                                 |
| `sources` | Comma-separated evidence sources: `accessibility`, `ocr`, `vision` (default: all). |

Without `app` and `pid`, the focused window of the frontmost application is
observed.

Every distinct combination of application and sources has its own
**tracking session** (protocol §3.1): its successive observations keep
element IDs, whichever client asked for them. Programs watching different
applications do not disturb each other. Up to 8 sessions are kept; when
another starts, the least recently used one ends. Element IDs are unique
within a session only: `e_4` of one application and `e_4` of another are
different elements.

Observations are made one at a time. Responses about a new observation carry
its timings in the standard `Server-Timing` header:

```text
Server-Timing: accessibility;dur=23, capture;dur=52, ocr;dur=0, vision;dur=0, fusion;dur=0, tracking;dur=0, total;dur=77
```

## 5. Endpoints

### `GET /v1/health`

Whether the service runs. Never touches the screen.

```json
{ "status": "ok", "version": "0.0.1", "protocol_version": "0.1", "uptime_ms": 16565, "observations": 14 }
```

`observations` is the number of observations in the history.

### `GET /v1/observation`

Observes now and answers the [Observation](ARGUS_PROTOCOL.md#2-observation).
Parameters: `app`, `pid`, `sources` (§4).

```bash
curl 'http://127.0.0.1:7412/v1/observation?app=Calculator'
```

### `GET /v1/observation/{id}`

An earlier observation from the history, unchanged. `404
observation_not_found` once it has left the history.

### `GET /v1/changes?since={id}`

Observes again the application of observation `since`, with the same
sources, and answers the [ObservationDelta](ARGUS_PROTOCOL.md#10-observationdelta)
from `since` to the new observation. The new observation is in the history
under the delta's `to`; the next call is `/v1/changes?since=<to>`.

`since` may be any earlier observation of the same session still in the
history: the delta then covers all changes since it. If the new observation
starts a new session (another application or window came to the front, the
application restarted, the session ended), the element IDs of the two cannot
be compared: the answer is `409 session_changed`, and the new observation's
ID is given in `error.observation`:

```json
{ "error": { "code": "session_changed", "message": "...", "observation": "obs_mu81clxa_000015" } }
```

The client then reads it in full from `/v1/observation/{id}`.

A typical client loop:

```text
o = GET /v1/observation?app=Calculator
loop:
    d = GET /v1/changes?since=o.id
    409 session_changed → o = GET /v1/observation/{error.observation}
    200 → apply d to o (protocol §10); o.id = d.to
```

### `GET /v1/elements/{id}`

An element and the evidence behind it: which source said what, and which
conflicts were resolved how. Parameter `observation` names the observation;
without it, the latest observation of any session is used.

```json
{
  "observation": "obs_mu81clxa_000014",
  "element": { "id": "e_1", "role": "window", "...": "..." },
  "evidence": { "contributions": [{ "source": "accessibility", "...": "..." }] }
}
```

`evidence.conflicts` is present only when the sources disagreed. The
`evidence` format is diagnostic and may change between versions.

### `GET /v1/observation/{id}/frame`

The pixels of a region of an observation, as a PNG image (`Content-Type:
image/png`). Parameters: `x`, `y`, `width`, `height` (the region, in screen
points; required) and `scale` (image pixels per point, `0.1`–`2`, default
`1`).

Only the **latest observation of each session** keeps its frames, in memory;
for older observations, and for observations made without pixel sources, the
answer is `404 frame_not_available`. The image comes from the frame that best
covers the region (the window, or a pop-up of the application) and is clipped
to it.

### Agent view: `GET /v1/agent/observation`, `GET /v1/agent/changes?since={id}`

The same observations as `/v1/observation` and `/v1/changes`, rendered for
language-model agents: compact text, one element per line (`e_7 button
"Save" [disabled] (x,y wxh)`), rows of like controls on one line, selection
and styles of text controls, and deltas without noise. The text format is
meant for models, not for parsing; it may change between versions.

Parameters: those of `/v1/observation` (resp. `since`), and:

| Parameter        | Meaning                                                                                 |
| ---------------- | --------------------------------------------------------------------------------------- |
| `mode`           | `text` (default) or `hybrid`: also list the regions the text describes poorly.         |
| `min_confidence` | Pixel-only elements below this existence confidence are left out (default `0.5`).     |

```json
{
  "observation": "obs_mu8j0000_000002",
  "text": "Window \"Settings\" (54,129 577x612)\n...",
  "regions": [
    {
      "bounds": { "x": 54, "y": 129, "width": 577, "height": 612 },
      "reason": "no_accessibility",
      "image": "/v1/observation/obs_mu8j0000_000002/frame?x=54&y=129&width=577&height=612"
    }
  ]
}
```

`reason` is one of `no_accessibility` (the window has no accessibility tree),
`drawn_content` (a large area drawn without structure), `uncertain_elements`
(controls seen in pixels only, with uncertain roles or no names) and `popup`
(a menu or pop-up seen in pixels only). A client shows the agent the images of
these regions and the text for everything else. After an action,
`/v1/agent/changes` lists only the regions touched by the changes. Its answer
also carries `from`, and `"new_session": true` with the whole view instead of
changes when the new observation starts a new tracking session.

## 6. Platform notes (macOS)

- Permissions (Accessibility, Screen Recording) belong to the application
  that starts `argus serve` (Terminal, ...), as for the CLI.
- macOS lets only one running process of a program capture the screen: while
  `argus serve` runs, pixel sources of another `argus` process (e.g.
  `argus observe`) time out. Use the service instead, or stop it.
- The first OCR after a new `argus` binary is built can take 20–50 s while
  macOS prepares its models.
