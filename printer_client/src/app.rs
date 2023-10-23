#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Interface {
    label: String,

    #[serde(skip)]
    value: f32,
}

impl Default for Interface {
    fn default() -> Self {
        Self {
            // Example stuff:
            label: "Hello World!".to_owned(),
            value: 2.7,
        }
    }
}

impl Interface {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        cc.egui_ctx.set_visuals(egui::Vis)

        Default::default()
    }
}

impl eframe::App for Interface {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Remote Print");
        });
    }
}
