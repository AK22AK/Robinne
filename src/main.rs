use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::Disableable;
use gpui_component::{h_flex, ActiveTheme, IconName, Sizable, TitleBar};
use gpui_component_assets::Assets;
use gpui_platform::application;
use robinne::ai::{test_connection, AiProviderProtocol, AiSettings};
use robinne::app_shortcuts::{
    app_shortcut_entries, main_panel_shortcuts, quick_add_overlay_keystroke, search_keystroke,
    settings_keystroke,
};
use robinne::config::{
    format_shortcut_for_display, keystroke_matches_shortcut, preview_shortcut_from_keystroke,
    preview_shortcut_from_modifiers, shortcut_from_keystroke, validate_shortcut_config,
    ShortcutConfig,
};
use robinne::data_management::app_data_dir;
use robinne::platform::{
    app_shortcut_scope_description, app_shortcuts_intro, build_app_menus, delete_secret,
    load_secret, prewarm_file_dialog, record_ai_usage, save_secret, secrets_file_path,
    AiUsageEventKind, SecretSource,
};
use robinne::settings::{
    load_app_settings, save_app_settings, settings_file_path, AppSettings, QuickAddDefaultMode,
    ShortcutSettings, StartupPanelPreference,
};
use robinne::store::{create_store, Store};
use robinne::ui::ai_panel::AiPanel;
use robinne::ui::dashboard::{Dashboard, DashboardAction};
use robinne::ui::data_management::DataManagementPanel;
use robinne::ui::floating_window::{
    quick_add_window_size, should_hide_app_after_global_quick_add_launch,
    should_hide_app_after_quick_add_close, InputMode, QuickAddDestination, QuickAddPresentation,
    QuickAddSessionController, QuickAddSessionStatus, QuickAddWindow,
};
use robinne::ui::note_panel::NotePanel;
use robinne::ui::quick_add_context::resolve_quick_add_mode;
use robinne::ui::search::SearchPanel;
use robinne::ui::sidebar::{main_sidebar_layout_mode, main_sidebar_width, Panel, Sidebar};
use robinne::ui::style::ClaudeLikeColors;
use robinne::ui::task_panel::TaskPanel;
use robinne::ui::timeline::Timeline;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

const SETTINGS_NAV_BREAKPOINT: Pixels = px(600.0);
const SETTINGS_SIDEBAR_NAV_WIDTH: Pixels = px(180.0);
const QUICK_CAPTURE_DEBOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

actions!(
    app_menu,
    [OpenQuickAddOverlay, OpenSearch, OpenSettings, QuitApp]
);

#[derive(Clone)]
struct MainWindowController {
    handle: Option<AnyWindowHandle>,
    window_id: Option<WindowId>,
    current_panel: Panel,
    is_active: bool,
}

impl Default for MainWindowController {
    fn default() -> Self {
        Self {
            handle: None,
            window_id: None,
            current_panel: Panel::Dashboard,
            is_active: false,
        }
    }
}

impl MainWindowController {
    fn track(&mut self, handle: AnyWindowHandle, panel: Panel) {
        self.handle = Some(handle);
        self.window_id = Some(handle.window_id());
        self.current_panel = panel;
        self.is_active = true;
    }

    fn clear_handle(&mut self) {
        self.handle = None;
        self.window_id = None;
        self.is_active = false;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    General,
    AI,
    Shortcuts,
    DataSync,
    About,
}

impl SettingsSection {
    fn label(self) -> &'static str {
        match self {
            Self::General => "通用",
            Self::AI => "AI",
            Self::Shortcuts => "快捷键",
            Self::DataSync => "数据与同步",
            Self::About => "关于",
        }
    }

    fn all() -> [Self; 5] {
        [
            Self::General,
            Self::AI,
            Self::Shortcuts,
            Self::DataSync,
            Self::About,
        ]
    }
}

#[derive(Clone, Copy)]
enum GlobalShortcutAction {
    QuickCapture,
}

#[derive(Clone, Copy)]
struct GlobalHotkeyBindings {
    quick_capture: global_hotkey::hotkey::HotKey,
}

impl GlobalHotkeyBindings {
    fn all(self) -> [global_hotkey::hotkey::HotKey; 1] {
        [self.quick_capture]
    }

    fn action_for_id(self, id: u32) -> Option<GlobalShortcutAction> {
        if id == self.quick_capture.id() {
            Some(GlobalShortcutAction::QuickCapture)
        } else {
            None
        }
    }
}

struct GlobalHotkeyController {
    manager: Option<GlobalHotKeyManager>,
    bindings: Option<GlobalHotkeyBindings>,
}

impl GlobalHotkeyController {
    fn new() -> Self {
        match GlobalHotKeyManager::new() {
            Ok(manager) => Self {
                manager: Some(manager),
                bindings: None,
            },
            Err(err) => {
                eprintln!("[Global Hotkey] Initialization failed: {}", err);
                Self {
                    manager: None,
                    bindings: None,
                }
            }
        }
    }

    fn apply_shortcuts(&mut self, config: &ShortcutConfig) -> Result<(), String> {
        let quick_capture = config
            .quick_capture_hotkey()
            .map_err(|err| format!("解析快捷输入失败: {}", err))?;
        let manager = self
            .manager
            .as_ref()
            .ok_or_else(|| "全局快捷键管理器初始化失败".to_string())?;

        if let Some(bindings) = self.bindings.take() {
            for hotkey in bindings.all() {
                if let Err(err) = manager.unregister(hotkey) {
                    eprintln!(
                        "[Global Hotkey] Failed to unregister {}: {}",
                        hotkey.id(),
                        err
                    );
                }
            }
        }

        let bindings = GlobalHotkeyBindings { quick_capture };

        manager
            .register(bindings.quick_capture)
            .map_err(|err| format!("注册快捷键 `{}` 失败: {}", config.quick_capture, err))?;
        eprintln!("[Global Hotkey] Registered {}", config.quick_capture);

        self.bindings = Some(bindings);
        Ok(())
    }

    fn action_for_id(&self, id: u32) -> Option<GlobalShortcutAction> {
        self.bindings
            .and_then(|bindings| bindings.action_for_id(id))
    }
}

fn load_settings_or_default() -> AppSettings {
    load_app_settings().unwrap_or_else(|err| {
        eprintln!("[Settings] Failed to load settings: {}", err);
        AppSettings::default()
    })
}

fn startup_panel_from_settings() -> Panel {
    match load_settings_or_default().general.startup_panel {
        StartupPanelPreference::Dashboard => Panel::Dashboard,
        StartupPanelPreference::Tasks => Panel::Tasks,
        StartupPanelPreference::Records => Panel::Records,
        StartupPanelPreference::Timeline => Panel::Timeline,
    }
}

fn quick_add_default_mode_from_settings() -> InputMode {
    match load_settings_or_default().general.quick_add_default_mode {
        QuickAddDefaultMode::Task => InputMode::Task,
        QuickAddDefaultMode::Record => InputMode::Record,
    }
}

