# Decision: Closed F2 Widget Catalog (`0005` precursor)

Date: 2026-11-16
Phase: F2 ("Base da UI")
Status: Closed before Gate 2

## Context

Phase F2 builds only the native controls actually needed by the Phase F3 screens (`Calendar` / `WeekGrid`, `Day Timeline` + `TimerBar` + `EntryEditor` + `ActivityPopover`, `Projects` + `ProjectReport`, `Reports` + `EntryTree`, and `Welcome` / `Settings`). Keeping the toolkit strictly closed avoids dead abstraction layers, binary bloat, and untested accessibility surfaces.

## Final F2 Widget List

| Widget | Module | Role / Responsibility | Used by F3 screens |
| --- | --- | --- | --- |
| Button | `win32/src/ui/widgets/button.rs` | `Secondary`, `Primary`, `Ghost`, `Danger` variants; `Regular`, `Small`, `Icon` sizes; optional Fluent glyph + label | All screens (actions, caption/modal close, timer start/stop, period navigation) |
| Toggle | `win32/src/ui/widgets/toggle.rs` | `Switch` (`36x20`) and `Checkbox` (`16x16`) with shared toggled state and AccessKit `Switch`/`CheckBox` semantics | Settings toggles, manual/billable/tracker checkboxes |
| Segmented | `win32/src/ui/widgets/segmented.rs` | Single-select pill track (`Day`/`Week`, view tabs) with `Group` + `RadioButton` roles and Left/Right keyboard nav | Calendar/Reports/Shell navigation switchers |
| Select | `win32/src/ui/widgets/select.rs` | Trigger button + `WS_POPUP` `ListBox` menu clamped to monitor work area; `ComboBox` + `ListBoxOption` AccessKit states | Project picker, grouping/filter dropdowns |
| TextField | `win32/src/ui/widgets/text_field.rs` | Borderless `RICHEDIT50W` (`msftedit.dll`) with OS `IMF_SPELLCHECKING`, canvas border & focus ring, DPI font scaling, high-contrast tinting, and Tab/Shift+Tab/Esc routing | Entry editor notes/title, settings/project names |
| Dialog | `win32/src/ui/widgets/dialog.rs` | Modal scrim (`HitMode::Opaque`) + centered card + header Close button, `FocusScope::Modal` Tab trap and `Esc` dismissal | Entry editor modal, create/edit project modal, confirmation prompts |
| Popover | `win32/src/ui/widgets/popover.rs` | Anchored `WS_POPUP` surface panel backed by `PopupWindow`, auto-closing on `WA_INACTIVE` or `Esc` | Activity detail popover, quick project assignment |
| Table | `win32/src/ui/widgets/table.rs` | Multi-column sortable rows with `Table`/`ColumnHeader`/`Row`/`Cell` AccessKit roles and empty state | Projects breakdown, Reports activity tables |
| VirtualList | `win32/src/ui/widgets/virtual_list.rs` | O(visible) virtualized list viewport with proportional scrollbar thumb, clamped wheel scroll, and keyboard navigation | Day/week activity timeline and large entry lists |
| Charts | `win32/src/ui/widgets/charts.rs` | Direct2D `DrawCmd` primitives (`StackedBars`, `Heatmap`, `Donut`, `ProportionBar`, `Spark`, `ShareRow`) driven by `Palette` and `PROJECT_COLORS` tokens | Today/Reports/ProjectReport visualizations |

## Shared Foundations Used Across Widgets

- `Tree` (`win32/src/ui/tree.rs`): Flexbox layout (`taffy`), clip/scroll offsets, reverse z-order hit-testing, and `RichTextSlot` synchronization.
- `FocusModel` (`win32/src/ui/focus.rs`): Main / Modal / Popup scopes, `Tab` / `Shift+Tab`, focus-visible ring only on keyboard navigation, `Esc` unwinding.
- `a11y` (`win32/src/ui/a11y.rs`): AccessKit UIA bridge + `validate_tree_names` enforcement (every interactive/widget node requires a non-empty `name`).
- `PopupWindow` (`win32/src/ui/popup.rs`): Top-level `WS_POPUP` repositioned via `MonitorFromPoint` + `GetMonitorInfoW` `rcWork`.
- `SystemPrefs` & `Palette` (`win32/src/ui/prefs.rs`, `theme.rs`): WinRT `TextScaleFactor`, High Contrast `GetSysColor`, Reduced Motion, Mica with solid fallback.

## Explicitly Excluded from F2

- General-purpose animation/spring timeline engines (instant transitions honor `SPI_GETCLIENTAREAANIMATION` and keep idle CPU at 0%).
- Custom scrollbars/splitters/drag-and-drop tree reordering (not required by F3 screens).
- Floating toast stack, rich markdown renderer, or custom text shaping editor (`RICHEDIT50W` covers all editing & spellcheck needs natively).
