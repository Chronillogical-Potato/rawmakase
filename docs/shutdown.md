# Shutdown contract

What each long-lived worker thread may block on, how it is told to stop, and whether
the UI thread may wait for it when RAWmakase quits. Code that adds a worker, or
changes what one waits on, updates its row here.

## Rules

1. **Signal, then close, then join.** At exit the UI thread first sets every
   cancel and stop flag. It then drops the channel ends the workers wait on, and
   only then joins the workers listed as joinable below, with a deadline. GPU
   resources go last: the preview renderer is joined before eframe drops the
   painter and device.
2. **Join only what is bounded and needs nothing from the UI thread.** A worker is
   joinable when, once signalled, it finishes within its current job and never
   waits for a reply from the UI thread. Everything else stays detached, keeps a
   stop signal it checks, and has a comment saying why it is not joined.
3. **No blanket join in `Drop`.** Dropping a handle signals; joining is a decision
   made in one exit hook, in the order above.
4. **Never hold a bounded receiver while joining its sender.** A worker blocked in
   `send` on a full channel never sees its stop signal.
5. **The exit hook runs on every exit path, but cannot refuse one.** On macOS,
   Quit (Cmd-Q, the Dock, logging out) closes the window without a close request,
   so eframe calls `App::on_exit` but never the close guard, and the process ends
   right after it. `on_exit` therefore saves what it can synchronously: the edit
   and the autosave in flight. It cannot keep an export or Sync Settings running,
   or ask what to do about an edit that fails to save; see Quit on macOS below.
6. **One deadline bounds every join.** The loaders, previews, exports and output
   jobs read photo files, often on a network share, so a stalled read can outlast
   their current job. They are joinable only under the shared deadline: a worker
   still running when it passes is left detached and ends with the process. No
   worker is joined without a deadline.

## Workers

"Stop" is the signal the worker checks; "Join" is what the exit hook may do.

Joining needs each worker's `JoinHandle`. Today none is kept: `Latest`, the
autosave and the export queue discard theirs, so the code that joins starts by
keeping them.

| Worker | Where | Blocks on | Stop | Join |
| --- | --- | --- | --- | --- |
| Develop loader, with its full-size loader and prefetcher | `app/worker/loader.rs` | A job; file I/O and LibRaw decodes; decode cache reads and writes | `Latest` stopped, between jobs; the load `Task`'s cancel within one; `prefetch_cancel` for a prefetch | Yes, after cancelling the load and the prefetch |
| Preview renderer | `app/worker/renderer.rs` | A job; CPU and GPU renders, up to 10 s per GPU submission; the egui renderer lock, briefly | `Latest` stopped; the After and Before render `Task`s | Yes, before the painter and device are dropped |
| Reference View loader | `app/worker/reference.rs` | A job; file I/O and decodes | `Latest` stopped; the reference `Task` | Yes |
| Autosave | `app/autosave.rs` | The job channel; one SQLite commit | Its job sender dropped | Yes, after dropping the job sender, which also lets the last save finish |
| Export queue | `export/queue.rs` | A batch: decodes, renders and file writes | `closed`, between batches; the running batch's cancel, between photos and stages | Yes, after cancelling the running batch |
| Command output jobs | `app/commands/output.rs` | One export or preview job | Each job's cancel, between stages | Yes, after cancelling |
| MIDI listener | `app/automation/midi.rs` | A 2 s wait on its stop channel between port scans | The device dropped, which closes the channel | Yes; it stops at once |
| Control socket listener | `app/automation/socket.rs` | A blocking `accept` | The stop flag, seen after a wake-up connection | Bounded only: if the wake-up connection fails, `accept` never returns |
| Control socket connections | `app/automation/socket.rs` | A 5 s read; **a 3 s wait for the UI thread's reply**; a 5 s write | The stop flag; the request queue's receiver dropped | Bounded, and only after the request queue's receiver is dropped |
| Library thumbnails | `app/library/previews.rs` | The request channel; preview cache and file reads; **a send on a result channel bounded at 24** | Its channels dropped | Yes, after dropping the result receiver |
| Library edited previews | `app/library/previews.rs` | A queue of renders, none cancellable | Its channels dropped, seen when a result fails to send | Yes, bounded by one render |
| Library screen previews | `app/library/screen.rs` | A job; renders, none cancellable | `ScreenPreviews` closed | Yes, bounded by one render |
| Library background readers | `app/library/background.rs` | File reads in batches, possibly on a network share | The reader's cancel, per batch | No: a stalled share can hang a read |
| Volume probe | `app/library/volumes.rs` | `is_dir` and free space on every mount | None (one check per thread) | No: it can hang on a stalled mount |
| Availability check | `app/library/availability.rs` | File metadata for every photo | None | No, for the same reason |
| Update checker | `app/updates.rs` | Up to an hour between checks; a check or download of up to 15 min | Its request sender dropped, between requests | No: a download has no cancel |
| Usage stats | `stats.rs` | A 30 s sleep, then a report of up to 20 s | `enabled`, after the sleep | No |
| GVFS bridge reaper (Linux) | `platform/network.rs` | The bridge process, which outlives the app | None | Never |