fn current_platform_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS"
    }

    #[cfg(target_os = "windows")]
    {
        "Windows"
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        "Unknown"
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsLayoutMode {
    Sidebar,
    TopTabs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShortcutField {
    QuickCapture,
    OpenMain,
    OpenTasks,
    OpenRecords,
}

impl ShortcutField {
    fn label(self) -> &'static str {
        match self {
            Self::QuickCapture => "快捷输入",
            Self::OpenMain => "看板面板",
            Self::OpenTasks => "任务面板",
            Self::OpenRecords => "记录面板",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::QuickCapture => "quick-capture",
            Self::OpenMain => "open-main",
            Self::OpenTasks => "open-tasks",
            Self::OpenRecords => "open-records",
        }
    }

    fn value(self, config: &ShortcutConfig) -> &str {
        match self {
            Self::QuickCapture => &config.quick_capture,
            Self::OpenMain => &config.open_main,
            Self::OpenTasks => &config.open_tasks,
            Self::OpenRecords => &config.open_records,
        }
    }

    fn set_value(self, config: &mut ShortcutConfig, value: String) {
        match self {
            Self::QuickCapture => config.quick_capture = value,
            Self::OpenMain => config.open_main = value,
            Self::OpenTasks => config.open_tasks = value,
            Self::OpenRecords => config.open_records = value,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShortcutCapturePhase {
    Waiting,
    Success,
    Failure,
}

#[derive(Clone)]
struct ShortcutCaptureState {
    field: ShortcutField,
    phase: ShortcutCapturePhase,
    preview_display: Option<String>,
    held_keys: BTreeSet<String>,
    held_modifiers: Modifiers,
    retry_ready: bool,
    message: Option<String>,
    serial: u64,
}

fn capture_held_keys_for_keystroke(key: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if !matches!(
        key,
        "" | "control" | "alt" | "shift" | "platform" | "command" | "cmd" | "super" | "win" | "fn"
    ) {
        keys.insert(key.to_string());
    }
    keys
}

fn install_app_shortcuts_and_menus(
    cx: &mut App,
    main_window: Rc<RefCell<MainWindowController>>,
    store: Store,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
) {
    cx.bind_keys([
        KeyBinding::new(quick_add_overlay_keystroke(), OpenQuickAddOverlay, None),
        KeyBinding::new(search_keystroke(), OpenSearch, None),
        KeyBinding::new(settings_keystroke(), OpenSettings, None),
    ]);

    let main_window_for_quick_add = main_window.clone();
    let store_for_quick_add = store.clone();
    let quick_add_for_quick_add = quick_add_session.clone();
    let global_hotkeys_for_quick_add = global_hotkeys.clone();
    cx.on_action(move |_: &OpenQuickAddOverlay, cx| {
        let main_window = main_window_for_quick_add.clone();
        let store = store_for_quick_add.clone();
        let quick_add_session = quick_add_for_quick_add.clone();
        let global_hotkeys = global_hotkeys_for_quick_add.clone();
        cx.defer(move |cx| {
            open_or_focus_quick_add_overlay(
                cx,
                store,
                main_window,
                quick_add_session,
                global_hotkeys,
            );
        });
    });

    let main_window_for_search = main_window.clone();
    let store_for_search = store.clone();
    let quick_add_for_search = quick_add_session.clone();
    let global_hotkeys_for_search = global_hotkeys.clone();
    cx.on_action(move |_: &OpenSearch, cx| {
        let main_window = main_window_for_search.clone();
        let store = store_for_search.clone();
        let quick_add_session = quick_add_for_search.clone();
        let global_hotkeys = global_hotkeys_for_search.clone();
        // Defer to avoid updating the active window while the action is being dispatched from it.
        cx.defer(move |cx| {
            ensure_main_window(
                cx,
                &main_window,
                &store,
                &quick_add_session,
                &global_hotkeys,
                Some(Panel::Search),
            );
        });
    });

    let main_window_for_settings = main_window.clone();
    let store_for_settings = store.clone();
    let quick_add_for_settings = quick_add_session.clone();
    let global_hotkeys_for_settings = global_hotkeys.clone();
    cx.on_action(move |_: &OpenSettings, cx| {
        let main_window = main_window_for_settings.clone();
        let store = store_for_settings.clone();
        let quick_add_session = quick_add_for_settings.clone();
        let global_hotkeys = global_hotkeys_for_settings.clone();
        // Defer to avoid updating the active window while the action is being dispatched from it.
        cx.defer(move |cx| {
            ensure_main_window(
                cx,
                &main_window,
                &store,
                &quick_add_session,
                &global_hotkeys,
                Some(Panel::Settings),
            );
        });
    });

    cx.on_action(|_: &QuitApp, cx| cx.quit());

    cx.set_menus(build_app_menus(
        OpenQuickAddOverlay,
        OpenSearch,
        OpenSettings,
        QuitApp,
    ));
}

fn main() {
    let app = application().with_assets(Assets);

    let (store, mut runtime) = create_store();
    let settings = load_settings_or_default();
    let shortcuts = ShortcutConfig::from(&settings.shortcuts);

    let main_window = Rc::new(RefCell::new(MainWindowController::default()));
    main_window.borrow_mut().current_panel = startup_panel_from_settings();
    let quick_add_session = Rc::new(RefCell::new(QuickAddSessionController::default()));
    let global_hotkeys = Rc::new(RefCell::new(GlobalHotkeyController::new()));

    let main_window_for_reopen = main_window.clone();
    let store_for_reopen = store.clone();
    let quick_add_for_reopen = quick_add_session.clone();
    let global_hotkeys_for_reopen = global_hotkeys.clone();
    app.on_reopen(move |cx| {
        ensure_main_window(
            cx,
            &main_window_for_reopen,
            &store_for_reopen,
            &quick_add_for_reopen,
            &global_hotkeys_for_reopen,
            None,
        );
    });

    let main_window_for_run = main_window.clone();
    let quick_add_for_run = quick_add_session.clone();
    let store_for_run = store.clone();
    let global_hotkeys_for_run = global_hotkeys.clone();
    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Light, None, cx);
        install_app_shortcuts_and_menus(
            cx,
            main_window_for_run.clone(),
            store_for_run.clone(),
            quick_add_for_run.clone(),
            global_hotkeys_for_run.clone(),
        );
        let main_window_for_closed = main_window_for_run.clone();
        cx.on_window_closed(move |cx| {
            sync_main_window_controller(cx, &main_window_for_closed);
        })
        .detach();

        cx.spawn(|_cx: &mut AsyncApp| async move {
            let data_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("robinne");
            std::fs::create_dir_all(&data_dir).ok();

            let db_path = data_dir.join("data.db");
            runtime.run(db_path).await;
        })
        .detach();

        cx.defer(|_cx| {
            prewarm_file_dialog();
        });

        if let Err(err) = global_hotkeys_for_run
            .borrow_mut()
            .apply_shortcuts(&shortcuts)
        {
            eprintln!("[Global Hotkey] {}", err);
        }

        let store_for_hotkey = store_for_run.clone();
        let main_window_for_hotkey = main_window_for_run.clone();
        let quick_add_for_hotkey = quick_add_for_run.clone();
        let global_hotkeys_for_events = global_hotkeys_for_run.clone();
        let global_hotkeys_for_actions = global_hotkeys_for_run.clone();

        cx.spawn(async move |cx| {
            let receiver = GlobalHotKeyEvent::receiver();

            loop {
                if let Ok(event) = receiver.try_recv() {
                    if event.state == HotKeyState::Released {
                        let action = global_hotkeys_for_events.borrow().action_for_id(event.id);
                        if let Some(action) = action {
                            cx.update(|cx| match action {
                                GlobalShortcutAction::QuickCapture => handle_quick_capture_hotkey(
                                    cx,
                                    store_for_hotkey.clone(),
                                    main_window_for_hotkey.clone(),
                                    quick_add_for_hotkey.clone(),
                                    global_hotkeys_for_actions.clone(),
                                ),
                            });
                        }
                    }
                }

                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    });
}

fn handle_quick_capture_hotkey(
    cx: &mut App,
    store: Store,
    main_window: Rc<RefCell<MainWindowController>>,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
) {
    let now = std::time::Instant::now();
    {
        let mut session = quick_add_session.borrow_mut();
        if session
            .last_hotkey_at
            .is_some_and(|last| now.duration_since(last) < QUICK_CAPTURE_DEBOUNCE_WINDOW)
        {
            return;
        }
        session.last_hotkey_at = Some(now);
    }

    dismiss_quick_add_overlay_for_window(cx, &main_window, &quick_add_session);

    let (status, current_presentation, has_draft) = {
        let session = quick_add_session.borrow();
        (session.status, session.presentation, session.has_draft())
    };

    match status {
        QuickAddSessionStatus::Closed => {
            let activate_app = cx.active_window().is_none();
            let hide_app_on_close = should_hide_app_after_global_quick_add_launch(
                cx.active_window().is_some(),
                main_window_exists(cx, &main_window),
            );
            open_or_focus_quick_add_window_only(
                cx,
                store,
                main_window,
                quick_add_session,
                global_hotkeys,
                None,
                activate_app,
                hide_app_on_close,
            );
        }
        QuickAddSessionStatus::Dormant => {
            let activate_app = cx.active_window().is_none();
            open_or_focus_quick_add_window_only(
                cx,
                store,
                main_window,
                quick_add_session,
                global_hotkeys,
                None,
                activate_app,
                false,
            );
        }
        QuickAddSessionStatus::Visible => {
            if current_presentation == Some(QuickAddPresentation::Window) {
                if has_draft {
                    show_quick_add_hotkey_protection(cx, &main_window, &quick_add_session);
                } else {
                    close_visible_quick_add(cx, &main_window, &quick_add_session);
                }
            } else {
                open_or_focus_quick_add_window_only(
                    cx,
                    store,
                    main_window,
                    quick_add_session,
                    global_hotkeys,
                    None,
                    cx.active_window().is_none(),
                    false,
                );
            }
        }
    }
}

fn dismiss_quick_add_window_for_overlay(
    cx: &mut App,
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
) {
    let handle = {
        let session = quick_add_session.borrow();
        if session.presentation == Some(QuickAddPresentation::Window) {
            session.handle
        } else {
            None
        }
    };

    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, _| {
            window.remove_window();
        });
    }

    let mut session = quick_add_session.borrow_mut();
    if session.presentation == Some(QuickAddPresentation::Window) {
        session.handle = None;
        session.presentation = None;
        session.status = if session.has_draft() {
            QuickAddSessionStatus::Dormant
        } else {
            QuickAddSessionStatus::Closed
        };
        session.hide_app_on_close = false;
    }
}

fn dismiss_quick_add_overlay_for_window(
    cx: &mut App,
    main_window: &Rc<RefCell<MainWindowController>>,
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
) {
    let presentation = quick_add_session.borrow().presentation;
    if presentation != Some(QuickAddPresentation::Overlay) {
        return;
    }

    let _ = dismiss_quick_add_overlay(cx, main_window);

    let mut session = quick_add_session.borrow_mut();
    if session.presentation == Some(QuickAddPresentation::Overlay) {
        session.presentation = None;
        session.status = if session.has_draft() {
            QuickAddSessionStatus::Dormant
        } else {
            QuickAddSessionStatus::Closed
        };
        session.hide_app_on_close = false;
    }
}

fn prime_quick_add_session(
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
    preferred_mode: Option<InputMode>,
    hide_app_on_close: bool,
) -> u64 {
    let mut session = quick_add_session.borrow_mut();
    session.mode = preferred_mode.unwrap_or_else(quick_add_default_mode_from_settings);
    let request_serial = session.mark_visible(QuickAddPresentation::Window);
    session.handle = None;
    session.hide_app_on_close = hide_app_on_close;
    request_serial
}

fn destroy_inactive_main_window_if_needed(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
) {
    let Some(handle) = resolve_main_window_handle(cx, controller) else {
        return;
    };

    if controller.borrow().is_active {
        return;
    }

    let _ = handle.update(cx, |_, window, _| {
        window.remove_window();
    });
    controller.borrow_mut().clear_handle();
}

fn focus_visible_quick_add(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
) -> bool {
    let presentation = quick_add_session.borrow().presentation;
    match presentation {
        Some(QuickAddPresentation::Window) => {
            let handle = quick_add_session.borrow().handle;
            let Some(handle) = handle else {
                return false;
            };

            handle
                .update(cx, |_, window, _| {
                    window.activate_window();
                })
                .is_ok()
        }
        Some(QuickAddPresentation::Overlay) => focus_quick_add_overlay(cx, controller),
        None => false,
    }
}

fn open_or_focus_quick_add_window_only(
    cx: &mut App,
    store: Store,
    main_window: Rc<RefCell<MainWindowController>>,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
    preferred_mode: Option<InputMode>,
    activate_app: bool,
    hide_app_on_close: bool,
) {
    if quick_add_session.borrow().presentation == Some(QuickAddPresentation::Window)
        && focus_visible_quick_add(cx, &main_window, &quick_add_session)
    {
        return;
    }

    open_quick_add_window(
        cx,
        store,
        main_window,
        quick_add_session,
        global_hotkeys,
        activate_app,
        hide_app_on_close,
        preferred_mode,
    );
}

fn open_or_focus_quick_add_overlay(
    cx: &mut App,
    store: Store,
    main_window: Rc<RefCell<MainWindowController>>,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
) {
    let preferred_mode = {
        let panel = main_window.borrow().current_panel;
        let last_mode = quick_add_session.borrow().mode;
        resolve_quick_add_mode(panel, last_mode)
    };

    ensure_main_window(
        cx,
        &main_window,
        &store,
        &quick_add_session,
        &global_hotkeys,
        None,
    );
    dismiss_quick_add_window_for_overlay(cx, &quick_add_session);
    let _ = show_quick_add_overlay(cx, &main_window, Some(preferred_mode));
}

fn open_quick_add_window(
    cx: &mut App,
    store: Store,
    main_window: Rc<RefCell<MainWindowController>>,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
    activate_app: bool,
    hide_app_on_close: bool,
    preferred_mode: Option<InputMode>,
) {
    if activate_app && main_window_exists(cx, &main_window) {
        destroy_inactive_main_window_if_needed(cx, &main_window);
    }

    let request_serial =
        prime_quick_add_session(&quick_add_session, preferred_mode, hide_app_on_close);

    // Showing a floating quick-add window from a background app still requires app activation.
    // Keep this independent from whether the app should be hidden again on close.
    if activate_app {
        cx.activate(true);
    }

    let window_size = quick_add_window_size();
    let window_bounds = WindowBounds::Windowed(Bounds::centered(None, window_size, cx));

    let open_destination: Arc<dyn Fn(QuickAddDestination, &mut App)> = {
        let store = store.clone();
        let main_window = main_window.clone();
        let quick_add_session = quick_add_session.clone();
        let global_hotkeys = global_hotkeys.clone();
        Arc::new(move |destination, cx| match destination {
            QuickAddDestination::Main => {
                ensure_main_window(
                    cx,
                    &main_window,
                    &store,
                    &quick_add_session,
                    &global_hotkeys,
                    None,
                );
            }
            QuickAddDestination::Tasks => {
                ensure_main_window(
                    cx,
                    &main_window,
                    &store,
                    &quick_add_session,
                    &global_hotkeys,
                    Some(Panel::Tasks),
                );
            }
            QuickAddDestination::Records => {
                ensure_main_window(
                    cx,
                    &main_window,
                    &store,
                    &quick_add_session,
                    &global_hotkeys,
                    Some(Panel::Records),
                );
            }
        })
    };

    let session_for_window = quick_add_session.clone();
    let open_destination_for_window = open_destination.clone();
    let store_for_window = store.clone();

    match cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            kind: WindowKind::Floating,
            is_resizable: false,
            ..Default::default()
        },
        move |window, cx| {
            let session = session_for_window.clone();
            let open_destination = open_destination_for_window.clone();
            let store = store_for_window.clone();
            let view = cx.new(|cx| {
                let mut view =
                    QuickAddWindow::new_window(store, session, open_destination, window, cx);
                view.hide_app_on_close = hide_app_on_close;
                view
            });
            cx.new(|cx| gpui_component::Root::new(view, window, cx).bg(cx.theme().background))
        },
    ) {
        Ok(handle) => {
            let handle: AnyWindowHandle = handle.into();
            let keep_window = {
                let mut session = quick_add_session.borrow_mut();
                let keep_window = session.request_serial == request_serial
                    && session.presentation == Some(QuickAddPresentation::Window);
                if keep_window {
                    session.handle = Some(handle);
                }
                keep_window
            };

            if !keep_window {
                let _ = handle.update(cx, |_, window, _| {
                    window.remove_window();
                });
                return;
            }

            // When the window is opened from a title-bar click, macOS can briefly
            // return focus to the main window after the click completes.
            // Re-activating the quick add window on the next turn keeps it visible.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    window.activate_window();
                });
            });
        }
        Err(err) => {
            let should_clear = {
                let session = quick_add_session.borrow();
                session.request_serial == request_serial
                    && session.presentation == Some(QuickAddPresentation::Window)
            };
            if should_clear {
                quick_add_session.borrow_mut().clear();
            }
            eprintln!("[QuickAdd] Failed to open window: {}", err);
        }
    }
}

