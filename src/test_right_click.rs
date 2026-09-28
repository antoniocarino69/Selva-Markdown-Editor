use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Test",
        options,
        Box::new(|_cc| Box::new(App { 
            text: "Select this text and right click".into(),
            last_state: None,
        })),
    )
}

struct App {
    text: String,
    last_state: Option<egui::text_edit::TextEditState>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let output = egui::TextEdit::multiline(&mut self.text).show(ui);
            
            output.response.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    println!("Copy clicked");
                }
            });
            
            let is_right_click = ui.input(|i| i.pointer.secondary_down() || i.pointer.secondary_pressed() || i.pointer.secondary_released());
            
            if is_right_click {
                if let Some(state) = self.last_state.clone() {
                    state.store(ui.ctx(), output.response.id);
                }
            } else {
                if let Some(state) = egui::TextEdit::load_state(ui.ctx(), output.response.id) {
                    self.last_state = Some(state);
                }
            }
        });
    }
}