One-shot jobs stay detached: Auto, Upright, Auto straighten, the Point Color and
Targeted Adjustment samples, the onboarding scan, catalog open and import, file
dialogs, bulk import, the preset scan, Sync Settings, folder relink and move, the
folder availability probe (which can hang on a stalled mount like the volume
probe), the update receipt acknowledgement, usage stats collection (which can run
the package manager) and the watermark fonts. They report through the event channel or a generation check,
so a result that arrives after its document is gone is dropped. File dialogs on
macOS run their panel on the main thread, so they are never joined from it.

## The exit sequence

1. Close guard, on a window close: refuse while an export, Sync Settings or a
   folder job runs, and flush the edit and the autosave in flight.
2. `on_exit`, on every path:
   1. Flush the edit and wait for the autosave in flight, if the close guard did not.
   2. Cancel: the load, render, reference and prefetch tasks, the running export
      batch and the command output jobs.
   3. Close: drop the autosave job sender, the update request sender, the control
      request receiver and the library's result receivers.
   4. Stop: the `Latest` workers, the export queue, the screen previews, MIDI and
      the control socket.
   5. Join the joinable workers above, with one deadline for all of them. The
      renderer is among them, so it is done before eframe drops the device; any
      worker still running at the deadline is left detached.
3. eframe drops the editor, then the painter. The detached workers end with the
   process.

## Quit on macOS

The close guard refuses to close while an export or Sync Settings runs, and asks
before closing when the edit cannot be saved. Quit bypasses it, and `on_exit`
cannot refuse, so until Quit is routed through the guard:

- the edit and the autosave in flight are saved synchronously;
- a running export is cut off: its finished photos stay, and the photo being
  written is not published, as exports write a temporary file and rename it;
- Sync Settings stops partway, leaving some of its photos synced and some not;
- an edit that fails to save is lost without a word.

The fix is for Quit to send the window a close request, so the guard runs as it
does for a window close. winit's default menu binds Quit to `terminate:`, which
does not, so this needs a Quit item of our own.

## Where the code stands

The exit hook (`app/exit.rs`) runs the sequence above. It saves the edit and the
place in the catalog (#269), cancels the jobs in progress, and stops and waits for
the develop loader, the preview renderer, the Reference View loader and the export
queue under a 3 s deadline (#270). Still to do:

- **Quit on macOS still cannot be refused** (see above): it cuts off a running
  export or Sync Settings.
- **The edit is saved without a deadline**, so a catalog on a stalled network share
  would hold up quitting.
- **The close guard misses folder jobs and command output jobs.**
- **Step 2.3 is not done yet:** the exit hook closes no channels itself; they
  close as the editor is dropped.
- **Not yet waited for:** the develop loader's nested full-size loader and
  prefetcher, the command output jobs (cancelled at quit), the library's preview
  workers, MIDI and the control socket. Each still stops through its own signal.
- **Renders that cannot be cancelled:** the library's edited and screen previews.
- **A worker cut off at the deadline** can leave its temporary file beside an
  export.