fn close_visible_quick_add(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
) {
    let (presentation, handle, hide_app_on_close) = {
        let session = quick_add_session.borrow();
        (
            session.presentation,
            session.handle,
            session.hide_app_on_close,
        )
    };

    match presentation {
        Some(QuickAddPresentation::Window) => {
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, _| {
                    window.remove_window();
                });
            }
        }
        Some(QuickAddPresentation::Overlay) => {
            let _ = dismiss_quick_add_overlay(cx, controller);
        }
        None => {}
    }

    let should_hide = {
        let mut session = quick_add_session.borrow_mut();
        let should_hide = should_hide_app_after_quick_add_close(
            hide_app_on_close && !session.has_draft(),
            live_window_handles(cx).len(),
        );
        session.clear();
        should_hide
    };

    if should_hide {
        cx.hide();
    }
}

fn show_quick_add_hotkey_protection(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
) {
    let presentation = quick_add_session.borrow().presentation;
    match presentation {
        Some(QuickAddPresentation::Window) => {
            let handle = quick_add_session.borrow().handle;
            if let Some(handle) = handle {
                let _ = handle.update(cx, |root_view, window, cx| {
                    if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
                        root.update(cx, |root, cx| {
                            if let Ok(view) = root.view().clone().downcast::<QuickAddWindow>() {
                                view.update(cx, |view, cx| view.show_hotkey_protection(cx));
                            }
                        });
                    }
                    window.activate_window();
                });
            }
        }
        Some(QuickAddPresentation::Overlay) => {
            let _ = show_overlay_hotkey_protection(cx, controller);
        }
        None => {}
    }
}

fn live_window_handles(cx: &App) -> Vec<AnyWindowHandle> {
    cx.window_stack().unwrap_or_else(|| cx.windows())
}

fn main_window_exists(cx: &mut App, controller: &Rc<RefCell<MainWindowController>>) -> bool {
    sync_main_window_controller(cx, controller);
    if controller.borrow().handle.is_some() {
        return true;
    }

    live_window_handles(cx)
        .into_iter()
        .any(|handle| is_main_window_handle(cx, handle))
}

fn sync_main_window_controller(cx: &mut App, controller: &Rc<RefCell<MainWindowController>>) {
    let tracked_window_id = controller.borrow().window_id;
    let Some(tracked_window_id) = tracked_window_id else {
        return;
    };

    if let Some(handle) = live_window_handles(cx)
        .into_iter()
        .find(|handle| handle.window_id() == tracked_window_id)
    {
        controller.borrow_mut().handle = Some(handle);
        return;
    }

    controller.borrow_mut().clear_handle();
}

fn is_main_window_handle(cx: &mut App, handle: AnyWindowHandle) -> bool {
    handle
        .update(cx, |root_view, _window, cx| {
            if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
                root.update(cx, |root, _cx| {
                    root.view().clone().downcast::<MainView>().is_ok()
                })
            } else {
                false
            }
        })
        .unwrap_or(false)
}

fn resolve_main_window_handle(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
) -> Option<AnyWindowHandle> {
    sync_main_window_controller(cx, controller);

    if let Some(handle) = controller.borrow().handle {
        return Some(handle);
    }

    let current_panel = controller.borrow().current_panel;
    for handle in live_window_handles(cx) {
        if is_main_window_handle(cx, handle) {
            controller.borrow_mut().track(handle, current_panel);
            return Some(handle);
        }
    }

    None
}

fn show_quick_add_overlay(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
    preferred_mode: Option<InputMode>,
) -> bool {
    let Some(handle) = resolve_main_window_handle(cx, controller) else {
        return false;
    };

    handle
        .update(cx, |root_view, window, cx| {
            if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
                root.update(cx, |root, cx| {
                    if let Ok(main_view) = root.view().clone().downcast::<MainView>() {
                        main_view.update(cx, |this, cx| {
                            this.show_quick_add_overlay(preferred_mode, window, cx);
                        });
                    }
                });
            }
            window.activate_window();
        })
        .is_ok()
}

fn focus_quick_add_overlay(cx: &mut App, controller: &Rc<RefCell<MainWindowController>>) -> bool {
    let Some(handle) = resolve_main_window_handle(cx, controller) else {
        return false;
    };

    handle
        .update(cx, |root_view, window, cx| {
            if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
                root.update(cx, |root, cx| {
                    if let Ok(main_view) = root.view().clone().downcast::<MainView>() {
                        main_view.update(cx, |this, cx| this.focus_quick_add_overlay(window, cx));
                    }
                });
            }
            window.activate_window();
        })
        .is_ok()
}

fn dismiss_quick_add_overlay(cx: &mut App, controller: &Rc<RefCell<MainWindowController>>) -> bool {
    let Some(handle) = resolve_main_window_handle(cx, controller) else {
        return false;
    };

    handle
        .update(cx, |root_view, _window, cx| {
            if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
                root.update(cx, |root, cx| {
                    if let Ok(main_view) = root.view().clone().downcast::<MainView>() {
                        main_view.update(cx, |this, cx| this.dismiss_quick_add_overlay(cx));
                    }
                });
            }
        })
        .is_ok()
}

fn show_overlay_hotkey_protection(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
) -> bool {
    let Some(handle) = resolve_main_window_handle(cx, controller) else {
        return false;
    };

    handle
        .update(cx, |root_view, window, cx| {
            if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
                root.update(cx, |root, cx| {
                    if let Ok(main_view) = root.view().clone().downcast::<MainView>() {
                        main_view.update(cx, |this, cx| {
                            this.show_quick_add_hotkey_protection(window, cx);
                        });
                    }
                });
            }
            window.activate_window();
        })
        .is_ok()
}

fn switch_existing_main_window(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
    panel: Panel,
) -> bool {
    let Some(handle) = resolve_main_window_handle(cx, controller) else {
        return false;
    };

    if handle
        .update(cx, |root_view, window, cx| {
            update_main_window(root_view, window, cx, panel)
        })
        .ok()
        == Some(true)
    {
        controller.borrow_mut().track(handle, panel);
        return true;
    }

    controller.borrow_mut().clear_handle();
    false
}

fn ensure_main_window(
    cx: &mut App,
    controller: &Rc<RefCell<MainWindowController>>,
    store: &Store,
    quick_add_session: &Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: &Rc<RefCell<GlobalHotkeyController>>,
    target_panel: Option<Panel>,
) {
    sync_main_window_controller(cx, controller);
    let desired_panel = target_panel.unwrap_or_else(|| {
        if controller.borrow().handle.is_some() {
            controller.borrow().current_panel
        } else {
            startup_panel_from_settings()
        }
    });

    if switch_existing_main_window(cx, controller, desired_panel) {
        return;
    }

    match open_main_window(
        cx,
        store.clone(),
        controller.clone(),
        quick_add_session.clone(),
        global_hotkeys.clone(),
        desired_panel,
    ) {
        Ok(handle) => {
            controller.borrow_mut().track(handle, desired_panel);
        }
        Err(err) => {
            eprintln!("[MainWindow] Failed to open main window: {}", err);
        }
    }
}

fn update_main_window(root_view: AnyView, window: &mut Window, cx: &mut App, panel: Panel) -> bool {
    let mut updated = false;
    if let Ok(root) = root_view.downcast::<gpui_component::Root>() {
        root.update(cx, |root, cx| {
            if let Ok(main_view) = root.view().clone().downcast::<MainView>() {
                updated = true;
                main_view.update(cx, |this, cx| this.switch_to_panel(panel, window, cx));
            }
        });
    }
    if updated {
        window.activate_window();
    }
    updated
}

