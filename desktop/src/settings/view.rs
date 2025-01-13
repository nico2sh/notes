use std::path::PathBuf;

use eframe::egui::{self, CollapsingHeader};
use log::{error, info};
use notes_core::utilities::path_to_string;

use crate::View;

use super::Settings;

pub struct SettingsView {
    settings: Settings,
}

impl SettingsView {
    pub fn new(settings: &Settings) -> Self {
        Self {
            settings: settings.to_owned(),
        }
    }
}

impl View for SettingsView {
    fn view(&mut self, ui: &mut eframe::egui::Ui) -> anyhow::Result<()> {
        egui::TopBottomPanel::top("Settings")
            .resizable(false)
            .show_separator_line(false)
            .min_height(48.0)
            .show_inside(ui, |ui| {
                CollapsingHeader::new("Workspace")
                    .default_open(true)
                    .show(ui, |ui| {
                        let workpspace_dir = &self.settings.workspace_dir;
                        ui.label("Main Workspace Directory: ");
                        ui.label(
                            workpspace_dir
                                .as_ref()
                                .map_or_else(|| "<None>".to_string(), path_to_string)
                                .to_string(),
                        );
                        let button = ui.button("Browse");
                        if button.clicked() {
                            if let Ok(path) = pick_workspace() {
                                if let Err(e) = self.settings.set_workspace(path) {
                                    error!("Error setting the workspace: {}", e);
                                }
                            }
                        }
                    })
            });
        egui::TopBottomPanel::bottom("Settings buttons")
            .resizable(false)
            .min_height(0.0)
            .show_inside(ui, |ui| {
                ui.add_space(8.0);
                let close_button = egui::Button::new("Close");
                let close_response = if self.settings.workspace_dir.is_some() {
                    ui.add_enabled(true, close_button)
                } else {
                    ui.add_enabled(false, close_button)
                };
                if close_response.clicked() {
                    info!("Closing");
                }
            });
        Ok(())
    }
}

fn pick_workspace() -> anyhow::Result<PathBuf> {
    let handle = rfd::FileDialog::new()
        .set_title("Choose a Workspace Directory")
        .pick_folder()
        .ok_or(anyhow::anyhow!("Dialog Closed"))?;

    Ok(handle.to_path_buf())
}
