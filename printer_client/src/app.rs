use std::{
    net::IpAddr,
    time::{Duration, Instant},
};
use url::Url;

use egui::{
    ahash::{HashMap, HashMapExt},
    Color32, Context, RichText, Widget,
};

use crate::{get_settings, save_settings, update, Printer};

#[derive(serde::Deserialize, serde::Serialize)]
pub enum Page {
    Home,
    Settings,
    NewPrinter,
    RemovePrinter,
}

pub enum Crud {
    Remove,
    Add,
}

/// Current version status
pub enum VersionStatus {
    UpToDate,
    OutDated(String),
}

pub struct Interface {
    picked_path: Option<String>,
    dropped_files: Vec<egui::DroppedFile>,
    current_page: Page,
    settings: Settings,

    carry: String, // Insturctions to carry to next iteration
    string: String,
    pub error: String,

    selected_printer: IpAddr,
    submit_result: Option<(String, Instant)>,

    update_status: VersionStatus,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Settings {
    printers: HashMap<IpAddr, Printer>, // Settings intended to be handled securely
}

impl Default for Interface {
    fn default() -> Self {
        let settings = get_settings().unwrap();
        Self {
            picked_path: None,
            dropped_files: Vec::new(),
            current_page: Page::Home,

            carry: String::new(),
            string: String::new(),
            error: String::new(),

            selected_printer: *settings
                .printers
                .clone()
                .keys()
                .next()
                .unwrap_or(&"0.0.0.0".parse::<IpAddr>().unwrap()),
            submit_result: None,
            settings,
            update_status: crate::update::check_oudated(),
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
            Page::NewPrinter => self.new_printer(ctx),
            Page::RemovePrinter => self.remove_printer(ctx),
        }
    }
}

impl Settings {
    pub fn new() -> Self {
        let printers: HashMap<IpAddr, Printer> = HashMap::new();

        Settings { printers }
    }

    fn update(&mut self, crud: Crud, key: String, value: Option<String>) {
        match crud {
            Crud::Remove => {
                self.printers.remove(&key.parse().unwrap());
            }
            Crud::Add => {
                if let Some(val) = value {
                    let key = key.parse().unwrap();
                    let printer = Printer::new(val);
                    self.printers.insert(key, printer);
                } else {
                    panic!("Attempted to add to settings with no value");
                }
            }
        }

        // Save settings
        if let Err(e) = save_settings(&self) {
            eprintln!("[Failed to update settings]: {}", e);
        }
    }
}

impl eframe::App for Interface {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_pixels_per_point(1.2);

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if ui.button("Home").clicked() {
                        self.current_page = Page::Home;
                        self.error = String::new();
                    }
                    if ui.button("Settings").clicked() {
                        self.current_page = Page::Settings;
                        self.error = String::new();
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
            ui.label(
                egui::RichText::new("Printing")
                    .heading()
                    .color(egui::Color32::from_rgb(255, 255, 255)),
            );

            if !self.dropped_files.is_empty() {
                if ui.button("Clear dropped files").clicked() {
                    self.dropped_files.clear();
                }

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
            } else {
                // No files dropped, use default settings
                ui.horizontal(|ui| {
                    if ui.button("Open file…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.picked_path = Some(path.display().to_string());
                        }
                    }

                    ui.label("Selected File")
                });

                if let Some(picked_path) = &self.picked_path {
                    ui.horizontal(|ui| {
                        ui.label("Picked file:");
                        ui.monospace(picked_path);
                    });
                }
            }

            preview_files_being_dropped(ctx);

            ctx.input(|i| {
                if !i.raw.dropped_files.is_empty() {
                    self.dropped_files = i.raw.dropped_files.clone();
                }
            });

            ui.add_space(8.0);
            if &self.settings.printers.len() > &0 {
                let selected = &self.selected_printer;

                egui::ComboBox::from_label("Selected Printer")
                    .selected_text(format!("{selected:?}"))
                    .show_ui(ui, |ui| {
                        ui.style_mut().wrap = Some(false);
                        ui.set_min_width(60.0);

                        for key in self.settings.printers.keys() {
                            ui.selectable_value(&mut self.selected_printer, *key, key.to_string());
                        }
                    });
            } else {
                ui.label("Please add a printer in settings");
            }

            ui.add_space(8.0);
            self.send_button(ui);