fn open_main_window(
    cx: &mut App,
    store: Store,
    controller: Rc<RefCell<MainWindowController>>,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
    initial_panel: Panel,
) -> Result<AnyWindowHandle> {
    let window_size = size(px(1100.0), px(720.0));
    let window_bounds = WindowBounds::Windowed(Bounds::centered(None, window_size, cx));

    cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            titlebar: Some(TitleBar::title_bar_options()),
            ..Default::default()
        },
        move |window, cx| {
            let store = store.clone();
            let controller = controller.clone();
            let quick_add_session = quick_add_session.clone();
            let global_hotkeys = global_hotkeys.clone();
            let view = cx.new(|cx| {
                MainView::new(
                    store,
                    controller,
                    quick_add_session,
                    global_hotkeys,
                    initial_panel,
                    window,
                    cx,
                )
            });
            cx.new(|cx| gpui_component::Root::new(view, window, cx).bg(cx.theme().background))
        },
    )
    .map(|h| h.into())
}

pub struct MainView {
    store: Store,
    current_panel: Panel,
    current_settings_section: SettingsSection,
    app_settings: AppSettings,
    ai_api_key_present: bool,
    ai_api_key_source: Option<SecretSource>,
    ai_connection_testing: bool,
    dashboard_panel: Entity<Dashboard>,
    ai_panel: Entity<AiPanel>,
    data_management_panel: Entity<DataManagementPanel>,
    search_panel: Entity<SearchPanel>,
    task_panel: Entity<TaskPanel>,
    timeline_panel: Entity<Timeline>,
    notes_panel: Entity<NotePanel>,
    ai_base_url_input: Entity<InputState>,
    ai_model_input: Entity<InputState>,
    ai_api_key_input: Entity<InputState>,
    quick_add_overlay: Option<Entity<QuickAddWindow>>,
    shortcut_config: ShortcutConfig,
    active_shortcut_capture: Option<ShortcutCaptureState>,
    shortcut_capture_serial: u64,
    settings_notice: Option<String>,
    settings_error: Option<String>,
    global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
    window_state: Rc<RefCell<MainWindowController>>,
    quick_add_session: Rc<RefCell<QuickAddSessionController>>,
    focus_handle: FocusHandle,
    _window_activation_subscription: Subscription,
}

