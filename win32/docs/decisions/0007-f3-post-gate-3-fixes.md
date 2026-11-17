# 0007. Post-Gate-3 Fixes and First-Frame `< 200 ms` Recovery

Date: 2026-11-17
Branch: `win32/f3-fixes` (cut from `main` at `a982638`, leaving `win32-gate-3` intact)

## Context

A post-Gate-3 review identified high-severity (`A1`–`A4`), medium-severity (`M1`–`M10`), required low-severity (`B1`, `B2`), optional polish items (`B3`–`B10`), missing overlap edge-case assertions in `calendar_layout.rs`, and first-frame cold-open times above the `200 ms` Gate 2/3 budget across both synthetic SQLite (`233.3–252.9 ms` in [the prior Gate 3 capture](../measurements/2026-11-17-f3-screens.csv)) and `--demo` (`210.6–990.3 ms` on this host before fixes).

## Implemented Fixes

- **High severity (`A1`–`A4`)**:
  - `A1` (`f5d51aa`): Backfilled missing `maxActivityMs` with `MAX(end - start)` in `db.rs`.
  - `A2` (`ee377f8`): Preserved the original overnight entry offset in `moved_start` across segment drags in `week_grid.rs`.
  - `A3` (`f70e7cd`): Activated focused clickable widgets on `Enter`/`Space` and re-laid out popup trees (`popup.rs`, `activity_popover.rs`).
  - `A4` (`99e714d`): Exposed `WeekGrid` entries, sessions, and hover cursor as virtual UIA children and added keyboard shortcuts to open `ActivityPopover`.
- **Gate 3 overlap tests** (`6870f4d`): Added `seg.col`, abutting (`end == start`), transitive `A–B–C` chain column reuse, and equal-start assertions in `calendar_layout.rs`.
- **Medium severity (`M1`–`M10`)**:
  - `M1` (`9c86dfd`): Checked `Store::get_entry` before saving from `EntryEditor` so externally stopped timers do not reopen.
  - `M2` (`2cfdeb7`): Suspended global shortcuts behind active modals/popups and incremented dialog `sync_rev`.
  - `M3` (`e9164b8`): Cleared transient dialog/confirmation state when navigating away (`reset_transient`).
  - `M4` (`d498c2f`): Fixed midnight DST spring-forward day/week boundary arithmetic across `time.rs`, `period.rs`, `demo.rs`, and screens.
  - `M5` (`16a3471`): Computed ISO week labels (`W1`..`W53`) from the Thursday inside the 7-day range (`week_iso_number`).
  - `M6` (`1b2e536`): Switched `wants_tick` to `Option<u32>` (`None` in `--demo`, `Some(30_000)` idle SQLite Calendar, `Some(1000)` running timer) and updated the timer clock directly via `Tree::replace_styled_text`.
  - `M7` (`90bd2f2`): Replaced positional `WidgetId` assignment with deterministic `keyed_widget_id` / `Tree::set_key` across conditional widgets.
  - `M8` (`4f3637f`): Updated `RichEditField` fonts/bounds and popup window sizes on `WM_DPICHANGED`.
  - `M9` (`eb35769`): Measured real overflow heights via `Tree::set_root_height_fill`, made `ActivityPopover` scrollable, and auto-scrolled focused widgets on keyboard `Tab`.
  - `M10` (`d370264`): Collected fallback detail rows in the single `ProjectReport` scan and reset stale `selected_app` filters when absent.
- **Low severity (`B1`, `B2`, plus `B3`, `B10`)**:
  - `B1` (`06ddc70`): Moved remaining UI copy to `strings.rs` and aligned the zero-rate hint to `"0 = internal"`.
  - `B2` (`12e1cb4`): Separated save-dialog cancel (`ERROR_CANCELLED`) from errors, surfaced Settings/Welcome failures, and cleared stale ProjectDialog errors with redraw on edits.
  - `B3` (`a8405e5`): Resolved sidebar project hover state via `keyed_widget_id` instead of reading `id_side_projects` after `.clear()`.
  - `B10` (`4b16ca6`): Cleared stale CSV `export_notice` when switching `Reports` period or filter.

## First-Frame Optimization (`1f929cf`)

Stage profiling on Release builds (`--ui-show 1`) identified two main causes for `first_frame_ms >= 200 ms`:

