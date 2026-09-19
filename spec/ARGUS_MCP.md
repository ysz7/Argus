# Argus MCP Server

**Version:** 0.1 (Argus 0.0.1) · **Transport:** stdio · **MCP revisions:**
2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05

`argus mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server that an AI client (Claude Desktop, Claude Code) starts as a local
process. It gives the client's model the agent view of the
[Observation Protocol](ARGUS_PROTOCOL.md) as tools, and executes the model's
actions after checking them. The model decides what to do; Argus describes
the interface, checks each action and executes it.

## 1. Transport

- newline-delimited JSON-RPC 2.0 on stdin and stdout; logs on stderr;
- methods: `initialize`, `ping`, `tools/list`, `tools/call`; notifications
  (`notifications/initialized`, cancellations) are accepted and not
  answered; batches (2025-03-26) are answered as batches;
- `initialize` answers the client's protocol revision when it is supported,
  else the newest one, and gives the model usage `instructions`;
- errors: `-32700` unreadable JSON, `-32600` invalid request, `-32601` unknown
  method, `-32602` unknown tool or missing `name`. A tool that runs and fails
  (refused action, no such element) answers normally with `isError: true`
  and the reason as text, so the model can correct itself.

Calls are served one at a time. Nothing touches the screen until the first
tool call.

## 2. Tools

Every tool answer starts with text and ends the text with the time Argus took
(`(Argus: 312 ms)`). Images are PNG `image` content items.

| Tool | Arguments | Answer |
|---|---|---|
| `list_apps` | — | names of the applications with windows, front to back; blocked ones marked `(blocked)`. No window titles |
| `observe` | `app` (name or bundle ID; required the first time) | the agent view of the application's front window, plus region images |
| `click` | target, `button` (`left` / `right` / `middle`), `count` (1–3), `modifiers` (`cmd`, `shift`, `alt`, `ctrl`) | what changed |
| `type_text` | `text` (≤ 5000 characters); optional `element_id` + `expect` (a field clicked first), `clear` (cmd+a first) | what changed |
| `press_keys` | `keys` (`"Return"`, `"cmd+shift+s"`, `"cmd++"`, `"Page_Down"`), `repeat` (1–50) | what changed |
| `scroll` | target (default: the window's center), `direction`, `amount` (lines, default 5) | what changed |
| `drag` | `from_…` target, `to_…` target | what changed |
| `screenshot` | — | the front window of the observed application as one image (for comparison; `observe` already sends images where needed) |

A **target** is one of:

- `element_id` with `expect`: the center of the element's visible bounds.
  `expect` is the name the model expects; the action is refused ("e_12 is
  "Cancel" (button), not "Save"; nothing was done") unless the element's
  name, description or label equals it or contains it as most of itself
  (`Save` fits `Save…`, not `Don't Save`), or its value contains it (a text
  field called by its content). Ignoring case, punctuation and spaces.
  `expect` is optional but recommended;
- `image` (`R1`, `R2`, …) with `x`, `y`: a pixel of a region image the model
  was shown;
- `x`, `y`: a point in global screen points.

`drag` prefixes the target fields with `from_` and `to_`.

### 2.1 The agent view

`observe` renders the observation as compact text (`argus_core::agent`):

```text
Calculator — observation obs_mu8n8p22_000001
Window "Calculator" (964,120 230x408)
e_1 window "Calculator" (964,120 230x408)
  e_8 text "0" (1165,209 19x36)
  row of 4 buttons (974,253 210x48): e_9 "Delete" | e_10 "All Clear" | e_11 "Percent" | e_12 "Divide"
  …
  e_33 button "Zoom" [disabled] (1028,138 16x16)
```

- one element per line: id, role, name, `= value`, `[state]`, bounds
  `(x,y wxh)` in screen points; nesting by indentation; `?` after an
  uncertain role or name, `(uncertain)` after an uncertain element;
- three or more like controls side by side share one line (`row of …`);
- text fields: `selected 0..5 "Title"` / `caret at 12` (when focused) and
  `styles: "Title" Helvetica-Bold 18pt bold; …` on continuation lines;