impl MainView {
    fn new(
        store: Store,
        window_state: Rc<RefCell<MainWindowController>>,
        quick_add_session: Rc<RefCell<QuickAddSessionController>>,
        global_hotkeys: Rc<RefCell<GlobalHotkeyController>>,
        initial_panel: Panel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store_for_panels = store.clone();
        let app_settings = load_settings_or_default();
        let shortcut_config = ShortcutConfig::from(&app_settings.shortcuts);
        let (ai_api_key_present, ai_api_key_source) = match load_secret(app_settings.ai.protocol) {
            Ok(Some(secret)) => (true, Some(secret.source)),
            Ok(None) | Err(_) => (false, None),
        };
        let dashboard_panel = cx.new(|cx| Dashboard::new(store_for_panels.clone(), window, cx));
        let ai_panel = cx.new(|cx| AiPanel::new(store_for_panels.clone(), window, cx));
        let data_management_panel =
            cx.new(|cx| DataManagementPanel::new(store_for_panels.clone(), window, cx));
        let search_panel = cx.new(|cx| SearchPanel::new(store_for_panels.clone(), window, cx));
        let task_panel = cx.new(|cx| TaskPanel::new(store_for_panels.clone(), window, cx));
        let timeline_panel = cx.new(|cx| Timeline::new(store_for_panels.clone(), window, cx));
        let notes_panel = cx.new(|cx| NotePanel::new(store_for_panels, window, cx));
        let ai_base_url_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://api.openai.com/v1"));
        let ai_model_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("例如 gpt-4.1-mini / claude-sonnet-4-5")
        });
        let ai_api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("输入 API Key，保存到本地配置")
        });
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        window_state.borrow_mut().current_panel = initial_panel;
        let window_state_for_activation = window_state.clone();
        let window_activation_subscription =
            cx.observe_window_activation(window, move |_this, window, _cx| {
                let mut controller = window_state_for_activation.borrow_mut();
                controller.is_active = window.is_window_active();
            });

        let handle = cx.entity().clone();
        dashboard_panel.update(cx, |panel, _cx| {
            panel.on_action(move |action, window, cx| {
                handle.update(cx, |this, cx| {
                    this.handle_dashboard_action(action, window, cx);
                });
            });
        });

        let mut view = Self {
            store,
            current_panel: initial_panel,
            current_settings_section: SettingsSection::General,
            app_settings,
            ai_api_key_present,
            ai_api_key_source,
            ai_connection_testing: false,
            dashboard_panel,
            ai_panel,
            data_management_panel,
            search_panel,
            task_panel,
            timeline_panel,
            notes_panel,
            ai_base_url_input,
            ai_model_input,
            ai_api_key_input,
            quick_add_overlay: None,
            shortcut_config,
            active_shortcut_capture: None,
            shortcut_capture_serial: 0,
            settings_notice: None,
            settings_error: None,
            global_hotkeys,
            window_state,
            quick_add_session,
            focus_handle,
            _window_activation_subscription: window_activation_subscription,
        };
        view.sync_ai_settings_inputs(window, cx);
        view
    }

    fn handle_dashboard_action(
        &mut self,
        action: DashboardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            DashboardAction::OpenTaskPreset(preset) => {
                self.switch_to_panel(Panel::Tasks, window, cx);
                self.task_panel.update(cx, |panel, cx| {
                    panel.apply_focus_preset(preset, window, cx);
                });
            }
            DashboardAction::OpenTimeline => {
                self.switch_to_panel(Panel::Timeline, window, cx);
            }
            DashboardAction::FilterByTag(tag) => {
                self.switch_to_panel(Panel::Timeline, window, cx);
                self.timeline_panel.update(cx, |panel, cx| {
                    panel.apply_filters(vec![tag], Vec::new(), cx);
                });
            }
            DashboardAction::FilterByPerson(person) => {
                self.switch_to_panel(Panel::Timeline, window, cx);
                self.timeline_panel.update(cx, |panel, cx| {
                    panel.apply_filters(Vec::new(), vec![person], cx);
                });
            }
        }
    }

    fn focus_active_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_add_overlay.is_some() {
            self.focus_quick_add_overlay(window, cx);
            return;
        }

        match panel {
            Panel::Search => {
                self.search_panel.update(cx, |panel, cx| {
                    panel.focus_input(window, cx);
                });
            }
            _ => {
                self.focus_handle.focus(window, cx);
            }
        }
    }

    fn quick_add_open_destination(&self) -> Arc<dyn Fn(QuickAddDestination, &mut App)> {
        let store = self.store.clone();
        let main_window = self.window_state.clone();
        let quick_add_session = self.quick_add_session.clone();
        let global_hotkeys = self.global_hotkeys.clone();
        Arc::new(move |destination, cx| match destination {
            QuickAddDestination::Main => {
                ensure_main_window(
                    cx,
                    &main_window,
                    &store,
                    &quick_add_session,
                    &global_hotkeys,
                    None,
                );
            }
            QuickAddDestination::Tasks => {
                ensure_main_window(
                    cx,
                    &main_window,
                    &store,
                    &quick_add_session,
                    &global_hotkeys,
                    Some(Panel::Tasks),
                );
            }
            QuickAddDestination::Records => {
                ensure_main_window(
                    cx,
                    &main_window,
                    &store,
                    &quick_add_session,
                    &global_hotkeys,
                    Some(Panel::Records),
                );
            }
        })
    }

    fn show_quick_add_overlay(
        &mut self,
        preferred_mode: Option<InputMode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(overlay) = self.quick_add_overlay.clone() {
            overlay.update(cx, |overlay, cx| {
                overlay.focus_input(window, cx);
            });
            let mut session = self.quick_add_session.borrow_mut();
            session.mark_visible(QuickAddPresentation::Overlay);
            session.handle = None;
            session.hide_app_on_close = false;
            window.activate_window();
            return;
        }

        {
            let mut session = self.quick_add_session.borrow_mut();
            if let Some(mode) = preferred_mode {
                session.mode = mode;
            }
            session.mark_visible(QuickAddPresentation::Overlay);
            session.handle = None;
            session.hide_app_on_close = false;
        }

        let dismiss_overlay: Arc<dyn Fn(&mut App)> = {
            let main_view = cx.entity().clone();
            Arc::new(move |cx| {
                let _ = main_view.update(cx, |this, cx| {
                    this.dismiss_quick_add_overlay(cx);
                });
            })
        };

        let store = self.store.clone();
        let session = self.quick_add_session.clone();
        let open_destination = self.quick_add_open_destination();
        let overlay = cx.new(|cx| {
            let mut view = QuickAddWindow::new_overlay(
                store,
                session,
                open_destination,
                dismiss_overlay,
                window,
                cx,
            );
            view.hide_app_on_close = false;
            view
        });
        self.quick_add_overlay = Some(overlay);
        cx.notify();
    }

    fn focus_quick_add_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(overlay) = self.quick_add_overlay.clone() {
            overlay.update(cx, |overlay, cx| {
                overlay.focus_input(window, cx);
            });
            let mut session = self.quick_add_session.borrow_mut();
            session.mark_visible(QuickAddPresentation::Overlay);
            session.handle = None;
            session.hide_app_on_close = false;
            window.activate_window();
        }
    }

    fn dismiss_quick_add_overlay(&mut self, cx: &mut Context<Self>) {
        self.quick_add_overlay = None;
        cx.notify();
    }

    fn show_quick_add_hotkey_protection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(overlay) = self.quick_add_overlay.clone() {
            overlay.update(cx, |overlay, cx| {
                overlay.show_hotkey_protection(cx);
                overlay.focus_input(window, cx);
            });
        }
    }

    fn open_quick_add_from_titlebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let main_window = self.window_state.clone();
        let quick_add_session = self.quick_add_session.clone();
        let global_hotkeys = self.global_hotkeys.clone();

        window.activate_window();
        cx.defer(move |cx| {
            open_or_focus_quick_add_overlay(
                cx,
                store,
                main_window,
                quick_add_session,
                global_hotkeys,
            );
        });
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .bg(transparent_black())
            .border_color(ClaudeLikeColors::app_background())
            .child(
                h_flex().h_full().items_center().child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0x595959))
                        .child(format!("Robinne · {}", self.current_panel.title())),
                ),
            )
            .child(
                h_flex()
                    .relative()
                    .h_full()
                    .items_center()
                    .pr(px(12.0))
                    .child(
                        Button::new("main-titlebar-quick-add")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.open_quick_add_from_titlebar(window, cx);
                            })),
                    ),
            )
    }

    pub fn switch_to_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if panel == Panel::Settings && self.current_panel != Panel::Settings {
            self.current_settings_section = SettingsSection::General;
            self.refresh_settings_state(window, cx);
        } else if panel == Panel::AI {
            self.reload_ai_panel_configuration(cx);
        } else if panel != Panel::Settings {
            self.active_shortcut_capture = None;
        }

        if self.current_panel != panel {
            eprintln!(
                "[MainView] Switching panel from {:?} to {:?}",
                self.current_panel, panel
            );
            self.current_panel = panel;
            self.window_state.borrow_mut().current_panel = panel;
            cx.notify();
        }

        self.focus_active_panel(panel, window, cx);
    }

    fn settings_content_width(window: &Window) -> Pixels {
        let viewport_width = window.viewport_size().width;
        let sidebar_width = main_sidebar_width(main_sidebar_layout_mode(viewport_width));
        std::cmp::max(viewport_width - sidebar_width, px(0.0))
    }

    fn current_settings_layout_mode(&self, window: &Window) -> SettingsLayoutMode {
        if Self::settings_content_width(window) >= SETTINGS_NAV_BREAKPOINT {
            SettingsLayoutMode::Sidebar
        } else {
            SettingsLayoutMode::TopTabs
        }
    }

    fn refresh_settings_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.app_settings = load_settings_or_default();
        self.shortcut_config = ShortcutConfig::from(&self.app_settings.shortcuts);
        self.refresh_ai_api_key_presence(self.app_settings.ai.protocol);
        self.sync_ai_settings_inputs(window, cx);
        self.active_shortcut_capture = None;
        self.settings_notice = None;
        self.settings_error = None;
        self.data_management_panel
            .update(cx, |panel, cx| panel.reload_settings(window, cx));
        self.ai_panel
            .update(cx, |panel, cx| panel.reload_configuration(cx));
    }

    fn persist_settings(&mut self, message: &'static str, cx: &mut Context<Self>) {
        match save_app_settings(&self.app_settings) {
            Ok(_) => {
                self.settings_error = None;
                self.settings_notice = Some(message.to_string());
            }
            Err(err) => {
                self.settings_notice = None;
                self.settings_error = Some(err);
            }
        }
        cx.notify();
    }

    fn sync_ai_settings_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base_url = self.app_settings.ai.normalized_base_url();
        let model = self.app_settings.ai.model.clone();
        self.ai_base_url_input.update(cx, |input, cx| {
            input.set_value(&base_url, window, cx);
        });
        self.ai_model_input.update(cx, |input, cx| {
            input.set_value(&model, window, cx);
        });
        self.sync_ai_api_key_input(window, cx);
    }

    fn sync_ai_api_key_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = if self.ai_api_key_present {
            "********（已有可用 Key，输入新值会保存到本地配置）"
        } else {
            "输入 API Key，保存到本地配置"
        };
        self.ai_api_key_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
            input.set_masked(true, window, cx);
        });
    }

    fn refresh_ai_api_key_presence(&mut self, protocol: AiProviderProtocol) -> bool {
        match load_secret(protocol) {
            Ok(Some(secret)) => {
                self.ai_api_key_present = true;
                self.ai_api_key_source = Some(secret.source);
            }
            Ok(None) | Err(_) => {
                self.ai_api_key_present = false;
                self.ai_api_key_source = None;
            }
        }
        self.ai_api_key_present
    }

    fn reload_ai_panel_configuration(&mut self, cx: &mut Context<Self>) {
        self.ai_panel
            .update(cx, |panel, cx| panel.reload_configuration(cx));
    }

    fn set_ai_protocol(
        &mut self,
        protocol: AiProviderProtocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.app_settings.ai.protocol == protocol {
            return;
        }

        let previous_default = self.app_settings.ai.protocol.default_base_url();
        self.app_settings.ai.protocol = protocol;
        if self.app_settings.ai.base_url.trim().is_empty()
            || self.app_settings.ai.base_url == previous_default
        {
            self.app_settings.ai.base_url = protocol.default_base_url().to_string();
        }
        self.refresh_ai_api_key_presence(protocol);
        self.sync_ai_settings_inputs(window, cx);
        self.persist_settings("已保存 AI 协议设置", cx);
        self.reload_ai_panel_configuration(cx);
    }

    fn save_ai_connection_settings(&mut self, cx: &mut Context<Self>) {
        let base_url = self.ai_base_url_input.read(cx).text().to_string();
        let model = self.ai_model_input.read(cx).text().to_string();
        let base_url = base_url.trim().to_string();
        let model = model.trim().to_string();
        self.app_settings.ai.base_url = if base_url.is_empty() {
            self.app_settings.ai.protocol.default_base_url().to_string()
        } else {
            base_url
        };
        self.app_settings.ai.model = model;
        self.persist_settings("已保存 AI 连接配置", cx);
        self.reload_ai_panel_configuration(cx);
    }

    fn draft_ai_settings(&self, cx: &App) -> AiSettings {
        let mut settings = self.app_settings.ai.clone();
        let base_url = self.ai_base_url_input.read(cx).text().to_string();
        let model = self.ai_model_input.read(cx).text().to_string();
        let base_url = base_url.trim().to_string();
        let model = model.trim().to_string();
        settings.base_url = if base_url.is_empty() {
            settings.protocol.default_base_url().to_string()
        } else {
            base_url
        };
        settings.model = model;
        settings
    }

    fn test_ai_connection(&mut self, cx: &mut Context<Self>) {
        if self.ai_connection_testing {
            return;
        }

        let settings = self.draft_ai_settings(cx);
        let protocol = settings.protocol;
        let model_label = settings.model.trim().to_string();
        let input_api_key = self.ai_api_key_input.read(cx).text().to_string();
        let input_api_key = input_api_key.trim().to_string();
        let api_key = if input_api_key.is_empty() {
            match load_secret(settings.protocol) {
                Ok(Some(secret)) => secret.value,
                Ok(None) => {
                    self.settings_notice = None;
                    self.settings_error = Some(format!(
                        "当前协议还没有可用 API Key，请先保存本地 Key，或提供环境变量 {}",
                        settings.protocol.api_key_env_var()
                    ));
                    cx.notify();
                    return;
                }
                Err(err) => {
                    self.settings_notice = None;
                    self.settings_error = Some(err);
                    cx.notify();
                    return;
                }
            }
        } else {
            input_api_key
        };

        self.ai_connection_testing = true;
        self.settings_notice = Some("正在测试 AI 连接…".to_string());
        self.settings_error = None;
        cx.notify();

        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { test_connection(&settings, &api_key) })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.ai_connection_testing = false;
                match result {
                    Ok(response) => {
                        let usage_message = response.usage.map(|usage| {
                            format!(
                                "（上行 {}，下行 {}）",
                                usage.input_tokens, usage.output_tokens
                            )
                        });
                        let base_message = format!(
                            "测试连接成功：{} / {}{}",
                            protocol.label(),
                            model_label,
                            usage_message.unwrap_or_default()
                        );
                        match record_ai_usage(
                            protocol,
                            AiUsageEventKind::TestConnection,
                            response.usage,
                        ) {
                            Ok(_) => {
                                this.settings_notice = Some(base_message);
                                this.settings_error = None;
                                this.reload_ai_panel_configuration(cx);
                            }
                            Err(err) => {
                                this.settings_notice = None;
                                this.settings_error =
                                    Some(format!("{base_message}，但写入 token 统计失败: {err}"));
                            }
                        }
                    }
                    Err(err) => {
                        if let Err(record_err) =
                            record_ai_usage(protocol, AiUsageEventKind::TestConnection, err.usage)
                        {
                            this.settings_error = Some(format!(
                                "{}；另外写入 token 统计失败: {}",
                                err.message, record_err
                            ));
                            cx.notify();
                            return;
                        }
                        this.reload_ai_panel_configuration(cx);
                        this.settings_notice = None;
                        this.settings_error = Some(err.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_ai_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.ai_api_key_input.read(cx).text().to_string();
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            self.settings_notice = None;
            self.settings_error = Some("请输入 API Key 后再保存".to_string());
            cx.notify();
            return;
        }

        match save_secret(self.app_settings.ai.protocol, &api_key) {
            Ok(_) => {
                self.refresh_ai_api_key_presence(self.app_settings.ai.protocol);
                self.sync_ai_api_key_input(window, cx);
                self.settings_error = None;
                self.settings_notice = Some("已保存 AI API Key 到本地配置".to_string());
                self.reload_ai_panel_configuration(cx);
            }
            Err(err) => {
                self.settings_notice = None;
                self.settings_error = Some(err);
            }
        }
        cx.notify();
    }

    fn clear_ai_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match delete_secret(self.app_settings.ai.protocol) {
            Ok(_) => {
                self.refresh_ai_api_key_presence(self.app_settings.ai.protocol);
                self.sync_ai_api_key_input(window, cx);
                self.settings_error = None;
                self.settings_notice = Some(match self.ai_api_key_source {
                    Some(SecretSource::Environment(env_var)) => {
                        format!("已清除本地 AI API Key，当前仍会回退读取环境变量 {env_var}")
                    }
                    _ => "已清除本地 AI API Key".to_string(),
                });
                self.reload_ai_panel_configuration(cx);
            }
            Err(err) => {
                self.settings_notice = None;
                self.settings_error = Some(err);
            }
        }
        cx.notify();
    }

    fn set_startup_panel_preference(
        &mut self,
        panel: StartupPanelPreference,
        cx: &mut Context<Self>,
    ) {
        self.app_settings.general.startup_panel = panel;
        self.persist_settings("已保存启动默认面板", cx);
    }

    fn set_quick_add_default_mode_preference(
        &mut self,
        mode: QuickAddDefaultMode,
        cx: &mut Context<Self>,
    ) {
        self.app_settings.general.quick_add_default_mode = mode;
        self.persist_settings("已保存快速输入默认模式", cx);
    }

    fn set_notifications_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.app_settings.reminders.notifications_enabled = enabled;
        self.persist_settings("已保存提醒通知设置", cx);
    }

    fn panel_for_keystroke(&self, keystroke: &Keystroke) -> Option<Panel> {
        if keystroke_matches_shortcut(keystroke, &self.shortcut_config.open_main) {
            return Some(Panel::Dashboard);
        }

        if keystroke_matches_shortcut(keystroke, &self.shortcut_config.open_tasks) {
            return Some(Panel::Tasks);
        }

        if keystroke_matches_shortcut(keystroke, &self.shortcut_config.open_records) {
            return Some(Panel::Records);
        }

        if !keystroke.modifiers.platform {
            return None;
        }

        main_panel_shortcuts()
            .into_iter()
            .find(|(key, _)| *key == keystroke.key.as_str())
            .map(|(_, panel)| panel)
    }

    fn apply_shortcut_config_change(&mut self, next_config: ShortcutConfig) -> Result<(), String> {
        if let Err(err) = validate_shortcut_config(&next_config) {
            return Err(err.to_string());
        }

        if let Err(err) = self
            .global_hotkeys
            .borrow_mut()
            .apply_shortcuts(&next_config)
        {
            return Err(err);
        }

        let previous_config = self.shortcut_config.clone();
        let previous_settings = self.app_settings.shortcuts.clone();
        self.shortcut_config = next_config.clone();
        self.app_settings.shortcuts = ShortcutSettings::from(&next_config);
        match save_app_settings(&self.app_settings) {
            Ok(_) => Ok(()),
            Err(err) => {
                self.shortcut_config = previous_config.clone();
                self.app_settings.shortcuts = previous_settings;
                if let Err(revert_err) = self
                    .global_hotkeys
                    .borrow_mut()
                    .apply_shortcuts(&previous_config)
                {
                    eprintln!(
                        "[Settings] Failed to revert shortcuts after save error: {revert_err}"
                    );
                }
                Err(err)
            }
        }
    }

    fn restore_default_shortcuts(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_shortcut_capture = None;
        match self.apply_shortcut_config_change(ShortcutConfig::default()) {
            Ok(_) => {
                self.settings_notice = Some("已恢复默认快捷键".to_string());
                self.settings_error = None;
            }
            Err(err) => {
                self.settings_notice = None;
                self.settings_error = Some(err);
            }
        }
        cx.notify();
    }

    fn next_shortcut_capture_serial(&mut self) -> u64 {
        self.shortcut_capture_serial = self.shortcut_capture_serial.wrapping_add(1);
        self.shortcut_capture_serial
    }

    fn schedule_shortcut_capture_transition(
        &self,
        serial: u64,
        phase: ShortcutCapturePhase,
        delay: std::time::Duration,
        next_state: Option<ShortcutCaptureState>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(delay).await;
            let _ = view.update(cx, |this, cx| {
                let should_apply = this
                    .active_shortcut_capture
                    .as_ref()
                    .is_some_and(|capture| capture.serial == serial && capture.phase == phase);
                if should_apply {
                    this.active_shortcut_capture = next_state.clone();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn enter_shortcut_capture_waiting(
        &mut self,
        field: ShortcutField,
        preview_display: Option<String>,
        serial: u64,
    ) {
        self.active_shortcut_capture = Some(ShortcutCaptureState {
            field,
            phase: ShortcutCapturePhase::Waiting,
            preview_display,
            held_keys: BTreeSet::new(),
            held_modifiers: Modifiers::default(),
            retry_ready: false,
            message: None,
            serial,
        });
    }

    fn enter_shortcut_capture_success(
        &mut self,
        field: ShortcutField,
        display_value: String,
        serial: u64,
    ) {
        self.active_shortcut_capture = Some(ShortcutCaptureState {
            field,
            phase: ShortcutCapturePhase::Success,
            preview_display: Some(display_value),
            held_keys: BTreeSet::new(),
            held_modifiers: Modifiers::default(),
            retry_ready: false,
            message: Some("快捷键已保存并生效".to_string()),
            serial,
        });
    }

    fn enter_shortcut_capture_failure(
        &mut self,
        field: ShortcutField,
        preview_display: Option<String>,
        held_keys: BTreeSet<String>,
        held_modifiers: Modifiers,
        error_message: String,
        serial: u64,
    ) {
        self.active_shortcut_capture = Some(ShortcutCaptureState {
            field,
            phase: ShortcutCapturePhase::Failure,
            preview_display,
            held_keys,
            held_modifiers,
            retry_ready: false,
            message: Some(format!("{error_message}。继续按键重新录制，或按 Esc 退出")),
            serial,
        });
    }

    fn start_shortcut_capture(
        &mut self,
        field: ShortcutField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let serial = self.next_shortcut_capture_serial();
        self.enter_shortcut_capture_waiting(field, None, serial);
        self.settings_notice = None;
        self.settings_error = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn handle_shortcut_capture_keydown(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(capture) = self.active_shortcut_capture.clone() else {
            return false;
        };
        let field = capture.field;
        let mut effective_keystroke = event.keystroke.clone();
        effective_keystroke.modifiers = window.modifiers();

        window.prevent_default();
        cx.stop_propagation();

        if effective_keystroke.key == "escape" && !effective_keystroke.modifiers.modified() {
            self.active_shortcut_capture = None;
            self.settings_notice = None;
            self.settings_error = None;
            cx.notify();
            return true;
        }

        if capture.phase == ShortcutCapturePhase::Success {
            return true;
        }

        if capture.phase == ShortcutCapturePhase::Failure && !capture.retry_ready {
            let mut held_keys = capture.held_keys.clone();
            held_keys.extend(capture_held_keys_for_keystroke(&effective_keystroke.key));
            self.active_shortcut_capture = Some(ShortcutCaptureState {
                held_keys,
                held_modifiers: effective_keystroke.modifiers,
                ..capture
            });
            cx.notify();
            return true;
        }

        let preview_display = match preview_shortcut_from_keystroke(&effective_keystroke) {
            Ok(display) => display,
            Err(err) => {
                let serial = self.next_shortcut_capture_serial();
                self.enter_shortcut_capture_failure(
                    field,
                    capture.preview_display.clone(),
                    capture_held_keys_for_keystroke(&effective_keystroke.key),
                    effective_keystroke.modifiers,
                    err.to_string(),
                    serial,
                );
                self.settings_notice = None;
                self.settings_error = None;
                cx.notify();
                return true;
            }
        };

        let waiting_serial = if capture.phase == ShortcutCapturePhase::Waiting {
            capture.serial
        } else {
            self.next_shortcut_capture_serial()
        };
        self.enter_shortcut_capture_waiting(field, Some(preview_display.clone()), waiting_serial);
        self.settings_notice = None;
        self.settings_error = None;
        cx.notify();

        match shortcut_from_keystroke(&effective_keystroke) {
            Ok(Some(shortcut)) => {
                let mut next_config = self.shortcut_config.clone();
                field.set_value(&mut next_config, shortcut);
                match self.apply_shortcut_config_change(next_config) {
                    Ok(_) => {
                        let serial = self.next_shortcut_capture_serial();
                        self.enter_shortcut_capture_success(field, preview_display, serial);
                        self.schedule_shortcut_capture_transition(
                            serial,
                            ShortcutCapturePhase::Success,
                            std::time::Duration::from_secs(1),
                            None,
                            cx,
                        );
                    }
                    Err(err) => {
                        let serial = self.next_shortcut_capture_serial();
                        self.enter_shortcut_capture_failure(
                            field,
                            Some(preview_display),
                            capture_held_keys_for_keystroke(&effective_keystroke.key),
                            effective_keystroke.modifiers,
                            err,
                            serial,
                        );
                    }
                }
                self.settings_notice = None;
                self.settings_error = None;
                cx.notify();
            }
            Ok(None) => {}
            Err(err) => {
                let serial = self.next_shortcut_capture_serial();
                self.enter_shortcut_capture_failure(
                    field,
                    Some(preview_display),
                    capture_held_keys_for_keystroke(&effective_keystroke.key),
                    effective_keystroke.modifiers,
                    err.to_string(),
                    serial,
                );
                self.settings_notice = None;
                self.settings_error = None;
                cx.notify();
            }
        }

        true
    }

    fn handle_shortcut_capture_keyup(
        &mut self,
        event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(capture) = self.active_shortcut_capture.clone() else {
            return false;
        };

        window.prevent_default();
        cx.stop_propagation();

        if capture.phase != ShortcutCapturePhase::Failure || capture.retry_ready {
            return true;
        }

        let mut held_keys = capture.held_keys.clone();
        held_keys.remove(&event.keystroke.key);
        let held_modifiers = window.modifiers();
        let retry_ready = held_keys.is_empty() && !held_modifiers.modified();

        if held_keys != capture.held_keys
            || held_modifiers != capture.held_modifiers
            || retry_ready != capture.retry_ready
        {
            self.active_shortcut_capture = Some(ShortcutCaptureState {
                held_keys,
                held_modifiers,
                retry_ready,
                ..capture
            });
            cx.notify();
        }

        true
    }

    fn handle_shortcut_capture_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(capture) = self.active_shortcut_capture.clone() else {
            return false;
        };

        window.prevent_default();
        cx.stop_propagation();

        if capture.phase == ShortcutCapturePhase::Success {
            return true;
        }

        if capture.phase == ShortcutCapturePhase::Failure {
            if capture.retry_ready && event.modifiers.modified() {
                let Some(preview_display) = preview_shortcut_from_modifiers(event.modifiers) else {
                    return true;
                };
                let serial = self.next_shortcut_capture_serial();
                self.enter_shortcut_capture_waiting(capture.field, Some(preview_display), serial);
                self.settings_notice = None;
                self.settings_error = None;
                cx.notify();
                return true;
            }

            let held_modifiers = event.modifiers;
            let held_keys = if held_modifiers.modified() {
                capture.held_keys.clone()
            } else {
                BTreeSet::new()
            };
            let retry_ready = held_keys.is_empty() && !held_modifiers.modified();
            if held_keys != capture.held_keys
                || held_modifiers != capture.held_modifiers
                || retry_ready != capture.retry_ready
            {
                self.active_shortcut_capture = Some(ShortcutCaptureState {
                    held_keys,
                    held_modifiers,
                    retry_ready,
                    ..capture
                });
                cx.notify();
            }

            return true;
        }

        let Some(preview_display) = preview_shortcut_from_modifiers(event.modifiers) else {
            if capture.phase == ShortcutCapturePhase::Waiting {
                self.enter_shortcut_capture_waiting(capture.field, None, capture.serial);
                self.settings_notice = None;
                self.settings_error = None;
                cx.notify();
            }
            return true;
        };

        let serial = if capture.phase == ShortcutCapturePhase::Waiting {
            capture.serial
        } else {
            self.next_shortcut_capture_serial()
        };
        self.enter_shortcut_capture_waiting(capture.field, Some(preview_display), serial);
        self.settings_notice = None;
        self.settings_error = None;
        cx.notify();
        true
    }

    fn render_settings_message(&self) -> Option<AnyElement> {
        if let Some(message) = self.settings_notice.as_deref() {
            return Some(render_settings_message_box(message, false));
        }

        self.settings_error
            .as_deref()
            .map(|message| render_settings_message_box(message, true))
    }

    fn render_settings_choice_button<F>(
        &self,
        id: impl Into<String>,
        label: &'static str,
        selected: bool,
        on_click: F,
    ) -> AnyElement
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        Button::new(id.into())
            .child(label)
            .when(selected, |button| {
                button.with_variant(gpui_component::button::ButtonVariant::Primary)
            })
            .on_click(on_click)
            .into_any_element()
    }

    fn render_settings_nav_item(
        &self,
        section: SettingsSection,
        is_active: bool,
        layout_mode: SettingsLayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let px_x = match layout_mode {
            SettingsLayoutMode::Sidebar => px(12.0),
            SettingsLayoutMode::TopTabs => px(14.0),
        };
        let py_y = match layout_mode {
            SettingsLayoutMode::Sidebar => px(10.0),
            SettingsLayoutMode::TopTabs => px(8.0),
        };

        div()
            .id(("settings-section", section as usize))
            .cursor_pointer()
            .px(px_x)
            .py(py_y)
            .rounded(px(10.0))
            .bg(if is_active {
                rgb(0xf5f5f0)
            } else {
                rgb(0xffffff)
            })
            .hover(|style| {
                style.bg(if is_active {
                    rgb(0xf5f5f0)
                } else {
                    rgb(0xf7f7f7)
                })
            })
            .child(
                div()
                    .text_sm()
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_active {
                        rgb(0x262626)
                    } else {
                        rgb(0x595959)
                    })
                    .child(section.label()),
            )
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.current_settings_section = section;
                this.active_shortcut_capture = None;
                if section == SettingsSection::DataSync {
                    this.data_management_panel
                        .update(cx, |panel, cx| panel.reload_settings(window, cx));
                }
                cx.notify();
            }))
    }

    fn render_shortcuts_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app_shortcut_entries = app_shortcut_entries()
            .into_iter()
            .map(|(label, shortcut)| (label.to_string(), shortcut.to_string()))
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("快捷键"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8c8c8c))
                    .line_height(relative(1.5))
                    .child(app_shortcuts_intro()),
            )
            .when_some(self.render_settings_message(), |this, message| {
                this.child(message)
            })
            .child(self.render_shortcut_group(
                "固定应用内快捷键",
                app_shortcut_scope_description(),
                app_shortcut_entries,
            ))
            .child(render_settings_card(
                "应用内面板切换",
                "仅在 Robinne 前台时生效，录制成功后会立即保存并生效。",
                vec![
                    self.render_shortcut_capture_row(ShortcutField::OpenMain, cx),
                    self.render_shortcut_capture_row(ShortcutField::OpenTasks, cx),
                    self.render_shortcut_capture_row(ShortcutField::OpenRecords, cx),
                ],
            ))
            .child(render_settings_card(
                "全局快捷键",
                "即使 Robinne 不在前台也可触发，录制成功后会立即保存并生效。",
                vec![
                    self.render_shortcut_capture_row(ShortcutField::QuickCapture, cx),
                    div()
                        .flex()
                        .gap(px(10.0))
                        .child(
                            Button::new("restore-default-shortcuts")
                                .child("恢复全部默认")
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.restore_default_shortcuts(window, cx);
                                })),
                        )
                        .into_any_element(),
                ],
            ))
    }

    fn render_shortcut_group(
        &self,
        title: &'static str,
        description: &'static str,
        shortcut_entries: Vec<(String, String)>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0x262626))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x8c8c8c))
                            .line_height(relative(1.5))
                            .child(description),
                    ),
            )
            .child(div().flex().flex_col().gap(px(10.0)).children(
                shortcut_entries.into_iter().map(|(label, shortcut)| {
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(14.0))
                        .py(px(12.0))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(rgb(0xf0f0f0))
                        .bg(rgb(0xfafafa))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0x262626))
                                .child(label),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_family(".SystemUIFont")
                                .text_color(rgb(0x0958d9))
                                .child(shortcut),
                        )
                }),
            ))
    }

    fn render_shortcut_capture_row(
        &self,
        field: ShortcutField,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capture_state = self
            .active_shortcut_capture
            .as_ref()
            .filter(|capture| capture.field == field);
        let is_active = capture_state.is_some();
        let phase = capture_state
            .map(|capture| capture.phase)
            .unwrap_or(ShortcutCapturePhase::Waiting);
        let shortcut_value = field.value(&self.shortcut_config);
        let display_value = capture_state
            .and_then(|capture| capture.preview_display.clone())
            .unwrap_or_else(|| {
                format_shortcut_for_display(shortcut_value)
                    .unwrap_or_else(|_| shortcut_value.to_string())
            });
        let subtitle = match capture_state {
            Some(capture) => capture
                .message
                .clone()
                .unwrap_or_else(|| "按下新的快捷键组合，单独按 Esc 取消".to_string()),
            None => "点击右侧按钮开始录制新的快捷键组合".to_string(),
        };
        let row_id = format!("shortcut-capture-row-{}", field.id());
        let button_id = format!("shortcut-capture-button-{}", field.id());

        div()
            .id(row_id)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .p(px(14.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(if is_active {
                rgb(0x91caff)
            } else {
                rgb(0xf0f0f0)
            })
            .bg(if is_active {
                rgb(0xe6f4ff)
            } else {
                rgb(0xfafafa)
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0x262626))
                            .child(field.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if phase == ShortcutCapturePhase::Failure {
                                rgb(0xcf1322)
                            } else {
                                rgb(0x8c8c8c)
                            })
                            .line_height(relative(1.5))
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .flex()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .max_w(px(240.0))
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(999.0))
                            .bg(match phase {
                                ShortcutCapturePhase::Success => rgb(0xf6ffed),
                                ShortcutCapturePhase::Failure => rgb(0xfff2f0),
                                ShortcutCapturePhase::Waiting if is_active => rgb(0xffffff),
                                ShortcutCapturePhase::Waiting => rgb(0xf0f5ff),
                            })
                            .text_sm()
                            .font_family(".SystemUIFont")
                            .text_color(match phase {
                                ShortcutCapturePhase::Success => rgb(0x389e0d),
                                ShortcutCapturePhase::Failure => rgb(0xcf1322),
                                ShortcutCapturePhase::Waiting if is_active => rgb(0x0958d9),
                                ShortcutCapturePhase::Waiting => rgb(0x262626),
                            })
                            .child(display_value),
                    )
                    .when(
                        matches!(
                            phase,
                            ShortcutCapturePhase::Success | ShortcutCapturePhase::Failure
                        ),
                        |this| {
                            this.child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(match phase {
                                        ShortcutCapturePhase::Success => rgb(0x389e0d),
                                        ShortcutCapturePhase::Failure => rgb(0xcf1322),
                                        ShortcutCapturePhase::Waiting => rgb(0x262626),
                                    })
                                    .child(match phase {
                                        ShortcutCapturePhase::Success => "✅",
                                        ShortcutCapturePhase::Failure => "❌",
                                        ShortcutCapturePhase::Waiting => "",
                                    }),
                            )
                        },
                    )
                    .when(!is_active, |this| {
                        this.child(Button::new(button_id).child("录制").small().on_click(
                            cx.listener(move |this, _event, window, cx| {
                                this.start_shortcut_capture(field, window, cx);
                            }),
                        ))
                    }),
            )
            .into_any_element()
    }

    fn render_general_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("通用"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8c8c8c))
                    .line_height(relative(1.5))
                    .child("启动默认面板、快速输入默认模式和提醒通知总开关会立即保存。"),
            )
            .when_some(self.render_settings_message(), |this, message| {
                this.child(message)
            })
            .child(render_settings_card(
                "启动默认面板",
                "仅在普通打开主窗口且当前没有主窗口实例时生效。",
                vec![div()
                    .flex()
                    .gap(px(10.0))
                    .child(self.render_settings_choice_button(
                        "startup-panel-dashboard",
                        "看板",
                        self.app_settings.general.startup_panel
                            == StartupPanelPreference::Dashboard,
                        cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.set_startup_panel_preference(
                                StartupPanelPreference::Dashboard,
                                cx,
                            );
                        }),
                    ))
                    .child(self.render_settings_choice_button(
                        "startup-panel-tasks",
                        "任务",
                        self.app_settings.general.startup_panel == StartupPanelPreference::Tasks,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_startup_panel_preference(StartupPanelPreference::Tasks, cx);
                        }),
                    ))
                    .child(self.render_settings_choice_button(
                        "startup-panel-records",
                        "记录",
                        self.app_settings.general.startup_panel == StartupPanelPreference::Records,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_startup_panel_preference(StartupPanelPreference::Records, cx);
                        }),
                    ))
                    .child(self.render_settings_choice_button(
                        "startup-panel-timeline",
                        "时间线",
                        self.app_settings.general.startup_panel == StartupPanelPreference::Timeline,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_startup_panel_preference(StartupPanelPreference::Timeline, cx);
                        }),
                    ))
                    .into_any_element()],
            ))
            .child(render_settings_card(
                "快速输入默认模式",
                "当全局快捷键直接唤起浮动快速输入时使用该默认值。",
                vec![div()
                    .flex()
                    .gap(px(10.0))
                    .child(self.render_settings_choice_button(
                        "quick-add-default-task",
                        "任务",
                        self.app_settings.general.quick_add_default_mode
                            == QuickAddDefaultMode::Task,
                        cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.set_quick_add_default_mode_preference(
                                QuickAddDefaultMode::Task,
                                cx,
                            );
                        }),
                    ))
                    .child(self.render_settings_choice_button(
                        "quick-add-default-record",
                        "记录",
                        self.app_settings.general.quick_add_default_mode
                            == QuickAddDefaultMode::Record,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_quick_add_default_mode_preference(
                                QuickAddDefaultMode::Record,
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()],
            ))
            .child(render_settings_card(
                "提醒通知",
                "关闭后仍可设置任务提醒时间，但后台不会发送桌面通知。",
                vec![div()
                    .flex()
                    .gap(px(10.0))
                    .child(self.render_settings_choice_button(
                        "notifications-enabled",
                        "允许通知",
                        self.app_settings.reminders.notifications_enabled,
                        cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.set_notifications_enabled(true, cx);
                        }),
                    ))
                    .child(self.render_settings_choice_button(
                        "notifications-disabled",
                        "关闭通知",
                        !self.app_settings.reminders.notifications_enabled,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_notifications_enabled(false, cx);
                        }),
                    ))
                    .into_any_element()],
            ))
    }

    fn render_ai_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let protocol = self.app_settings.ai.protocol;
        let key_status = if let Some(source) = self.ai_api_key_source {
            match source {
                SecretSource::LocalFile => "已保存到本地配置".to_string(),
                SecretSource::Environment(env_var) => format!("来自环境变量 {env_var}"),
            }
        } else {
            "未配置".to_string()
        };
        let secrets_path = secrets_file_path();

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("AI"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8c8c8c))
                    .line_height(relative(1.5))
                    .child("AI 设置只保存协议、Base URL 和 Model。API Key 优先保存到应用自己的本地配置文件；如果本地没有，再回退读取标准环境变量。"),
            )
            .when_some(self.render_settings_message(), |this, message| {
                this.child(message)
            })
            .child(render_settings_card(
                "协议与连接",
                "当前支持 OpenAI 格式和 Anthropic 格式。OpenAI 格式默认走 `/chat/completions`，Anthropic 格式走 `/messages`。",
                vec![
                    div()
                        .flex()
                        .gap(px(10.0))
                        .child(self.render_settings_choice_button(
                            "ai-protocol-openai",
                            "OpenAI 格式",
                            protocol == AiProviderProtocol::OpenAiCompatible,
                            cx.listener(|this, _event, window, cx| {
                                this.set_ai_protocol(
                                    AiProviderProtocol::OpenAiCompatible,
                                    window,
                                    cx,
                                );
                            }),
                        ))
                        .child(self.render_settings_choice_button(
                            "ai-protocol-anthropic",
                            "Anthropic 格式",
                            protocol == AiProviderProtocol::Anthropic,
                            cx.listener(|this, _event, window, cx| {
                                this.set_ai_protocol(AiProviderProtocol::Anthropic, window, cx);
                            }),
                        ))
                        .into_any_element(),
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0x595959))
                                .child("Base URL"),
                        )
                        .child(Input::new(&self.ai_base_url_input))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0x595959))
                                .child("Model"),
                        )
                        .child(Input::new(&self.ai_model_input))
                        .child(
                            div()
                                .flex()
                                .gap(px(10.0))
                                .child(
                                    Button::new("save-ai-connection")
                                        .child("保存连接配置")
                                        .with_variant(
                                            gpui_component::button::ButtonVariant::Primary,
                                        )
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.save_ai_connection_settings(cx);
                                        })),
                                )
                                .child(
                                    Button::new("test-ai-connection")
                                        .child(if self.ai_connection_testing {
                                            "测试中…"
                                        } else {
                                            "测试连接"
                                        })
                                        .when(self.ai_connection_testing, |button| {
                                            button.disabled(true)
                                        })
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.test_ai_connection(cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(0x8c8c8c))
                                        .child("Base URL 留空时会回落到当前协议默认值。测试连接会使用当前输入框里的值。"),
                                )
                                .into_any_element(),
                        )
                        .into_any_element(),
                ],
            ))
            .child(render_settings_card(
                "API Key",
                "当前协议优先读取本地配置文件；如果本地未保存，再回退到标准环境变量。",
                vec![
                    render_info_row("当前状态", &key_status).into_any_element(),
                    render_path_row("本地文件", &secrets_path.display().to_string()).into_any_element(),
                    render_info_row(
                        "环境变量回退",
                        self.app_settings.ai.protocol.api_key_env_var(),
                    )
                    .into_any_element(),
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0x595959))
                                .child("新的 API Key"),
                        )
                        .child(Input::new(&self.ai_api_key_input))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x8c8c8c))
                                .line_height(relative(1.5))
                                .child("如果当前已经有可用值，这里会显示 `********` 占位。输入新值会写入本地配置并覆盖同协议旧值；清除只删本地配置，不会删除环境变量。"),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(10.0))
                                .child(
                                    Button::new("save-ai-api-key")
                                        .child("保存到本地")
                                        .with_variant(
                                            gpui_component::button::ButtonVariant::Primary,
                                        )
                                        .on_click(cx.listener(|this, _event, window, cx| {
                                            this.save_ai_api_key(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("clear-ai-api-key")
                                        .child("清除本地 Key")
                                        .on_click(cx.listener(|this, _event, window, cx| {
                                            this.clear_ai_api_key(window, cx);
                                        })),
                                )
                                .into_any_element(),
                        )
                        .into_any_element(),
                ],
            ))
    }

    fn render_about_settings(&self) -> impl IntoElement {
        let data_dir = app_data_dir();
        let db_path = data_dir.join("data.db");
        let settings_path = settings_file_path();
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("关于"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8c8c8c))
                    .line_height(relative(1.5))
                    .child("当前版本信息和本地运行路径。"),
            )
            .child(render_settings_card(
                "应用信息",
                "这些信息直接来自当前运行包和平台环境。",
                vec![
                    render_info_row("版本", env!("CARGO_PKG_VERSION")).into_any_element(),
                    render_info_row("平台", current_platform_label()).into_any_element(),
                    render_info_row("许可证", "MIT").into_any_element(),
                ],
            ))
            .child(render_settings_card(
                "本地路径",
                "Robinne 当前默认使用以下路径保存数据和设置。",
                vec![
                    render_path_row("数据目录", &data_dir.display().to_string()).into_any_element(),
                    render_path_row("数据库文件", &db_path.display().to_string())
                        .into_any_element(),
                    render_path_row("设置文件", &settings_path.display().to_string())
                        .into_any_element(),
                ],
            ))
    }

    fn render_settings_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match self.current_settings_section {
            SettingsSection::General => self.render_general_settings(cx).into_any_element(),
            SettingsSection::AI => self.render_ai_settings(cx).into_any_element(),
            SettingsSection::Shortcuts => self.render_shortcuts_settings(cx).into_any_element(),
            SettingsSection::DataSync => self.data_management_panel.clone().into_any_element(),
            SettingsSection::About => self.render_about_settings().into_any_element(),
        }
    }

    fn render_settings_panel(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout_mode = self.current_settings_layout_mode(window);
        let sections = SettingsSection::all();
        let current_section = self.current_settings_section;

        div()
            .size_full()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("设置"),
            )
            .child(match layout_mode {
                SettingsLayoutMode::Sidebar => div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(24.0))
                    .child(
                        div()
                            .w(SETTINGS_SIDEBAR_NAV_WIDTH)
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .children(sections.into_iter().map(|section| {
                                self.render_settings_nav_item(
                                    section,
                                    current_section == section,
                                    layout_mode,
                                    cx,
                                )
                                .into_any_element()
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .size_full()
                                    .pr(px(16.0))
                                    .overflow_y_scrollbar()
                                    .child(self.render_settings_content(cx)),
                            ),
                    )
                    .into_any_element(),
                SettingsLayoutMode::TopTabs => div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(div().flex().gap(px(8.0)).children(sections.into_iter().map(
                        |section| {
                            self.render_settings_nav_item(
                                section,
                                current_section == section,
                                layout_mode,
                                cx,
                            )
                            .into_any_element()
                        },
                    )))
                    .child(
                        div().flex_1().min_h_0().overflow_hidden().child(
                            div()
                                .size_full()
                                .pr(px(16.0))
                                .overflow_y_scrollbar()
                                .child(self.render_settings_content(cx)),
                        ),
                    )
                    .into_any_element(),
            })
    }
}