1. Sequential startup initialization: `window::create()` blocked serially on `D3D11CreateDevice(WARP)` (`65–94 ms`), `D2D`/`DComp` (`15–22 ms`), `SystemPrefs::query()` (`16–26 ms` for WinRT `UISettings`), `AccessKit Adapter::new` (`20–31 ms`), and `ShowWindow` (`12–15 ms`), totaling `148–190 ms` before store construction (`41–48 ms` for `DemoStore::new`, `9–12 ms` for `Db::open`) and screen layout (`10–18 ms`) even began.
2. Synchronous native `RICHEDIT50W` creation before paint 1: `LoadLibraryW("msftedit.dll")` + first `CreateWindowExW("RICHEDIT50W")` added `41–56 ms` before `UpdateWindow()` on `Calendar` and `Settings`.

### Changes Applied (Attempts 1 and 2 combined in `1f929cf`)

- Moved WARP `D3D11`, `DXGI`, multi-threaded `Direct2D`, and `DirectComposition` initialization onto a background warmup worker thread (`render::warmup_d3d`) joined only inside `on_paint` when building `Renderer`.
- Overlapped `SystemPrefs::query()` + `TextSystem::new()` and background `DemoStore::build` / `Db::open` (`BgStore`) with `CreateWindowExW`, `AccessKit Adapter::new`, and `ScreenHost::mount`.
- Deferred `sync_rich_fields` to `WM_APP_SYNC_RICH` posted right after frame 1 paints (flushing synchronously on immediate focus, pointer-down, or key input), while Direct2D draws `Leaf::RichTextSlot` text/placeholder cleanly during frame 1.
- Updated `--ui-cycle` warm-up iterations to call `UpdateWindow` so lazy renderer warm-up settles prior to the 50-cycle measurement.

## Verification (`4b16ca6`, Release `2.15 MB`)

- `win32/scripts/check.ps1 -Release`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (**140 passed**), Release binary `2.15 MB` (`< 5 MB` budget).
- First-frame Release sweep (`3` runs per combination, synthetic benchmark DB with `5` projects, `90` entries, `600` activity rows + `onboarded=true`; [CSV](../measurements/2026-11-17-f3-fixes-screens.csv)):
  - **All 39 runs across all 13 combinations (`db` x6, `demo-fresh welcome`, `demo` x6) completed under `< 200 ms`**: raw range `117.0–191.0 ms`, medians `127.4–181.0 ms` (`db` medians `156.1–181.0 ms`, `demo-fresh welcome` median `167.1 ms`, `demo` medians `127.4–168.6 ms`), steady working set `32.01–47.57 MB` (`< 60 MB`).
  - Note: earlier isolated single-run cold invocations during iteration still occasionally showed Windows OS/AV scheduling spikes (`232–290 ms` and one `658.0 ms` outlier on `db projects`), while every steady 3-run sample on the final `2.15 MB` Release binary cleared `< 200 ms`. The original Gate 3 baseline CSV ([`2026-11-17-f3-screens.csv`](../measurements/2026-11-17-f3-screens.csv)) is kept intact per `gates.md`.
- `--diag-drag`: `esc_cancel=true alt_nosnap=true autoscroll=true`, `3840x2160` (`200%` DPI), `180` frames in `1594.59 ms` (`avg 8.859 ms`, `p95 11.553 ms`, `max 24.231 ms`, **`112.88 fps`**; [log](../measurements/2026-11-17-f3-fixes-diag-drag.txt)).
- `--ui-cycle 50 --all-screens`: `21.3 s`, `first_open 314.7 ms`, private memory `9.77 MB` -> `8.29 MB` (`-1.47 MB` growth, `< 30 MB` & `< 1 MB` budget; [CSV](../measurements/2026-11-17-f3-fixes-ui-cycle-all-screens.csv)).
- Open-window idle CPU ([CSV](../measurements/2026-11-17-f3-fixes-open-idle.csv)):
  - All 6 idle `--db` screens + idle `--demo calendar` / `--demo activity`: `0.000 s` across idle 1s intervals (one `0.016 s` tick on `db projects`), steady working set `36.86–49.05 MB` (`< 60 MB`).
  - Running timer on `db-running calendar`: `0.000–0.078 s` across 1s ticks from expected 1s lightweight clock updates (`steady_ws=45.05 MB`).

## Decision and F5 Backlog

- Keep the `< 200 ms` cold-open budget unchanged (`gates.md`); Gate 3 metrics remain satisfied without waiving any budget target.
- Deferred optional low-severity items (`B4`–`B9`, including `B7` draft exclusion / deterministic session sort in `week_grid` and `B9` `--db`/`--fresh` CLI validation in `main.rs`) are tracked in F5 backlog.
