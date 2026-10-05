# Editing with MCP

`rawmakase mcp` exposes RAWmakase's live editor to a local MCP client. It ships
in the existing executable, uses stdio, and needs no Python/Node installation,
account, login, or API key. The desktop app must be running and responsive.

## Connect

1. Start RAWmakase and open a catalog.
2. Enable **Preferences > Automation > Allow local scripts and applications**.
3. Configure your MCP client to launch the RAWmakase executable with argument
   `mcp`. Use an absolute executable path. On macOS the installed executable is
   `/Applications/RAWmakase.app/Contents/MacOS/rawmakase`.

For Codex, add this to your MCP configuration, adjusting the executable path:

```toml
[mcp_servers.rawmakase]
command = "/Applications/RAWmakase.app/Contents/MacOS/rawmakase"
args = ["mcp"]
tool_timeout_sec = 150
```

Alternatively, register the command with the CLI, then set `tool_timeout_sec`
in that server's configuration to allow the preview's 120-second wait:

```sh
codex mcp add rawmakase -- /Applications/RAWmakase.app/Contents/MacOS/rawmakase mcp
```

See the [official Codex MCP configuration documentation](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).
For clients using `mcpServers` JSON, the equivalent entry is:

```json
{
  "mcpServers": {
    "rawmakase": {
      "command": "/Applications/RAWmakase.app/Contents/MacOS/rawmakase",
      "args": ["mcp"]
    }
  }
}
```

For an app with a separate data directory, append `--data-dir` and its absolute
path to the arguments. The default follows `RAWMAKASE_DATA_DIR` and the app's
platform conventions. The adapter reads the local socket token automatically;
there is no separate MCP authentication flow. MCP opens no network listener.

## Tools

| Tool | What it does |
| --- | --- |
| `get_capabilities` | Lists parameter names, displayed units/ranges, supported actions and curve limits |
| `get_state` | Reads current photo, generation/revision, values, curves, masks, loading and saving state |
| `find_photos` | Searches catalog filenames with pagination |
| `open_photo` | Opens a catalog ID and waits up to 30 seconds for decoding |
| `set_parameter` | Sets exposure, contrast, highlights, shadows, whites, blacks, temperature, tint, texture, clarity, dehaze, vibrance, saturation or a color-mixer channel |
| `set_tone_curve` | Sets RGB, red, green or blue point curves; one channel per undo step |
| `apply_curve_preset` | Applies linear, medium-contrast or strong-contrast RGB curves, retaining the individual color channels |
| `auto_tone` | Starts automatic tone adjustment |
| `auto_white_balance` | Starts automatic white balance |
| `run_action` | Named operations such as undo/redo, reset, color/B&W treatment, ratings, flags and labels |
| `save_photo` | Waits for the current edit to be saved to the catalog |
| `preview_photo` | Returns a rendered JPEG image plus captured revision for visual inspection |
| `export_photo` | Starts a JPEG/TIFF export to a new absolute output path |
| `get_job` | Reports export progress, completion or failure |

`set_parameter` uses displayed units: Exposure in EV, Temperature in kelvin,
Tint in tint units, other basic controls in percent. Explicit masks use local
units listed by `get_capabilities`. Parameter values clamp to the allowed range.
Curve points instead reject invalid coordinates: 2–32 `[input, output]` pairs,
both values in 0–1, inputs increasing by at least 0.00049. Curves use the editor's
natural cubic interpolation. They are global; masks do not support curves.

All editing tools require a `target` containing the latest `generation` and
`revision`; include `photo_id` for catalog photos. Only parameter edits accept
an optional `mask` index. Use fresh guards returned in `structuredContent.state`
after each edit. Stale guards produce a tool error without applying the command.

## Example workflow

Ask your agent:

> Find DSCF0042, open it, and show me a preview. Lift exposure slightly, recover
> the highlights, and add a gentle S-curve. Show the result before saving.

The agent can:

1. Call `get_capabilities`, `find_photos`, and `open_photo`.
2. Read the returned state and preview the original edit with `preview_photo`.
3. Call `set_parameter` with `param: "exposure"`, `value: 0.35`, and current guards.
4. With the new guards, set `highlights` to `-20`.
5. With the next guards, call `set_tone_curve`:

   ```json
   {
     "channel": "rgb",
     "points": [[0, 0], [0.25, 0.2], [0.75, 0.8], [1, 1]],
     "target": {"photo_id": 17, "generation": 3, "revision": 9}
   }
   ```

   The target numbers are examples; use the actual state, not these literals.

6. Call `preview_photo` again. It returns image content directly to the client.
7. Use `run_action` with `undo` if needed, or `save_photo` to persist the result.
8. For a finished file, call `export_photo`, then `get_job` until `completed`.

Auto tools start background work. Poll `get_state` until `auto_running` is false,
then inspect the resulting values, the state’s `message`, and a preview; starting an operation does not
prove that its background calculation succeeded. Black-and-white conversions
may also report `treatment_pending`. Read fresh guards after background work.

## Completion and limits

Application errors are MCP tool errors with structured codes, including
`stale_target`, `busy`, `not_ready` and `protected`. A connection error or
`outcome_unknown` does not imply a mutation failed: inspect state before retrying.
The adapter never retries mutations automatically. It reconnects from the app's
connection file for each tool call, so restarting the app does not require
restarting the MCP server. Read fresh state and discard old target guards and output-job IDs after an app restart.

Preview uses a temporary JPEG, returns its pixels inline, and removes the file.
Its maximum edge defaults to 1600 and is limited to 2048; image payloads are
limited to 8 MiB. The reported preview path is temporary, not a persistent asset.
Use `export_photo` for a file to keep. Four MCP calls may be active at once;
RAWmakase allows two concurrent output jobs. Cancelling a wait does not undo an
already-applied edit or completed file export. Preview polling stops on
cancellation and removes its temporary directory.

A minimized or suspended desktop may not process commands; queued requests
expire safely. This server does not provide a headless editor, remote HTTP
access, or tools for creating/deleting masks. Existing masks can be adjusted.
For shell automation without MCP, see [External control](automation.md).