fn render_settings_message_box(message: &str, is_error: bool) -> AnyElement {
    let (background, border, text) = if is_error {
        (rgb(0xfff2f0), rgb(0xffccc7), rgb(0xcf1322))
    } else {
        (rgb(0xf6ffed), rgb(0xb7eb8f), rgb(0x389e0d))
    };

    div()
        .p(px(12.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(border)
        .bg(background)
        .child(
            div()
                .text_sm()
                .text_color(text)
                .line_height(relative(1.5))
                .child(message.to_string()),
        )
        .into_any_element()
}

fn render_settings_card(
    title: &'static str,
    description: &'static str,
    children: Vec<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(16.0))
        .rounded(px(14.0))
        .border_1()
        .border_color(rgb(0xf0f0f0))
        .bg(rgb(0xfcfcfc))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0x262626))
                        .child(title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x8c8c8c))
                        .line_height(relative(1.5))
                        .child(description),
                ),
        )
        .children(children)
        .into_any_element()
}

fn render_info_row(label: &'static str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0x595959))
                .child(label),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x262626))
                .child(value.to_string()),
        )
}

fn render_path_row(label: &'static str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0x595959))
                .child(label),
        )
        .child(
            div()
                .text_sm()
                .font_family(".SystemUIFont")
                .text_color(rgb(0x262626))
                .line_height(relative(1.5))
                .child(value.to_string()),
        )
}

