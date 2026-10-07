//! Native application menu bar (Tauri 2).
//!
//! Every *custom* menu item uses its action-registry id as the menu item id,
//! so the menu, the command palette, the vim layer, and scripting all run the
//! same [`contracts/action-registry.md`](../../../contracts/action-registry.md)
//! action. Selecting a custom item emits [`MENU_ACTION_EVENT`] with a
//! [`MenuActionPayload`]; the Solid shell listens for that event and runs the
//! action through `ui/src/actions/registry.ts`, which keeps menu and palette
//! in sync by construction.
//!
//! macOS standard roles (Services, Hide, Hide Others, Show All, Quit, About)
//! and the Edit text roles (Cut/Copy/Paste/Select All) are native
//! [`PredefinedMenuItem`]s so the OS handles them: they always perform their
//! native behavior, and when the OS delivers the selection event we *also*
//! emit the corresponding additive registry action (best-effort sync). Roles
//! the OS swallows silently (Services/Hide on macOS) have no event to
//! forward; see [`action_for_native`].
//!
//! Deliberately, no menu item uses a bare-key accelerator (Space, letters):
//! native accelerators are mode-unaware and would fire while typing in text
//! fields or while the vim layer is in insert/visual mode. Transport stays on
//! the vim bindings (`Space` / `s`) and the palette.

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuEvent, MenuId, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
    App, AppHandle, Emitter, Manager, Runtime,
};

/// Global event the Solid UI listens to. Payload is [`MenuActionPayload`].
pub const MENU_ACTION_EVENT: &str = "menu-action";

/// Payload of [`MENU_ACTION_EVENT`]. `action` is always a registry action id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuActionPayload {
    pub action: String,
}

/// A native macOS/text role used in the menu. Only roles listed here may
/// appear; everything else must be an [`MenuItemKind::Action`] carrying a
/// registry action id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRole {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// One row of a submenu. `Action` ids MUST equal a registry action id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItemKind {
    /// Custom item: menu item id == registry action id; always emits.
    Action {
        id: &'static str,
        title: &'static str,
        accelerator: Option<&'static str>,
    },
    Separator,
    Native(NativeRole),
}