            if let Some(value) = self.submit_result.clone() {
                if value.1.elapsed() >= Duration::from_secs(10) {
                    self.submit_result = None;
                }

                ui.label(value.0.clone());
            }

            if !self.error.is_empty() {
                ui.label(
                    RichText::new(self.error.clone())
                        .color(Color32::RED)
                        .strong(),
                );
            }

            ui.separator();
            ui.add_space(16.0);

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                footer(ui);
                egui::warn_if_debug_build(ui);
                self.version_warning(ui);
            });
        });
    }

    fn settings_page(&mut self, ctx: &Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Available Printers")
                    .heading()
                    .color(egui::Color32::from_rgb(255, 255, 255)),
            );
            ui.group(|ui| {
                if self.settings.printers.len() != 0 {
                    for printer in self.settings.printers.clone().keys() {
                        ui.horizontal(|ui| {
                            ui.label(printer.to_string());
                            ui.add_space(3.0);
                            if ui.button("Remove").clicked() {
                                self.carry = printer.to_string();
                                self.current_page = Page::RemovePrinter;
                            }
                        });
                    }
                } else {
                    ui.label("No Printers Added");
                }
            });

            if ui
                .add(egui::Button::new(RichText::new("Add a Printer")))
                .clicked()
            {
                self.current_page = Page::NewPrinter;
            }

            ui.separator();

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                footer(ui);
                egui::warn_if_debug_build(ui);
                self.version_warning(ui);
                //#[cfg(not(debug_assertions))]
            });
        });
    }

    fn remove_printer(&mut self, ctx: &Context) {
        let instruction = self.carry.clone();
        if !instruction.is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label(RichText::from("Are You Sure you wish to remove this printer?").heading());
                ui.label(
                    RichText::new(&instruction)
                        .color(Color32::from_rgb(214, 2, 7))
                        .heading(),
                );

                ui.separator();

                ui.horizontal(|ui| {
                    if ui.add_sized([80., 30.], egui::Button::new("Yes")).clicked() {
                        self.settings.update(Crud::Remove, instruction, None);
                        self.current_page = Page::Settings;
                        self.carry = String::new();
                    }

                    ui.add_space(20.);

                    if ui.add_sized([80., 30.], egui::Button::new("No")).clicked() {
                        self.current_page = Page::Settings;
                        self.carry = String::new();
                    }
                });
            });
        } else {
            self.current_page = Page::Settings;
        }
    }

    fn new_printer(&mut self, ctx: &Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(RichText::new("Add a Printer"));

            ui.separator();

            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.string).hint_text("IP Address"));
                ui.label("Remote IP");
            });

            ui.add_space(20.);

            ui.horizontal(|ui| {
                password_ui(ui, &mut self.carry);
                ui.label("Password");
            });

            ui.separator();

            ui.horizontal(|ui| {
                if ui
                    .add_sized([80., 30.], egui::Button::new("Done"))
                    .clicked()
                {
                    let is_valid = &self.string.parse(); // Simply tests if valid address

                    if !self.carry.is_empty() && !self.string.is_empty() && is_valid.is_ok() {
                        if !self
                            .settings
                            .printers
                            .contains_key(&is_valid.clone().unwrap())
                        {
                            self.settings.update(
                                Crud::Add,
                                self.string.clone(),
                                Some(self.carry.clone()),
                            );

                            self.current_page = Page::Settings;
                            self.carry = String::new();
                            self.string = String::new();
                            self.error = String::new();
                        } else {
                            self.error = String::from("Printer already added");
                        }
                    } else if is_valid.is_err() {
                        self.error = String::from("Invalid IP Address")
                    } else {
                        self.error = String::from("Missing Input");
                    }
                }

                ui.add_space(20.);

                if ui
                    .add_sized([80., 30.], egui::Button::new("Cancel"))
                    .clicked()
                {
                    self.current_page = Page::Settings;
                    self.carry = String::new();
                    self.string = String::new();
                }
            });

            if !self.error.is_empty() {
                ui.label(
                    RichText::new(self.error.clone())
                        .color(Color32::RED)
                        .strong(),
                );
            }
        });
    }

    fn send_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_sized([80., 30.], egui::Button::new("Print File"))
            .clicked()
        {
            let parsed_url =
                Url::parse(&format!("https://{}:4433", self.selected_printer)).unwrap();
            let printer_settings = self
                .settings
                .printers
                .get_mut(&self.selected_printer)
                .expect("Failed to get settings for selected printer.");

            if self.dropped_files.is_empty() {
                if let Some(file) = &self.picked_path {
                    // Handle result of sending file
                    match crate::send_file(
                        parsed_url,
                        Some("localhost".to_string()),
                        None,
                        file.into(),
                        Some(printer_settings),
                    ) {
                        Ok(_) => {
                            self.submit_result =
                                Some(("Successfully printed file".to_string(), Instant::now()))
                        }
                        Err(e) => {
                            self.submit_result =
                                Some((format!("Failed to print:\n {:?}", e), Instant::now()))
                        }
                    };
                } else {
                    self.error = String::from("No Send file specified")
                }
            } else {
                let mut results = Vec::new();
                for file in &self.dropped_files {
                    let file = match &file.path {
                        Some(v) => v,
                        None => {
                            results.push(
                                "Failed to get one of the files; Do all the files exist?"
                                    .to_string(),
                            );
                            break;
                        }
                    };

                    match crate::send_file(
                        parsed_url.clone(),
                        Some("localhost".to_string()),
                        None,
                        file.into(),
                        Some(printer_settings),
                    ) {
                        Ok(_) => results.push(format!(
                            "Successfully printed: {:?}",
                            file.file_name().unwrap_or_default()
                        )),
                        Err(e) => {
                            results.push(format!(
                                "Failed to print {:?}: {:?}",
                                file.file_name().unwrap_or_default(),
                                e
                            ));
                        }
                    };
                }
                self.submit_result = Some((results.join("\n"), Instant::now()));
            }
        }
    }

    fn version_warning(&mut self, ui: &mut egui::Ui) {
        if let VersionStatus::OutDated(ver) = &self.update_status {
            //RichText::new(format!("New Version Available: {} -> {}", env!("CARGO_PKG_VERSION"), ver))
            let button = egui::Button::new(
                RichText::new(format!(
                    "New Version Available: {} -> {}",
                    env!("CARGO_PKG_VERSION"),
                    ver
                ))
                .small()
                .color(Color32::from_rgb(255, 140, 0)),
            )
            .frame(false);
            let mut response = button.ui(ui);

            if cfg!(target_os = "windows") {
                response = response.on_hover_text("Click to launch updater");
            } else if cfg!(target_os = "linux") {
                response = response.on_hover_text("Click to launch updater");
            } else {
                response = response.on_hover_text("Please visit releases to update app");
            }

            if response.clicked() {
                if let Err(e) = update::update() {
                    self.error = e.to_string();
                };
            }
        }
    }
}

