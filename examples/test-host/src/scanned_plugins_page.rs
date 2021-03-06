use eframe::egui;

use dropseed::DSEngineRequest;

use super::DSExampleGUI;

pub(crate) fn show(app: &mut DSExampleGUI, ui: &mut egui::Ui) {
    // TODO: Add/remove plugin paths.

    if ui.button("Rescan all plugin directories").clicked() {
        app.engine_handle.send(DSEngineRequest::RescanPluginDirectories);
    }

    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Available Plugins");
        egui::ScrollArea::horizontal().id_source("available_plugs_hscroll").show(ui, |ui| {
            egui::Grid::new("available_plugs").num_columns(10).striped(true).show(ui, |ui| {
                ui.label("NAME");
                ui.label("VERSION");
                ui.label("VENDOR");
                ui.label("FORMAT");
                ui.label("FORMAT VERSION");
                ui.label("DESCRIPTION");
                ui.label("RDN");
                ui.label("URL");
                ui.label("MANUAL URL");
                ui.label("SUPPORT URL");
                ui.end_row();

                for plugin in app.plugin_list.iter() {
                    ui.label(&plugin.0.description.name);
                    ui.label(&plugin.0.description.version);
                    ui.label(&plugin.0.description.vendor);
                    ui.label(format!("{}", plugin.0.format));
                    ui.label(&plugin.0.format_version);
                    ui.label(&plugin.0.description.description);
                    ui.label(&plugin.0.description.id);
                    ui.label(&plugin.0.description.url);
                    ui.label(&plugin.0.description.manual_url);
                    ui.label(&plugin.0.description.support_url);
                    ui.end_row();
                }
            });
        });

        ui.separator();

        ui.heading("Failed Plugin Errors");
        egui::ScrollArea::horizontal().id_source("failed_plugs_hscroll").show(ui, |ui| {
            egui::Grid::new("failed_plugs").num_columns(2).striped(true).show(ui, |ui| {
                ui.label("PATH");
                ui.label("ERROR");
                ui.end_row();

                for (path, error) in app.failed_plugins_text.iter() {
                    ui.label(path);
                    ui.label(error);
                    ui.end_row();
                }
            });
        });
    });
}
