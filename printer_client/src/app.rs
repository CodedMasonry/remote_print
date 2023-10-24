use egui::{
    ahash::{HashMap, HashMapExt},
    Color32, Context, RichText,
};

#[derive(serde::Deserialize, serde::Serialize)]
pub enum Page {
    Home,
    Settings,
    NewPrinter
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Interface {
    picked_path: Option<String>,
    dropped_files: Vec<egui::DroppedFile>,
    current_page: Page,
    settings: Settings,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct Settings {
    username: String,
    key: String,
    encrypted_settings: EncryptedSettings,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct EncryptedSettings {
    printers: HashMap<String, String>,
}

impl Default for Interface {
    fn default() -> Self {
        Self {
            picked_path: None,
            dropped_files: Vec::new(),
            current_page: Page::Home,
            settings: Settings::parse("username".to_string(), "key".to_string()),
        }
    }
}

impl Interface {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Default::default()
    }

    pub fn render_page(&mut self, ctx: &Context) {
        match self.current_page {
            Page::Home => self.home_page(ctx),
            Page::Settings => self.settings_page(ctx),
            Page::NewPrinter => self.new_printer(ctx)
        }
    }
}

impl Settings {
    fn parse(username: String, key: String) -> Self {
        let encrypted_settings = EncryptedSettings {
            printers: HashMap::new(),
        };

        Settings {
            username,
            key,
            encrypted_settings,
        }
    }
}

impl eframe::App for Interface {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_pixels_per_point(1.1);

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ui.menu_button("File", |ui| {
                        if ui.button("Quit").clicked() {
                            _frame.close();
                        }
                    });

                    if ui.button("Home").clicked() {
                        self.current_page = Page::Home;
                    }
                    if ui.button("Settings").clicked() {
                        self.current_page = Page::Settings;
                    }
                    ui.add_space(16.0);
                }
            });
        });

        self.render_page(&ctx);
    }
}

impl Interface {
    fn home_page(&mut self, ctx: &Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(egui::RichText::new("Printing").heading().color(egui::Color32::from_rgb(255, 255, 255)));

            if ui.button("Open file…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    self.picked_path = Some(path.display().to_string());
                }
            }

            if let Some(picked_path) = &self.picked_path {
                ui.horizontal(|ui| {
                    ui.label("Picked file:");
                    ui.monospace(picked_path);
                });
            }

            if !self.dropped_files.is_empty() {
                ui.group(|ui| {
                    ui.label("Dropped Files:");

                    for file in &self.dropped_files {
                        let mut info = if let Some(path) = &file.path {
                            path.display().to_string()
                        } else if !file.name.is_empty() {
                            file.name.clone()
                        } else {
                            "???".to_owned()
                        };

                        let mut additional_info = vec![];
                        if !file.mime.is_empty() {
                            additional_info.push(format!("{} bytes", file.mime));
                        }
                        if let Some(bytes) = &file.bytes {
                            additional_info.push(format!("{} bytes", bytes.len()));
                        }
                        if !additional_info.is_empty() {
                            info += &format!(" ({}),", additional_info.join(", "));
                        }

                        ui.label(info);
                    }
                });
            }

            preview_files_being_dropped(ctx);

            ctx.input(|i| {
                if !i.raw.dropped_files.is_empty() {
                    self.dropped_files = i.raw.dropped_files.clone();
                }
            });

            ui.separator();
            ui.add_space(16.0);

            ui.collapsing("Click to see what is hidden!", |ui| {
                ui.label("Not much, as it turns out");
            });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                footer(ui);
                egui::warn_if_debug_build(ui);
            });
        });
    }
    
    fn settings_page(&mut self, ctx: &Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(egui::RichText::new("Available Printers").heading().color(egui::Color32::from_rgb(255, 255, 255)));
            ui.group(|ui| {
                if self.settings.encrypted_settings.printers.len() != 0 {
                    for printer in self.settings.encrypted_settings.printers.clone().keys() {
                        ui.horizontal(|ui| {
                            ui.label(printer);
                            if ui.button("remove").clicked() {
                                self.settings.encrypted_settings.printers.remove(printer);
                            }
                        });
                    }
                } else {
                    ui.label("No Printers Added");
                }


            });

            ui.separator();
            ui.label(egui::RichText::new("General Info").heading().color(egui::Color32::from_rgb(255, 255, 255)));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label("Username: ");
                ui.label(RichText::new(&self.settings.username).color(Color32::GREEN));
            });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                footer(ui);
                egui::warn_if_debug_build(ui);
            });
        });
    }

    fn new_printer(&mut self, ctx: &Context) {
        egui::CentralPanel::default().show(ctx, |_ui| {
            
        });
    }
}

fn footer(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label("Powered by ");
        ui.hyperlink_to(
            "remote_print",
            "https://github.com/CodedMasonry/remote_print",
        );
        ui.label(".");
    });
}

fn preview_files_being_dropped(ctx: &egui::Context) {
    use egui::*;
    use std::fmt::Write as _;

    if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
        let text = ctx.input(|i| {
            let mut text = "Dropping files:\n".to_owned();
            for file in &i.raw.hovered_files {
                if let Some(path) = &file.path {
                    write!(text, "\n{}", path.display()).ok();
                } else if !file.mime.is_empty() {
                    write!(text, "\n{}", file.mime).ok();
                } else {
                    text += "\n???";
                }
            }
            text
        });

        let painter =
            ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("file_drop_target")));

        let screen_rect = ctx.screen_rect();
        painter.rect_filled(screen_rect, 0.0, Color32::from_black_alpha(45));
        painter.text(
            screen_rect.center(),
            Align2::CENTER_CENTER,
            text,
            TextStyle::Heading.resolve(&ctx.style()),
            Color32::WHITE,
        );
    }
}