/// `(submenu title, items)` for the whole menu bar. `macos` selects the
/// platform variant: macOS gets the standard App menu with native roles;
/// other platforms get Quit/Preferences/About fallbacks in File/Edit/Help.
pub fn menu_spec(macos: bool) -> Vec<(&'static str, Vec<MenuItemKind>)> {
    use MenuItemKind::{Action as A, Native as N, Separator as S};
    let mut spec: Vec<(&'static str, Vec<MenuItemKind>)> = Vec::new();

    if macos {
        spec.push((
            "App",
            vec![
                A { id: "app.about", title: "About ccez-daw", accelerator: None },
                A { id: "app.update.check", title: "Check for Updates…", accelerator: None },
                S,
                A { id: "app.preferences", title: "Preferences…", accelerator: Some("CmdOrCtrl+,") },
                S,
                N(NativeRole::Services),
                S,
                N(NativeRole::Hide),
                N(NativeRole::HideOthers),
                N(NativeRole::ShowAll),
                S,
                N(NativeRole::Quit),
            ],
        ));
    }

    let mut file = vec![
        A { id: "project.new", title: "New Project…", accelerator: Some("CmdOrCtrl+N") },
        A { id: "project.open", title: "Open…", accelerator: Some("CmdOrCtrl+O") },
        A { id: "project.save", title: "Save", accelerator: Some("CmdOrCtrl+S") },
        S,
        A { id: "branch.merge", title: "Merge Branch…", accelerator: None },
    ];
    if !macos {
        file.push(S);
        file.push(A { id: "app.quit", title: "Quit", accelerator: Some("CmdOrCtrl+Q") });
    }
    spec.push(("File", file));

    let mut edit = vec![
        A { id: "project.undo", title: "Undo", accelerator: Some("CmdOrCtrl+Z") },
        A { id: "project.redo", title: "Redo", accelerator: Some("Shift+CmdOrCtrl+Z") },
        S,
        N(NativeRole::Cut),
        N(NativeRole::Copy),
        N(NativeRole::Paste),
        N(NativeRole::SelectAll),
    ];
    if !macos {
        edit.push(S);
        edit.push(A {
            id: "app.preferences",
            title: "Preferences…",
            accelerator: Some("CmdOrCtrl+,"),
        });
    }
    spec.push(("Edit", edit));

    spec.push((
        "View",
        vec![
            A { id: "palette.open", title: "Command Palette…", accelerator: Some("CmdOrCtrl+K") },
            S,
            A { id: "view.zoom.in", title: "Zoom In", accelerator: Some("CmdOrCtrl+=") },
            A { id: "view.zoom.out", title: "Zoom Out", accelerator: Some("CmdOrCtrl+-") },
            A { id: "view.zoom.reset", title: "Reset Zoom", accelerator: Some("CmdOrCtrl+0") },
            S,
            A { id: "view.fullscreen", title: "Toggle Fullscreen", accelerator: Some("F11") },
            A { id: "view.focusSession", title: "Focus Session", accelerator: None },
            S,
            A { id: "session.launch", title: "Launch Session Slot…", accelerator: None },
            A { id: "session.jam_record", title: "Record Jam to Timeline", accelerator: None },
        ],
    ));

    spec.push((
        "Transport",
        vec![
            // No accelerators here on purpose: transport keys (Space / s) belong
            // to the mode-aware vim layer, not to mode-unaware menu shortcuts.
            A { id: "transport.play", title: "Play", accelerator: None },
            A { id: "transport.stop", title: "Stop", accelerator: None },
            S,
            A { id: "engine.set_tempo", title: "Set Tempo…", accelerator: None },
            A { id: "record.punch", title: "Punch In/Out", accelerator: None },
            A { id: "link.join", title: "Join Link Session", accelerator: None },
        ],
    ));

    spec.push((
        "Track",
        vec![
            A { id: "track.add", title: "Add Track", accelerator: Some("CmdOrCtrl+T") },
            A { id: "track.list", title: "List Tracks", accelerator: None },
            S,
            A { id: "track.delete", title: "Delete Selected Track", accelerator: None },
        ],
    ));

    spec.push((
        "Clip",
        vec![
            A { id: "clip.add", title: "Add Clip", accelerator: Some("CmdOrCtrl+L") },
            A { id: "clip.duplicate", title: "Duplicate Clip", accelerator: Some("CmdOrCtrl+D") },
            A { id: "clip.delete", title: "Delete Clip", accelerator: Some("CmdOrCtrl+Backspace") },
            S,
            A { id: "automation.point_set", title: "Set Automation Point…", accelerator: None },
            A { id: "comp.commit", title: "Commit Comp…", accelerator: None },
            A { id: "groove.apply", title: "Apply Groove…", accelerator: None },
        ],
    ));

    let mut help = vec![
        A { id: "help.open_docs", title: "Documentation", accelerator: Some("F1") },
        A { id: "help.show_shortcuts", title: "Keyboard Shortcuts", accelerator: None },
    ];
    if !macos {
        help.push(S);
        help.push(A { id: "app.update.check", title: "Check for Updates…", accelerator: None });
        help.push(A { id: "app.about", title: "About ccez-daw", accelerator: None });
    }
    spec.push(("Help", help));

    spec
}

/// Registry action id for a custom menu item id. Custom item ids ARE action
/// ids, so this is the identity map over the spec's action items.
pub fn action_for_menu_item_id(menu_item_id: &str) -> Option<&'static str> {
    for (_, items) in menu_spec(true).into_iter().chain(menu_spec(false)) {
        for item in items {
            if let MenuItemKind::Action { id, .. } = item {
                if id == menu_item_id {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Best-effort sync action for a native role. `None` means the role is
/// natively handled with no registry equivalent (Services/Hide/Hide Others/
/// Show All on macOS: the OS swallows the event, nothing to forward).
/// (About is a custom `app.about` item, not a native role, so its selection
/// always emits deterministically.)
pub fn action_for_native(role: NativeRole) -> Option<&'static str> {
    match role {
        NativeRole::Quit => Some("app.quit"),
        NativeRole::Cut => Some("edit.cut"),
        NativeRole::Copy => Some("edit.copy"),
        NativeRole::Paste => Some("edit.paste"),
        NativeRole::SelectAll => Some("edit.select_all"),
        NativeRole::Services
        | NativeRole::Hide
        | NativeRole::HideOthers
        | NativeRole::ShowAll => None,
    }
}

/// Built menu plus the runtime ids of native-role items that carry a sync
/// action, for the event handler installed by [`install`].
pub type BuiltMenu<R> = (Menu<R>, Vec<(MenuId, &'static str)>);

/// Build the app menu from [`menu_spec`]. The single source of truth stays in
/// the spec so the mapping test cannot drift from the built menu.
pub fn build_app_menu<R: Runtime, M: Manager<R>>(
    manager: &M,
    macos: bool,
) -> tauri::Result<BuiltMenu<R>> {
    use tauri::menu::MenuBuilder;
    let mut builder = MenuBuilder::new(manager);
    // Native items get muda-assigned ids at build time; record them so the
    // event handler can forward their sync actions.
    let mut native_ids: Vec<(MenuId, &'static str)> = Vec::new();
    let mut keepalive: Vec<PredefinedMenuItem<R>> = Vec::new();
    for (title, items) in menu_spec(macos) {
        let mut sub = SubmenuBuilder::new(manager, title);
        for item in items {
            match item {
                MenuItemKind::Action { id, title, accelerator } => {
                    let mut b = MenuItemBuilder::with_id(id, title);
                    if let Some(acc) = accelerator {
                        b = b.accelerator(acc);
                    }
                    let mi = b.build(manager)?;
                    sub = sub.item(&mi);
                }
                MenuItemKind::Separator => {
                    sub = sub.separator();
                }
                MenuItemKind::Native(role) => {
                    let pi = build_native(manager, role)?;
                    if let Some(action) = action_for_native(role) {
                        native_ids.push((pi.id().clone(), action));
                    }
                    sub = sub.item(&pi);
                    keepalive.push(pi);
                }
            }
        }
        let submenu = sub.build()?;
        builder = builder.item(&submenu);
    }
    // `keepalive` is intentionally held to the end of the build; ownership of
    // the items moves into the menu tree afterwards.
    let _ = &keepalive;
    builder.build().map(|menu| (menu, native_ids))
}

fn build_native<R: Runtime, M: Manager<R>>(
    manager: &M,
    role: NativeRole,
) -> tauri::Result<PredefinedMenuItem<R>> {
    match role {
        NativeRole::Services => PredefinedMenuItem::services(manager, None),
        NativeRole::Hide => PredefinedMenuItem::hide(manager, None),
        NativeRole::HideOthers => PredefinedMenuItem::hide_others(manager, None),
        NativeRole::ShowAll => PredefinedMenuItem::show_all(manager, None),
        NativeRole::Quit => PredefinedMenuItem::quit(manager, None),
        NativeRole::Cut => PredefinedMenuItem::cut(manager, None),
        NativeRole::Copy => PredefinedMenuItem::copy(manager, None),
        NativeRole::Paste => PredefinedMenuItem::paste(manager, None),
        NativeRole::SelectAll => PredefinedMenuItem::select_all(manager, None),
    }
}

/// Resolve a menu selection to its registry action id: custom ids via
/// [`action_for_menu_item_id`], native roles via the runtime id table built
/// alongside the menu (muda assigns native ids at build time, so they cannot
/// live in the pure spec table).
pub fn route_menu_event(
    menu_item_id: &str,
    native_ids: &[(MenuId, &'static str)],
) -> Option<&'static str> {
    if let Some(action) = action_for_menu_item_id(menu_item_id) {
        return Some(action);
    }
    native_ids
        .iter()
        .find(|(id, _)| id.0.as_str() == menu_item_id)
        .map(|(_, action)| *action)
}

/// Route a menu selection: emit [`MENU_ACTION_EVENT`] with the registry
/// action id. `native_ids` is the runtime id table from [`build_app_menu`].
pub fn handle_menu_event<R: Runtime>(
    app: &AppHandle<R>,
    event: &MenuEvent,
    native_ids: &[(MenuId, &'static str)],
) {
    let id = event.id().0.as_str();
    // Native-role handling is best-effort: most native selections are already
    // performed by the OS before/around this event; the emission only keeps
    // palette-side state (e.g. clipboard views) in sync.
    if let Some(action) = route_menu_event(id, native_ids) {
        emit_action(app, action);
        // Non-macOS Quit is a custom item: the OS won't quit for us.
        if action == "app.quit" && !cfg!(target_os = "macos") {
            app.exit(0);
        }
    }
}

/// Install the menu and its event handler. Call once from `setup`.
pub fn install<R: Runtime>(app: &mut App<R>) -> tauri::Result<()> {
    let macos = cfg!(target_os = "macos");
    let (menu, native_ids) = build_app_menu(app, macos)?;
    app.set_menu(menu)?;
    app.on_menu_event(move |app_handle, event| {
        handle_menu_event(app_handle, &event, &native_ids);
    });
    Ok(())
}

fn emit_action<R: Runtime>(app: &AppHandle<R>, action: &str) {
    let _ = app.emit(
        MENU_ACTION_EVENT,
        MenuActionPayload { action: action.to_string() },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Mirror of `contracts/action-registry.md` ids (frozen v0 + the additive
    /// menu-gap rows this module introduces). The mapping test fails if the
    /// menu references an action outside this list.
    const KNOWN_ACTION_IDS: &[&str] = &[
        // frozen v0
        "transport.play",
        "transport.stop",
        "project.new",
        "project.get",
        "project.open",
        "project.save",
        "project.undo",
        "project.redo",
        "op.apply",
        "track.add",
        "track.list",
        "clip.add",
        "param.set",
        "engine.set_tempo",
        "palette.open",
        "vim.mode.normal",
        "vim.mode.insert",
        "vim.mode.visual",
        "vim.motion.left",
        "vim.motion.down",
        "vim.motion.up",
        "vim.motion.right",
        "vim.motion.wordForward",
        "vim.motion.wordBack",
        "vim.motion.lineStart",
        "vim.motion.lineEnd",
        "vim.motion.first",
        "vim.motion.last",
        // additive menu-gap rows (— local; Track K wires UI handlers)
        "app.about",
        "app.quit",
        "app.preferences",
        "app.update.check",
        "app.update.install",
        "edit.cut",
        "edit.copy",
        "edit.paste",
        "edit.select_all",
        "view.zoom.in",
        "view.zoom.out",
        "view.zoom.reset",
        "view.fullscreen",
        "view.focusSession",
        "session.launch",
        "session.jam_record",
        "automation.point_set",
        "comp.commit",
        "groove.apply",
        "branch.merge",
        "record.punch",
        "link.join",
        "help.open_docs",
        "help.show_shortcuts",
        "track.delete",
        "clip.delete",
        "clip.duplicate",
    ];

    fn action_items(macos: bool) -> Vec<(&'static str, &'static str, Option<&'static str>)> {
        menu_spec(macos)
            .into_iter()
            .flat_map(|(_, items)| items)
            .filter_map(|item| match item {
                MenuItemKind::Action { id, title, accelerator } => Some((id, title, accelerator)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn submenu_titles_cover_app_file_edit_view_transport_track_clip_help() {
        let mac_titles: Vec<_> =
            menu_spec(true).into_iter().map(|(t, _)| t).collect();
        assert_eq!(
            mac_titles,
            vec!["App", "File", "Edit", "View", "Transport", "Track", "Clip", "Help"]
        );
        let other_titles: Vec<_> =
            menu_spec(false).into_iter().map(|(t, _)| t).collect();
        assert_eq!(
            other_titles,
            vec!["File", "Edit", "View", "Transport", "Track", "Clip", "Help"]
        );
    }

    #[test]
    fn every_menu_item_maps_to_a_known_registry_action() {
        let known: HashSet<_> = KNOWN_ACTION_IDS.iter().collect();
        for macos in [true, false] {
            for (id, _title, _acc) in action_items(macos) {
                assert!(
                    known.contains(&id),
                    "menu item {id:?} (macos={macos}) has no registry action"
                );
                assert_eq!(action_for_menu_item_id(id), Some(id));
            }
        }
    }

    #[test]
    fn core_registry_actions_are_reachable_from_the_menu() {
        for macos in [true, false] {
            let mut ids: HashSet<_> =
                action_items(macos).into_iter().map(|(id, _, _)| id).collect();
            // Native roles reachable via best-effort sync emission count too.
            for (_, items) in menu_spec(macos) {
                for item in items {
                    if let MenuItemKind::Native(role) = item {
                        if let Some(action) = action_for_native(role) {
                            ids.insert(action);
                        }
                    }
                }
            }
            for expected in [
                "transport.play",
                "transport.stop",
                "project.new",
                "project.open",
                "project.save",
                "project.undo",
                "project.redo",
                "engine.set_tempo",
                "track.add",
                "track.list",
                "clip.add",
                "palette.open",
                "app.about",
                "app.quit",
                "app.preferences",
                "app.update.check",
            ] {
                assert!(ids.contains(expected), "{expected:?} missing (macos={macos})");
            }
        }
    }

    #[test]
    fn menu_item_ids_are_unique_per_variant() {
        for macos in [true, false] {
            let items = action_items(macos);
            let mut seen = HashSet::new();
            for (id, _, _) in items {
                assert!(seen.insert(id), "duplicate menu id {id:?} (macos={macos})");
            }
        }
    }

    #[test]
    fn accelerators_are_unique_per_variant() {
        for macos in [true, false] {
            let mut seen: HashMap<&str, &str> = HashMap::new();
            for (id, _, acc) in action_items(macos) {
                if let Some(a) = acc {
                    assert!(
                        seen.insert(a, id).is_none(),
                        "accelerator {a:?} shared by {:?} and {id:?}",
                        seen.get(a)
                    );
                }
            }
        }
    }

    #[test]
    fn key_items_show_accelerators() {
        for macos in [true, false] {
            let map: HashMap<_, _> = action_items(macos)
                .into_iter()
                .map(|(id, _, acc)| (id, acc))
                .collect();
            for id in [
                "project.new",
                "project.open",
                "project.save",
                "project.undo",
                "project.redo",
                "palette.open",
                "track.add",
                "clip.add",
                "clip.duplicate",
            ] {
                assert!(
                    map.get(id).copied().flatten().is_some(),
                    "{id:?} shows no accelerator (macos={macos})"
                );
            }
        }
    }

    #[test]
    fn native_roles_cover_macos_standards() {
        let native: Vec<_> = menu_spec(true)
            .into_iter()
            .flat_map(|(_, items)| items)
            .filter_map(|item| match item {
                MenuItemKind::Native(role) => Some(role),
                _ => None,
            })
            .collect();
        for role in [
            NativeRole::Services,
            NativeRole::Hide,
            NativeRole::Quit,
            NativeRole::Cut,
            NativeRole::Copy,
            NativeRole::Paste,
            NativeRole::SelectAll,
        ] {
            assert!(native.contains(&role), "missing native role {role:?}");
        }
    }

    #[test]
    fn native_sync_actions_stay_inside_the_registry() {
        let known: HashSet<_> = KNOWN_ACTION_IDS.iter().collect();
        for role in [
            NativeRole::Services,
            NativeRole::Hide,
            NativeRole::HideOthers,
            NativeRole::ShowAll,
            NativeRole::Quit,
            NativeRole::Cut,
            NativeRole::Copy,
            NativeRole::Paste,
            NativeRole::SelectAll,
        ] {
            if let Some(action) = action_for_native(role) {
                assert!(known.contains(&action), "{role:?} -> {action:?} unknown");
            }
        }
        // Pure-OS roles emit nothing.
        assert_eq!(action_for_native(NativeRole::Services), None);
        assert_eq!(action_for_native(NativeRole::Hide), None);
    }

    #[test]
    fn menu_action_payload_shape() {
        let p = MenuActionPayload { action: "transport.play".to_string() };
        let v = serde_json::to_value(&p).expect("serializes");
        assert_eq!(v, serde_json::json!({ "action": "transport.play" }));
        assert_eq!(MENU_ACTION_EVENT, "menu-action");
    }

    #[test]
    fn unknown_ids_map_to_nothing() {
        assert_eq!(action_for_menu_item_id("does.not.exist"), None);
        assert_eq!(action_for_menu_item_id(""), None);
    }

    #[test]
    fn route_prefers_custom_ids_then_native_table() {
        let native = vec![(MenuId::new("muda-native-copy"), "edit.copy")];
        // Custom id wins even when a native row exists.
        assert_eq!(route_menu_event("transport.play", &native), Some("transport.play"));
        // Native runtime id resolves through the table.
        assert_eq!(
            route_menu_event("muda-native-copy", &native),
            Some("edit.copy")
        );
        // Unknown with an empty table resolves to nothing.
        assert_eq!(route_menu_event("muda-native-copy", &[]), None);
        assert_eq!(route_menu_event("nope", &native), None);
    }
}