fn footer(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.hyperlink_to(
            "Remote print",
            "https://github.com/CodedMasonry/remote_print/releases",
        );
        ui.label(format!("\tv{}", env!("CARGO_PKG_VERSION")));
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

// Credit: https://github.com/emilk/egui/blob/master/crates/egui_demo_lib/src/demo/password.rs
#[allow(clippy::ptr_arg)] // false positive
pub fn password_ui(ui: &mut egui::Ui, password: &mut String) -> egui::Response {
    // This widget has its own state — show or hide password characters (`show_plaintext`).
    // In this case we use a simple `bool`, but you can also declare your own type.
    // It must implement at least `Clone` and be `'static`.
    // If you use the `persistence` feature, it also must implement `serde::{Deserialize, Serialize}`.

    // Generate an id for the state
    let state_id = ui.id().with("show_plaintext");

    // Get state for this widget.
    // You should get state by value, not by reference to avoid borrowing of [`Memory`].
    let mut show_plaintext = ui.data_mut(|d| d.get_temp::<bool>(state_id).unwrap_or(false));

    // Process ui, change a local copy of the state
    // We want TextEdit to fill entire space, and have button after that, so in that case we can
    // change direction to right_to_left.
    let result = ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        // Show the password field:
        ui.add(
            egui::TextEdit::singleline(password)
                .password(!show_plaintext)
                .hint_text("Password"),
        );

        // Toggle the `show_plaintext` bool with a button:
        let response = ui
            .add(egui::SelectableLabel::new(show_plaintext, "👁"))
            .on_hover_text("Show/hide password");

        if response.clicked() {
            show_plaintext = !show_plaintext;
        }
    });

    // Store the (possibly changed) state:
    ui.data_mut(|d| d.insert_temp(state_id, show_plaintext));

    result.response
}

pub fn password(password: &mut String) -> impl egui::Widget + '_ {
    move |ui: &mut egui::Ui| password_ui(ui, password)
}
