//! First-run Welcome screen explaining each local activity source before any capture starts.

use taffy::prelude::*;

use crate::db::{SettingsPatch, SharedStore};
use crate::ui::screen::{BuiltScreen, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::tree::{style, Leaf, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonVariant};
use crate::ui::widgets::toggle::{self, ToggleKind, ToggleProps};
use crate::ui::widgets::{add_label, glyph, Interact};

use super::{build_card, build_hairline, build_scroll_page, screen_root_tree, scroll_wheel_step};

pub struct WelcomeScreen {
    store: SharedStore,
    monitor_enabled: bool,
    browser_tabs: bool,
    claude_sessions: bool,
    scroll_y: f32,
    viewport_h: f32,
    id_monitor: Option<WidgetId>,
    id_browser: Option<WidgetId>,
    id_claude: Option<WidgetId>,
    id_start: Option<WidgetId>,
    on_complete: Option<Box<dyn FnMut()>>,
}

impl WelcomeScreen {
    pub fn new(store: SharedStore) -> Self {
        let s = store
            .borrow()
            .get_settings()
            .unwrap_or_else(|_| crate::db::Settings::defaults_for(store.borrow().system_locale()));
        Self {
            store,
            monitor_enabled: s.monitor_enabled,
            browser_tabs: s.browser_tabs,
            claude_sessions: s.claude_sessions,
            scroll_y: 0.0,
            viewport_h: 720.0,
            id_monitor: None,
            id_browser: None,
            id_claude: None,
            id_start: None,
            on_complete: None,
        }
    }

    #[cfg(test)]
    pub fn with_on_complete(mut self, f: impl FnMut() + 'static) -> Self {
        self.on_complete = Some(Box::new(f));
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn build_source_row(
        tree: &mut Tree,
        parent: Node,
        icon: char,
        title: &str,
        hint: &str,
        checked: bool,
        disabled: bool,
        slot: Option<WidgetId>,
        env: &ScreenEnv<'_>,
    ) -> WidgetId {
        let palette = env.palette;
        let interact = Interact::from_state(slot, env.hover, env.pressed, disabled);
        let row = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                padding: style::pad(14.0, 12.0, 14.0, 12.0),
                gap: style::gap(14.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );

        let left = tree.add_container(
            row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::FlexStart),
                gap: style::gap(12.0),
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let icon_box = tree.add_container(
            left,
            Style {
                padding: style::pad(0.0, 2.0, 0.0, 0.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        tree.add_leaf(
            icon_box,
            Style::default(),
            Leaf::Icon {
                glyph: icon,
                size: 18.0,
                color: if disabled {
                    palette.disabled_text
                } else {
                    palette.accent
                },
            },
        );

        let text_col = tree.add_container(
            left,
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
            13.5,
            Weight::SemiBold,
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

        let (_sw, sw_id) = toggle::build(
            tree,
            row,
            &ToggleProps {
                kind: ToggleKind::Switch,
                label: "",
                aria_label: Some(title),
                checked,
                disabled,
                interact,
            },
            palette,
        );
        sw_id
    }
}

impl WelcomeScreen {
    pub fn build_content(&mut self, tree: &mut Tree, root: Node, env: &ScreenEnv<'_>) {
        self.viewport_h = env.height_dip;
        let p = env.palette;

        let (_vp, col) = build_scroll_page(tree, root, 620.0, 28.0, 0.0, self.scroll_y);
        let card = build_card(tree, col, 28.0, 16.0, p);

        add_label(
            tree,
            card,
            strings::APP_NAME,
            13.0,
            Weight::SemiBold,
            Align::Left,
            p.accent,
        );
        tree.add_leaf(
            card,
            Style::default(),
            Leaf::StyledText {
                text: strings::WELCOME_HEADLINE.into(),
                size: 22.0,
                weight: Weight::SemiBold,
                align: Align::Left,
                wrap: Wrap::Word,
                color: p.text,
            },
        );
        tree.add_leaf(
            card,
            Style::default(),
            Leaf::StyledText {
                text: strings::WELCOME_LEAD.into(),
                size: 13.0,
                weight: Weight::Regular,
                align: Align::Left,
                wrap: Wrap::Word,
                color: p.muted,
            },
        );

        let sources = tree.add_container(
            card,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            sources,
            PaintStyle {
                bg: Some(p.backdrop),
                border_color: Some(p.card_border),
                border_width: p.border_width,
                radius: 8.0,
                clip: true,
            },
        );

        self.id_monitor = Some(Self::build_source_row(
            tree,
            sources,
            glyph::CALENDAR,
            strings::WELCOME_SRC_APPS_TITLE,
            strings::WELCOME_SRC_APPS_HINT,
            self.monitor_enabled,
            false,
            self.id_monitor,
            env,
        ));
        build_hairline(tree, sources, p);

        let br_disabled = !self.monitor_enabled;
        self.id_browser = Some(Self::build_source_row(
            tree,
            sources,
            glyph::ACTIVITY,
            strings::WELCOME_SRC_BROWSER_TITLE,
            strings::WELCOME_SRC_BROWSER_HINT,
            self.browser_tabs && !br_disabled,
            br_disabled,
            self.id_browser,
            env,
        ));
        build_hairline(tree, sources, p);

        self.id_claude = Some(Self::build_source_row(
            tree,
            sources,
            glyph::PROJECTS,
            strings::WELCOME_SRC_CLAUDE_TITLE,
            strings::WELCOME_SRC_CLAUDE_HINT,
            self.claude_sessions,
            false,
            self.id_claude,
            env,
        ));

        tree.add_leaf(
            card,
            Style::default(),
            Leaf::StyledText {
                text: strings::WELCOME_PRIVACY.into(),
                size: 12.0,
                weight: Weight::Regular,
                align: Align::Left,
                wrap: Wrap::Word,
                color: p.muted,
            },
        );

        let foot = tree.add_container(
            card,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                justify_content: Some(JustifyContent::FlexEnd),
                padding: style::pad(0.0, 4.0, 0.0, 0.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let mut start_props = ButtonProps::new(strings::WELCOME_START_BTN);
        start_props.variant = ButtonVariant::Primary;
        start_props.interact = Interact::from_state(self.id_start, env.hover, env.pressed, false);
        let (_, start_id) = button::build(tree, foot, &start_props, p);
        self.id_start = Some(start_id);
    }
}

impl Screen for WelcomeScreen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        let mut tree = screen_root_tree(env);
        let root = tree.root();
        self.build_content(&mut tree, root, env);
        BuiltScreen {
            tree,
            modal_root: None,
        }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        if Some(id) == self.id_monitor {
            self.monitor_enabled = !self.monitor_enabled;
            return true;
        }
        if Some(id) == self.id_browser && self.monitor_enabled {
            self.browser_tabs = !self.browser_tabs;
            return true;
        }
        if Some(id) == self.id_claude {
            self.claude_sessions = !self.claude_sessions;
            return true;
        }
        if Some(id) == self.id_start {
            let patch = SettingsPatch {
                monitor_enabled: Some(self.monitor_enabled),
                browser_tabs: Some(self.browser_tabs),
                claude_sessions: Some(self.claude_sessions),
                onboarded: Some(true),
                ..Default::default()
            };
            let _ = self.store.borrow_mut().save_settings(patch);
            if let Some(cb) = self.on_complete.as_mut() {
                cb();
            }
            return true;
        }
        false
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        let max_scroll = (600.0 - self.viewport_h).max(0.0);
        scroll_wheel_step(&mut self.scroll_y, delta_dip, max_scroll)
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
    use crate::ui::theme::Palette;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn welcome_validates_a11y_disables_browser_when_monitor_off_and_completes() {
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
        let store = shared_store(Box::new(DemoStore::fresh(
            crate::locale::SystemLocale::en_us(),
        )));
        assert!(!store.borrow().get_settings().unwrap().onboarded);

        let completed = Rc::new(Cell::new(false));
        let completed_flag = Rc::clone(&completed);
        let screen = WelcomeScreen::new(Rc::clone(&store)).with_on_complete(move || {
            completed_flag.set(true);
        });

        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        // Tab sequentially through all 4 controls: monitor, browser, claude, start button.
        let mon_id = focus.tab(false).unwrap();
        let br_id = focus.tab(false).unwrap();
        let _cl_id = focus.tab(false).unwrap();
        let start_id = focus.tab(false).unwrap();
        assert_eq!(focus.tab(false), Some(mon_id));

        // Turn off monitor -> browser switch becomes disabled & skipped by Tab.
        assert!(host.on_click(mon_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(!host.on_click(br_id));

        // Activate "Start using Focus" -> saves settings and fires callback.
        assert!(host.on_click(start_id));
        assert!(completed.get());
        let saved = store.borrow().get_settings().unwrap();
        assert!(saved.onboarded);
        assert!(!saved.monitor_enabled);
    }
}