impl Focusable for MainView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_panel = self.current_panel;
        let sidebar_layout_mode = main_sidebar_layout_mode(window.viewport_size().width);
        let on_panel_change = cx.listener(
            |this: &mut MainView,
             panel: &Panel,
             window: &mut Window,
             cx: &mut Context<MainView>| {
                this.switch_to_panel(*panel, window, cx);
            },
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(ClaudeLikeColors::app_background())
            .track_focus(&self.focus_handle(cx))
            .on_modifiers_changed(cx.listener(
                move |this, event: &ModifiersChangedEvent, window, cx| {
                    if this.current_panel == Panel::Settings
                        && this.handle_shortcut_capture_modifiers_changed(event, window, cx)
                    {
                        return;
                    }
                },
            ))
            .on_key_up(cx.listener(move |this, event: &KeyUpEvent, window, cx| {
                if this.current_panel == Panel::Settings
                    && this.handle_shortcut_capture_keyup(event, window, cx)
                {
                    return;
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.current_panel == Panel::Settings
                    && this.handle_shortcut_capture_keydown(event, window, cx)
                {
                    return;
                }

                if let Some(panel) = this.panel_for_keystroke(&event.keystroke) {
                    this.switch_to_panel(panel, window, cx);
                }
            }))
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .w(main_sidebar_width(sidebar_layout_mode))
                    .rounded_tr(px(18.0))
                    .rounded_br(px(18.0))
                    .border_r_1()
                    .border_color(ClaudeLikeColors::separator())
                    .bg(ClaudeLikeColors::sidebar_background()),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .flex_col()
                    .child(self.render_title_bar(cx))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .overflow_hidden()
                            .relative()
                            .child(
                                Sidebar::new(move |panel, window, app| {
                                    on_panel_change(&panel, window, app);
                                })
                                .with_panel(current_panel)
                                .with_layout_mode(sidebar_layout_mode),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .relative()
                                    .bg(ClaudeLikeColors::app_background())
                                    .child(match self.current_panel {
                                        Panel::Dashboard => {
                                            self.dashboard_panel.clone().into_any_element()
                                        }
                                        Panel::Tasks => self.task_panel.clone().into_any_element(),
                                        Panel::Records => {
                                            self.notes_panel.clone().into_any_element()
                                        }
                                        Panel::Timeline => {
                                            self.timeline_panel.clone().into_any_element()
                                        }
                                        Panel::AI => self.ai_panel.clone().into_any_element(),
                                        Panel::Search => {
                                            self.search_panel.clone().into_any_element()
                                        }
                                        Panel::Settings => self
                                            .render_settings_panel(window, cx)
                                            .into_any_element(),
                                    }),
                            )
                            .when_some(self.quick_add_overlay.clone(), |el, overlay| {
                                el.child(overlay)
                            }),
                    ),
            )
    }
}
