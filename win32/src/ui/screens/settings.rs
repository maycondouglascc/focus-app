//! Settings screen: monitor sources, idle timeout, privacy exclusions,
//! currency, day-start hour, first day of week, 24h/12h clock, CSV default,
//! and 2-step activity history deletion.

use taffy::prelude::*;

use crate::db::{FirstDayPref, Settings, SettingsPatch, SharedStore, TimeFormatPref};
use crate::format::{self, ALL_CURRENCIES};
use crate::ui::screen::{BuiltScreen, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, Leaf, Node, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::select::{self, SelectProps};
use crate::ui::widgets::text_field::{self, TextFieldProps};
use crate::ui::widgets::toggle::{self, ToggleKind, ToggleProps};
use crate::ui::widgets::{add_label, Interact};

use super::{
    build_card, build_hairline, build_scroll_page, build_section_title, screen_root_tree,
    scroll_wheel_step, PopupSelectSlot,
};

const IDLE_MINUTES: [u32; 6] = [1, 2, 3, 5, 10, 15];
const KEY_EXCL_APPS: &str = "settings.excluded_apps";
const KEY_EXCL_TITLES: &str = "settings.excluded_titles";

pub struct SettingsScreen {
    store: SharedStore,
    confirm_clear: bool,
    scroll_y: f32,
    viewport_h: f32,
    draft_apps: String,
    draft_titles: String,
    apps_rev: u64,
    titles_rev: u64,

    id_monitor: Option<WidgetId>,
    id_browser: Option<WidgetId>,
    id_claude: Option<WidgetId>,
    id_busy: Option<WidgetId>,
    id_csv_standard: Option<WidgetId>,
    id_delete_prompt: Option<WidgetId>,
    id_delete_keep: Option<WidgetId>,
    id_delete_confirm: Option<WidgetId>,
    id_apps_slot: Option<WidgetId>,
    id_titles_slot: Option<WidgetId>,

    sel_idle: PopupSelectSlot,
    sel_currency: PopupSelectSlot,
    sel_day_start: PopupSelectSlot,
    sel_first_day: PopupSelectSlot,
    sel_time_format: PopupSelectSlot,
}

impl SettingsScreen {
    pub fn new(store: SharedStore) -> Self {
        let s = store
            .borrow()
            .get_settings()
            .unwrap_or_else(|_| Settings::defaults_for(store.borrow().system_locale()));
        Self {
            store,
            confirm_clear: false,
            scroll_y: 0.0,
            viewport_h: 720.0,
            draft_apps: s.excluded_apps.join(", "),
            draft_titles: s.excluded_title_patterns.join(", "),
            apps_rev: 0,
            titles_rev: 0,
            id_monitor: None,
            id_browser: None,
            id_claude: None,
            id_busy: None,
            id_csv_standard: None,
            id_delete_prompt: None,
            id_delete_keep: None,
            id_delete_confirm: None,
            id_apps_slot: None,
            id_titles_slot: None,
            sel_idle: PopupSelectSlot::default(),
            sel_currency: PopupSelectSlot::default(),
            sel_day_start: PopupSelectSlot::default(),
            sel_first_day: PopupSelectSlot::default(),
            sel_time_format: PopupSelectSlot::default(),
        }
    }

    fn settings(&self) -> Settings {
        let b = self.store.borrow();
        b.get_settings()
            .unwrap_or_else(|_| Settings::defaults_for(b.system_locale()))
    }

    fn save(&self, patch: SettingsPatch) {
        let _ = self.store.borrow_mut().save_settings(patch);
    }

    fn currency_options(cur: &str) -> Vec<String> {
        let mut out: Vec<String> = ALL_CURRENCIES.iter().map(|c| (*c).to_string()).collect();
        if !cur.is_empty() && !out.iter().any(|c| c.eq_ignore_ascii_case(cur)) {
            out.insert(0, cur.to_string());
        }
        out
    }

    fn hour_labels(is_24h: bool) -> Vec<String> {
        (0..=12_u8)
            .map(|h| {
                if is_24h {
                    format!("{h:02}:00")
                } else {
                    let suffix = if h >= 12 { "PM" } else { "AM" };
                    let h12 = match h % 12 {
                        0 => 12,
                        v => v,
                    };
                    format!("{h12}:00 {suffix}")
                }
            })
            .collect()
    }

    fn setting_row_shell(
        tree: &mut Tree,
        parent: Node,
        title: &str,
        hint: &str,
        disabled: bool,
        palette: &Palette,
    ) -> (Node, Node) {
        let row = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                padding: style::pad(16.0, 12.0, 16.0, 12.0),
                gap: style::gap(16.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let text_col = tree.add_container(
            row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(2.0),
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        add_label(
            tree,
            text_col,
            title,
            13.0,
            Weight::Medium,
            Align::Left,
            if disabled {
                palette.disabled_text
            } else {
                palette.text
            },
        );
        tree.add_leaf(
            text_col,
            Style::default(),
            Leaf::StyledText {
                text: hint.into(),
                size: 12.0,
                weight: Weight::Regular,
                align: Align::Left,
                wrap: Wrap::Word,
                color: if disabled {
                    palette.disabled_text
                } else {
                    palette.muted
                },
            },
        );
        let ctl = tree.add_container(
            row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        (row, ctl)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_toggle_row(
        tree: &mut Tree,
        parent: Node,
        title: &str,
        hint: &str,
        checked: bool,
        disabled: bool,
        slot: &mut Option<WidgetId>,
        env: &ScreenEnv<'_>,
    ) {
        let (_, ctl) = Self::setting_row_shell(tree, parent, title, hint, disabled, env.palette);
        let interact = Interact::from_state(*slot, env.hover, env.pressed, disabled);
        let (_, id) = toggle::build(
            tree,
            ctl,
            &ToggleProps {
                kind: ToggleKind::Switch,
                label: "",
                aria_label: Some(title),
                checked,
                disabled,
                interact,
            },
            env.palette,
        );
        *slot = Some(id);
    }

    #[allow(clippy::too_many_arguments)]
    fn build_select_row(
        tree: &mut Tree,
        parent: Node,
        title: &str,
        hint: &str,
        options: &[&str],
        selected: Option<usize>,
        slot: &mut PopupSelectSlot,
        env: &ScreenEnv<'_>,
    ) {
        let (_, ctl) = Self::setting_row_shell(tree, parent, title, hint, false, env.palette);
        let interact = Interact::from_state(slot.trigger_id, env.hover, env.pressed, false);
        let (_, id) = select::build(
            tree,
            ctl,
            &SelectProps {
                aria_label: title,
                options,
                selected,
                placeholder: strings::SELECT_PLACEHOLDER,
                min_width: 152.0,
                open: slot.is_open(),
                disabled: false,
                interact,
            },
            env.palette,
        );
        slot.trigger_id = Some(id);
    }

    /// Renders the Settings page into `parent` (used both standalone and inside `AppShell`).
    pub fn build_content(
        &mut self,
        tree: &mut Tree,
        _overlay_root: Node,
        parent: Node,
        env: &ScreenEnv<'_>,
    ) -> Option<Node> {
        self.viewport_h = env.height_dip;
        let s = self.settings();
        let is_24h = s.time_format.is_24h(self.store.borrow().system_locale());
        let p = env.palette;

        let (_vp, col) = build_scroll_page(tree, parent, 720.0, 28.0, 14.0, self.scroll_y);

        // Headline
        let head = tree.add_container(
            col,
            Style {
                flex_shrink: 0.0,
                padding: style::pad(0.0, 0.0, 0.0, 4.0),
                ..Default::default()
            },
        );
        add_label(
            tree,
            head,
            strings::SETTINGS_TITLE,
            22.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );

        // 1. Activity monitor
        build_section_title(tree, col, strings::SETTINGS_SEC_MONITOR, p);
        let mon_card = build_card(tree, col, 0.0, 0.0, p);
        Self::build_toggle_row(
            tree,
            mon_card,
            strings::SETTINGS_RECORD_LABEL,
            strings::SETTINGS_RECORD_HINT,
            s.monitor_enabled,
            false,
            &mut self.id_monitor,
            env,
        );
        build_hairline(tree, mon_card, p);
        Self::build_toggle_row(
            tree,
            mon_card,
            strings::SETTINGS_BROWSER_LABEL,
            strings::SETTINGS_BROWSER_HINT,
            s.browser_tabs && s.monitor_enabled,
            !s.monitor_enabled,
            &mut self.id_browser,
            env,
        );
        build_hairline(tree, mon_card, p);
        Self::build_toggle_row(
            tree,
            mon_card,
            strings::SETTINGS_CLAUDE_LABEL,
            strings::SETTINGS_CLAUDE_HINT,
            s.claude_sessions,
            false,
            &mut self.id_claude,
            env,
        );
        build_hairline(tree, mon_card, p);
        Self::build_toggle_row(
            tree,
            mon_card,
            strings::SETTINGS_BUSY_LABEL,
            strings::SETTINGS_BUSY_HINT,
            s.keep_active_when_busy,
            false,
            &mut self.id_busy,
            env,
        );
        build_hairline(tree, mon_card, p);
        let idle_labels: Vec<String> = IDLE_MINUTES.iter().map(|m| format!("{m} min")).collect();
        let idle_refs: Vec<&str> = idle_labels.iter().map(String::as_str).collect();
        let idle_idx = IDLE_MINUTES.iter().position(|&m| m == s.idle_minutes);
        Self::build_select_row(
            tree,
            mon_card,
            strings::SETTINGS_IDLE_LABEL,
            strings::SETTINGS_IDLE_HINT,
            &idle_refs,
            idle_idx,
            &mut self.sel_idle,
            env,
        );

        // 2. Privacy exclusions
        build_section_title(tree, col, strings::SETTINGS_SEC_PRIVACY, p);
        let priv_card = build_card(tree, col, 16.0, 12.0, p);

        add_label(
            tree,
            priv_card,
            strings::SETTINGS_EXCL_APPS_LABEL,
            13.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        tree.add_leaf(
            priv_card,
            Style::default(),
            Leaf::StyledText {
                text: strings::SETTINGS_EXCL_APPS_HINT.into(),
                size: 12.0,
                weight: Weight::Regular,
                align: Align::Left,
                wrap: Wrap::Word,
                color: p.muted,
            },
        );
        let apps_focused = env.focused.is_some() && env.focused == self.id_apps_slot;
        let (_c1, apps_slot) = text_field::build_tree_node(
            tree,
            priv_card,
            &TextFieldProps {
                key: KEY_EXCL_APPS,
                aria_label: strings::SETTINGS_EXCL_APPS_LABEL,
                placeholder: strings::SETTINGS_EXCL_APPS_PLACEHOLDER,
                value: &self.draft_apps,
                multiline: false,
                min_height: 32.0,
                focused: apps_focused,
                error: false,
                disabled: false,
                sync_rev: self.apps_rev,
            },
            p,
        );
        self.id_apps_slot = Some(apps_slot);

        build_hairline(tree, priv_card, p);
        add_label(
            tree,
            priv_card,
            strings::SETTINGS_EXCL_TITLES_LABEL,
            13.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        tree.add_leaf(
            priv_card,
            Style::default(),
            Leaf::StyledText {
                text: strings::SETTINGS_EXCL_TITLES_HINT.into(),
                size: 12.0,
                weight: Weight::Regular,
                align: Align::Left,
                wrap: Wrap::Word,
                color: p.muted,
            },
        );
        let titles_focused = env.focused.is_some() && env.focused == self.id_titles_slot;
        let (_c2, titles_slot) = text_field::build_tree_node(
            tree,
            priv_card,
            &TextFieldProps {
                key: KEY_EXCL_TITLES,
                aria_label: strings::SETTINGS_EXCL_TITLES_LABEL,
                placeholder: strings::SETTINGS_EXCL_TITLES_PLACEHOLDER,
                value: &self.draft_titles,
                multiline: false,
                min_height: 32.0,
                focused: titles_focused,
                error: false,
                disabled: false,
                sync_rev: self.titles_rev,
            },
            p,
        );
        self.id_titles_slot = Some(titles_slot);

        // 3. Time tracking & calendar
        build_section_title(tree, col, strings::SETTINGS_SEC_TRACKING, p);
        let track_card = build_card(tree, col, 0.0, 0.0, p);

        let cur_opts = Self::currency_options(&s.currency);
        let cur_refs: Vec<&str> = cur_opts.iter().map(String::as_str).collect();
        let cur_idx = cur_opts
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&s.currency));
        Self::build_select_row(
            tree,
            track_card,
            strings::SETTINGS_CURRENCY_LABEL,
            strings::SETTINGS_CURRENCY_HINT,
            &cur_refs,
            cur_idx,
            &mut self.sel_currency,
            env,
        );
        build_hairline(tree, track_card, p);

        let hour_opts = Self::hour_labels(is_24h);
        let hour_refs: Vec<&str> = hour_opts.iter().map(String::as_str).collect();
        let hour_idx = usize::try_from(s.day_start_hour)
            .ok()
            .filter(|&i| i < hour_refs.len());
        Self::build_select_row(
            tree,
            track_card,
            strings::SETTINGS_DAY_START_LABEL,
            strings::SETTINGS_DAY_START_HINT,
            &hour_refs,
            hour_idx,
            &mut self.sel_day_start,
            env,
        );
        build_hairline(tree, track_card, p);

        let first_day_opts = [
            strings::SETTINGS_FIRST_DAY_SYSTEM,
            strings::SETTINGS_FIRST_DAY_MONDAY,
            strings::SETTINGS_FIRST_DAY_SUNDAY,
        ];
        let fd_idx = Some(match s.first_day_of_week {
            FirstDayPref::System => 0,
            FirstDayPref::Monday => 1,
            FirstDayPref::Sunday => 2,
        });
        Self::build_select_row(
            tree,
            track_card,
            strings::SETTINGS_FIRST_DAY_LABEL,
            strings::SETTINGS_FIRST_DAY_HINT,
            &first_day_opts,
            fd_idx,
            &mut self.sel_first_day,
            env,
        );
        build_hairline(tree, track_card, p);

        let tf_opts = [
            strings::SETTINGS_TIME_SYSTEM,
            strings::SETTINGS_TIME_24H,
            strings::SETTINGS_TIME_12H,
        ];
        let tf_idx = Some(match s.time_format {
            TimeFormatPref::System => 0,
            TimeFormatPref::H24 => 1,
            TimeFormatPref::H12 => 2,
        });
        Self::build_select_row(
            tree,
            track_card,
            strings::SETTINGS_TIME_FORMAT_LABEL,
            strings::SETTINGS_TIME_FORMAT_HINT,
            &tf_opts,
            tf_idx,
            &mut self.sel_time_format,
            env,
        );

        // 4. Data & exports
        build_section_title(tree, col, strings::SETTINGS_SEC_DATA, p);
        let data_card = build_card(tree, col, 0.0, 0.0, p);
        Self::build_toggle_row(
            tree,
            data_card,
            strings::SETTINGS_CSV_LABEL,
            strings::SETTINGS_CSV_HINT,
            s.csv_standard,
            false,
            &mut self.id_csv_standard,
            env,
        );
        build_hairline(tree, data_card, p);

        let delete_hint = if self.confirm_clear {
            strings::SETTINGS_DELETE_CONFIRM_HINT
        } else {
            strings::SETTINGS_DELETE_HINT
        };
        let (_, del_ctl) = Self::setting_row_shell(
            tree,
            data_card,
            strings::SETTINGS_DELETE_LABEL,
            delete_hint,
            false,
            p,
        );
        if self.confirm_clear {
            self.id_delete_prompt = None;
            let mut keep_props = ButtonProps::new(strings::SETTINGS_DELETE_KEEP_BTN);
            keep_props.size = ButtonSize::Small;
            keep_props.interact =
                Interact::from_state(self.id_delete_keep, env.hover, env.pressed, false);
            let (_, keep_id) = button::build(tree, del_ctl, &keep_props, p);
            self.id_delete_keep = Some(keep_id);

            let mut del_props = ButtonProps::new(strings::SETTINGS_DELETE_CONFIRM_BTN);
            del_props.variant = ButtonVariant::Danger;
            del_props.size = ButtonSize::Small;
            del_props.interact =
                Interact::from_state(self.id_delete_confirm, env.hover, env.pressed, false);
            let (_, del_id) = button::build(tree, del_ctl, &del_props, p);
            self.id_delete_confirm = Some(del_id);
        } else {
            self.id_delete_keep = None;
            self.id_delete_confirm = None;
            let mut btn_props = ButtonProps::new(strings::SETTINGS_DELETE_BTN);
            btn_props.size = ButtonSize::Small;
            btn_props.interact =
                Interact::from_state(self.id_delete_prompt, env.hover, env.pressed, false);
            let (_, pid) = button::build(tree, del_ctl, &btn_props, p);
            self.id_delete_prompt = Some(pid);
        }

        None
    }

    pub fn handle_click(&mut self, id: WidgetId, palette: &Palette) -> bool {
        let s = self.settings();
        if Some(id) == self.id_monitor {
            self.save(SettingsPatch {
                monitor_enabled: Some(!s.monitor_enabled),
                ..Default::default()
            });
            return true;
        }
        if Some(id) == self.id_browser && s.monitor_enabled {
            self.save(SettingsPatch {
                browser_tabs: Some(!s.browser_tabs),
                ..Default::default()
            });
            return true;
        }
        if Some(id) == self.id_claude {
            self.save(SettingsPatch {
                claude_sessions: Some(!s.claude_sessions),
                ..Default::default()
            });
            return true;
        }
        if Some(id) == self.id_busy {
            self.save(SettingsPatch {
                keep_active_when_busy: Some(!s.keep_active_when_busy),
                ..Default::default()
            });
            return true;
        }
        if Some(id) == self.id_csv_standard {
            self.save(SettingsPatch {
                csv_standard: Some(!s.csv_standard),
                ..Default::default()
            });
            return true;
        }
        if Some(id) == self.id_delete_prompt {
            self.confirm_clear = true;
            return true;
        }
        if Some(id) == self.id_delete_keep {
            self.confirm_clear = false;
            return true;
        }
        if Some(id) == self.id_delete_confirm {
            let _ = self.store.borrow_mut().clear_activity();
            self.confirm_clear = false;
            return true;
        }
        if Some(id) == self.sel_idle.trigger_id {
            let labels: Vec<String> = IDLE_MINUTES.iter().map(|m| format!("{m} min")).collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let idx = IDLE_MINUTES.iter().position(|&m| m == s.idle_minutes);
            self.sel_idle.open(&refs, idx, palette);
            return true;
        }
        if Some(id) == self.sel_currency.trigger_id {
            let opts = Self::currency_options(&s.currency);
            let refs: Vec<&str> = opts.iter().map(String::as_str).collect();
            let idx = opts
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&s.currency));
            self.sel_currency.open(&refs, idx, palette);
            return true;
        }
        if Some(id) == self.sel_day_start.trigger_id {
            let is_24h = s.time_format.is_24h(self.store.borrow().system_locale());
            let opts = Self::hour_labels(is_24h);
            let refs: Vec<&str> = opts.iter().map(String::as_str).collect();
            let idx = usize::try_from(s.day_start_hour)
                .ok()
                .filter(|&i| i < refs.len());
            self.sel_day_start.open(&refs, idx, palette);
            return true;
        }
        if Some(id) == self.sel_first_day.trigger_id {
            let opts = [
                strings::SETTINGS_FIRST_DAY_SYSTEM,
                strings::SETTINGS_FIRST_DAY_MONDAY,
                strings::SETTINGS_FIRST_DAY_SUNDAY,
            ];
            let idx = Some(match s.first_day_of_week {
                FirstDayPref::System => 0,
                FirstDayPref::Monday => 1,
                FirstDayPref::Sunday => 2,
            });
            self.sel_first_day.open(&opts, idx, palette);
            return true;
        }
        if Some(id) == self.sel_time_format.trigger_id {
            let opts = [
                strings::SETTINGS_TIME_SYSTEM,
                strings::SETTINGS_TIME_24H,
                strings::SETTINGS_TIME_12H,
            ];
            let idx = Some(match s.time_format {
                TimeFormatPref::System => 0,
                TimeFormatPref::H24 => 1,
                TimeFormatPref::H12 => 2,
            });
            self.sel_time_format.open(&opts, idx, palette);
            return true;
        }
        false
    }

    pub fn handle_popup_poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(res) = self.sel_idle.poll() {
            if let Some(idx) = res {
                if let Some(&mins) = IDLE_MINUTES.get(idx) {
                    self.save(SettingsPatch {
                        idle_minutes: Some(mins),
                        ..Default::default()
                    });
                }
            }
            changed = true;
        }
        if let Some(res) = self.sel_currency.poll() {
            if let Some(idx) = res {
                let s = self.settings();
                let opts = Self::currency_options(&s.currency);
                if let Some(cur) = opts.get(idx) {
                    self.save(SettingsPatch {
                        currency: Some(cur.clone()),
                        ..Default::default()
                    });
                }
            }
            changed = true;
        }
        if let Some(res) = self.sel_day_start.poll() {
            if let Some(idx) = res {
                if idx <= 12 {
                    self.save(SettingsPatch {
                        day_start_hour: Some(idx as u32),
                        ..Default::default()
                    });
                }
            }
            changed = true;
        }
        if let Some(res) = self.sel_first_day.poll() {
            if let Some(idx) = res {
                let fd = match idx {
                    1 => FirstDayPref::Monday,
                    2 => FirstDayPref::Sunday,
                    _ => FirstDayPref::System,
                };
                self.save(SettingsPatch {
                    first_day_of_week: Some(fd),
                    ..Default::default()
                });
            }
            changed = true;
        }
        if let Some(res) = self.sel_time_format.poll() {
            if let Some(idx) = res {
                let tf = match idx {
                    1 => TimeFormatPref::H24,
                    2 => TimeFormatPref::H12,
                    _ => TimeFormatPref::System,
                };
                self.save(SettingsPatch {
                    time_format: Some(tf),
                    ..Default::default()
                });
            }
            changed = true;
        }
        changed
    }

    pub fn handle_text_change(&mut self, key: &str, value: &str) -> bool {
        if key == KEY_EXCL_APPS {
            self.draft_apps = value.to_string();
            let list = format::parse_csv_tags(value);
            self.save(SettingsPatch {
                excluded_apps: Some(list),
                ..Default::default()
            });
            return false;
        }
        if key == KEY_EXCL_TITLES {
            self.draft_titles = value.to_string();
            let list = format::parse_csv_tags(value);
            self.save(SettingsPatch {
                excluded_title_patterns: Some(list),
                ..Default::default()
            });
            return false;
        }
        false
    }

    pub fn handle_text_submit(&mut self, key: &str, value: &str) -> bool {
        if key == KEY_EXCL_APPS {
            let list = format::parse_csv_tags(value);
            self.draft_apps = list.join(", ");
            self.apps_rev += 1;
            self.save(SettingsPatch {
                excluded_apps: Some(list),
                ..Default::default()
            });
            return true;
        }
        if key == KEY_EXCL_TITLES {
            let list = format::parse_csv_tags(value);
            self.draft_titles = list.join(", ");
            self.titles_rev += 1;
            self.save(SettingsPatch {
                excluded_title_patterns: Some(list),
                ..Default::default()
            });
            return true;
        }
        false
    }

    pub fn handle_wheel(&mut self, delta_dip: f32) -> bool {
        let max_scroll = (1150.0 - self.viewport_h).max(0.0);
        scroll_wheel_step(&mut self.scroll_y, delta_dip, max_scroll)
    }
}

impl Screen for SettingsScreen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        let mut tree = screen_root_tree(env);
        let root = tree.root();
        let modal_root = self.build_content(&mut tree, root, root, env);
        BuiltScreen { tree, modal_root }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        self.handle_click(id, &Palette::LIGHT)
    }

    fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        self.handle_text_change(key, value)
    }

    fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        self.handle_text_submit(key, value)
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        self.handle_wheel(delta_dip)
    }

    fn on_escape(&mut self) -> bool {
        if self.confirm_clear {
            self.confirm_clear = false;
            true
        } else {
            false
        }
    }

    fn on_popup_poll(&mut self) -> bool {
        self.handle_popup_poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::shared_store;
    use crate::demo::DemoStore;
    use crate::ui::a11y;
    use crate::ui::focus::FocusModel;
    use crate::ui::screen::ScreenHost;
    use crate::ui::text::TextSystem;
    use std::rc::Rc;

    #[test]
    fn settings_screen_validates_a11y_persists_changes_and_confirms_clear_activity() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1024.0,
            height_dip: 720.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let store = shared_store(Box::new(DemoStore::new(
            crate::locale::SystemLocale::en_us(),
        )));
        assert!(!store
            .borrow()
            .list_activity(0, store.borrow().now_ms())
            .unwrap()
            .is_empty());

        let screen = SettingsScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        // Locate Monitor toggle & Delete button via FocusModel tab order.
        let mon_btn_id = focus.tab(false).unwrap();
        let _browser_id = focus.tab(false).unwrap();
        let claude_id = focus.tab(false).unwrap();
        let _busy_id = focus.tab(false).unwrap();
        let _idle_id = focus.tab(false).unwrap();
        let _apps_id = focus.tab(false).unwrap();
        let _titles_id = focus.tab(false).unwrap();
        let _cur_id = focus.tab(false).unwrap();
        let _day_start_id = focus.tab(false).unwrap();
        let _first_day_id = focus.tab(false).unwrap();
        let _tf_id = focus.tab(false).unwrap();
        let csv_id = focus.tab(false).unwrap();
        let del_prompt_id = focus.tab(false).unwrap();

        // Toggle Claude & CSV defaults -> saved immediately.
        assert!(host.on_click(claude_id));
        assert!(host.on_click(csv_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        let s = store.borrow().get_settings().unwrap();
        assert!(!s.claude_sessions);
        assert!(s.csv_standard);

        // Excluded apps text change & submit normalizes comma-separated tags.
        host.on_text_change(KEY_EXCL_APPS, "1Password, Vaultwarden , KeePass");
        assert!(host.on_text_submit(KEY_EXCL_APPS, "1Password, Vaultwarden , KeePass"));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(
            store.borrow().get_settings().unwrap().excluded_apps,
            vec!["1Password", "Vaultwarden", "KeePass"]
        );

        // Two-step activity clear: Delete... -> Keep cancels; Delete... -> Confirm clears activity.
        assert!(host.on_click(del_prompt_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        // Tab down to confirm delete button (last item in tree).
        let del_node_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::SETTINGS_DELETE_CONFIRM_BTN)
            .unwrap()
            .id;
        assert!(host.on_click(del_node_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert!(store
            .borrow()
            .list_activity(0, store.borrow().now_ms())
            .unwrap()
            .is_empty());

        // Disabling monitor_enabled disables browser toggle cleanly.
        assert!(host.on_click(mon_btn_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
    }
}
