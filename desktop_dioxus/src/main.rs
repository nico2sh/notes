#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod editor;
mod modal;

mod settings;
use std::rc::Rc;

use dioxus::prelude::*;
use editor::{note_browser::NoteBrowser, text_editor::TextEditor};
use log::{debug, info};
use modal::Modal;
use settings::Settings;

use core_notes::{nfs::NotePath, NoteVault};

// Urls are relative to your Cargo.toml file
const THEME: Asset = asset!("./assets/theme.css");
const FONTS: Asset = asset!("./assets/fonts.css");
const ICONS: Asset = asset!("./assets/icons.css");
const STYLE: Asset = asset!("./assets/main.css");

fn main() {
    // Init logger
    env_logger::Builder::new()
        .filter(Some("noters"), log::LevelFilter::max())
        .init();
    info!("starting app");

    dioxus::launch(App);
}

#[derive(Debug, Clone)]
pub struct AppContext {
    pub vault: NoteVault,
    pub current_error: Signal<Option<String>>,
}

#[allow(non_snake_case)]
pub fn App() -> Element {
    let settings = use_signal(|| {
        info!("Settings loaded");
        Settings::load().unwrap()
    });
    use_context_provider(|| {
        let error: Signal<Option<String>> = Signal::new(None);
        let workspace_path = settings.read();
        let vault = NoteVault::new(workspace_path.workspace_dir.clone().unwrap()).unwrap();
        AppContext {
            vault,
            current_error: error,
        }
    });

    let app_context: AppContext = use_context();
    let error: Signal<Option<String>> = app_context.current_error;

    let current_note_path: SyncSignal<Option<NotePath>> = use_signal_sync(|| None);
    let note_path_display = use_memo(move || {
        let d = match &*current_note_path.read() {
            Some(path) => {
                if path.is_note() {
                    path.to_string()
                } else {
                    String::new()
                }
            }
            None => String::new(),
        };
        d
    });
    let mut modal = use_signal(Modal::new);
    let editor_signal: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    if !modal.read().is_open() {
        spawn(async move {
            loop {
                if let Some(e) = editor_signal.with(|f| f.clone()) {
                    info!("Focus input on Editor");
                    let _ = e.set_focus(true).await;
                    break;
                }
            }
        });
    }

    rsx! {
        document::Link { rel: "stylesheet", href: THEME }
        document::Link { rel: "stylesheet", href: FONTS }
        document::Link { rel: "stylesheet", href: ICONS }
        document::Link { rel: "stylesheet", href: STYLE }
        div {
            class: "container",
            onkeydown: move |event: Event<KeyboardData>| {
                let key = event.data.code();
                let modifiers = event.data.modifiers();
                if modifiers.meta() && key == Code::KeyO {
                    debug!("Trigger Open Note Select");
                    modal.write().set_note_select();
                }
                if modifiers.meta() && key == Code::KeyS {
                    debug!("Trigger Open Note Search");
                    modal.write().set_note_search();
                }
            },
            // We close any modal if we click on the main UI
            onclick: move |_e| {
                if modal.read().is_open() {
                    modal.write().close();
                    info!("Close dialog");
                }
            },
            aside {
                class: "sidebar",
                NoteBrowser {
                    note_path: current_note_path,
                }
            }
            header {
                class: "header",
                div {
                    class: "path",
                    "{note_path_display}"
                }
            }
            div {
                class: "mainarea",
                { Modal::get_element(modal, current_note_path) },
                TextEditor {
                    note_path: current_note_path,
                    editor_signal,
                }
            }
            footer {
                class: "footer",
                if let Some(err) = &*error.read() {
                        p{"{err}"}
                }
            }
        }
    }
}