- pixel-only detections below confidence 0.5 are hidden (their count is
  given), their children are still shown;
- **a window without accessibility** is not listed in full: the text is a
  flat list of at most 60 named targets (controls first, then text), because
  the window's image comes with it.

### 2.2 Region images

Where text describes the window poorly, the answer includes images of those
regions (at most 3 per answer, at most 1400 pixels per side, one pixel per
screen point when it fits), each announced in the text:

```text
Image R1: region (100,80 640x480), no accessibility
```

Reasons: `no accessibility` (the whole window), `drawn content` (a large
area without structure: canvas, chart), `uncertain elements` (pixel-only
controls with uncertain roles or no names, not inside an accessible
control), `popup` (a menu or dialog window without accessibility in front of
the window).

After an action, a region image is sent again only if its pixels changed
visibly (more than 0.4% of the pixels), under the same label when the region
is the same. A full `observe` starts the labels again at `R1`.

### 2.3 After an action

Every action waits 450 ms, observes again and answers:

```text
Done: clicked at (1062,277).
Changes (observation obs_mu8n8p22_000004):
~ e_8 "7" name: "0" → "7"
```

(`+` added element, `-` removed, `~` changed property; noise such as
confidence changes and moves under 2 points is left out.) When tracking
cannot continue (another window, a new session), the whole view is sent
instead: `The window changed completely (observation …)`.

## 3. Guards

Checked on every call, in this order:

1. **Read-only.** With `--read-only`, every action is refused.
2. **Application.** Blocked applications are neither observed nor driven:
   terminals (Terminal, iTerm2, Warp, Ghostty, Alacritty, kitty), password
   stores (Keychain Access, Passwords, 1Password), System Settings, Script
   Editor, Automator, and AI chat clients and the editors hosting them
   (Claude, ChatGPT, VS Code, Cursor: the agent must not drive its own
   conversation). `--unblock-app NAME` lifts one entry;
   `--allow-app NAME` restricts the server to the listed applications.
3. **Target.** Actions go to the application of the latest observation. It
   is brought to the front first (refused if that fails). A pointer target
   must lie on one of its own windows (menus and pop-ups included) and not
   under another application's window.
4. **Element.** Element ids must exist in the latest observation; `expect`
   must match (§2).
5. **Keys.** System-wide combinations are refused: cmd+tab, cmd+\`,
   cmd+space, ctrl+space, cmd+alt+space, cmd+shift+q, cmd+ctrl+q,
   cmd+alt+escape, cmd+h, cmd+alt+h, cmd+m, cmd+alt+d, ctrl+arrows,
   cmd+shift+3/4/5, cmd+ctrl+f.
6. **The user.** Typing stops (`stopped: the user moved the mouse`) when the
   mouse moves during it.

The client's own confirmation of tool calls (Claude asks before running a
tool unless allowed) is a further, independent guard.

## 4. Privacy and logs

- observations and images go only to the client that started the server, as
  tool results; Argus stores nothing and sends nothing elsewhere. The
  client sends them to its model: observe only what may be shared with it;
- `list_apps` gives application names, never window titles;
- `--log-file PATH` appends one JSON line per call: time, tool, argument
  names (not values), duration, text size, image sizes, error flag. Never
  screen content;
- stderr logs follow the same rule.

## 5. Platform notes (macOS)

- The permissions (Accessibility, Screen & System Audio Recording) belong to
  the client application that starts `argus mcp`.
- Only one process of an executable can capture the screen at a time. So
  every `argus mcp` a client starts is a thin relay to one shared background
  process (`argus mcp --daemon`, started on demand, in its own process
  group) over a Unix socket in the user's private temporary directory
  (`argus-mcp.sock`, mode 0600). The relay sends its settings (policy, log
  file) as the first line; each client gets its own session and settings,
  requests of all clients run one at a time, and the background process
  exits 60 s after its last client left. `--standalone` serves in the
  client's own process instead. `argus serve` cannot capture while the
  background process runs.
- The first text recognition of a newly built binary loads the recognition
  models (tens of seconds); the server does it in the background at start,
  on a blank synthetic image.
