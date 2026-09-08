//! The GUI's first slice (FR-17's core loop only): method/URL bar, Send,
//! Headers/Body panels, a response pane. No collections sidebar, tabs,
//! or environments yet (FR-18..21) — those build on this once the
//! send/receive/async loop is proven out, the same way the CLI started
//! with one-shot mode before collections existed.

use std::sync::mpsc;
use std::time::Duration;

use curly_core::exec::{self, ResponseSummary};
use curly_core::model::{Body, Request};
use reqwest::Method;

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

struct HeaderRow {
    name: String,
    value: String,
    enabled: bool,
}

impl HeaderRow {
    fn empty() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}

enum SendOutcome {
    Success(ResponseSummary),
    Error(String),
}

pub struct CurlyApp {
    method: String,
    url: String,
    headers: Vec<HeaderRow>,
    body: String,

    response: Option<Result<ResponseSummary, String>>,
    in_flight: bool,
    rx: Option<mpsc::Receiver<SendOutcome>>,

    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl CurlyApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default_state()
    }

    /// Shared by `new` and unit tests — split out so tests don't need a real
    /// `eframe::CreationContext` (the parameter `new` takes is unused today
    /// anyway; it exists for when GUI state needs `cc.egui_ctx`/storage later).
    fn default_state() -> Self {
        Self {
            method: "GET".to_string(),
            url: String::new(),
            headers: vec![HeaderRow::empty()],
            body: String::new(),
            response: None,
            in_flight: false,
            rx: None,
            runtime: tokio::runtime::Runtime::new().expect("failed to start async runtime"),
            client: reqwest::Client::new(),
        }
    }

    fn build_request(&self) -> anyhow::Result<Request> {
        let method = Method::from_bytes(self.method.to_uppercase().as_bytes())?;
        let mut request = Request::new(method, self.url.clone());
        for h in &self.headers {
            if h.enabled && !h.name.trim().is_empty() {
                request = request.with_header(h.name.clone(), h.value.clone());
            }
        }
        if !self.body.is_empty() {
            request = request.with_body(Body::Raw(self.body.clone()));
        }
        Ok(request)
    }

    /// Fire the request on the background runtime; the response comes back
    /// through `self.rx`, polled each frame in `App::ui`. `ctx.request_repaint()`
    /// wakes the UI thread immediately when it arrives, rather than waiting
    /// on the next input-driven repaint.
    fn send(&mut self, ctx: &egui::Context) {
        let request = match self.build_request() {
            Ok(r) => r,
            Err(e) => {
                self.response = Some(Err(e.to_string()));
                return;
            }
        };

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.in_flight = true;
        self.response = None;

        let client = self.client.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let outcome = match exec::send(&client, &request).await {
                Ok(resp) => SendOutcome::Success(resp),
                Err(e) => SendOutcome::Error(e.to_string()),
            };
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
    }

    fn poll_response(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Ok(outcome) = rx.try_recv() {
            self.in_flight = false;
            self.response = Some(match outcome {
                SendOutcome::Success(r) => Ok(r),
                SendOutcome::Error(e) => Err(e),
            });
            self.rx = None;
        }
    }
}

impl eframe::App for CurlyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_response();

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();
            let mut should_send = false;

            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("method")
                    .selected_text(self.method.clone())
                    .show_ui(ui, |ui| {
                        for m in METHODS {
                            ui.selectable_value(&mut self.method, m.to_string(), m);
                        }
                    });

                let url_response = ui.add(
                    egui::TextEdit::singleline(&mut self.url)
                        .desired_width(ui.available_width() - 80.0)
                        .hint_text("https://api.example.com/..."),
                );
                let enter_in_url = url_response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));

                let send_clicked = ui
                    .add_enabled(
                        !self.in_flight,
                        egui::Button::new(if self.in_flight { "Sending…" } else { "Send" }),
                    )
                    .clicked();

                let ctrl_enter =
                    ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));

                if !self.in_flight
                    && !self.url.trim().is_empty()
                    && (send_clicked || enter_in_url || ctrl_enter)
                {
                    should_send = true;
                }
            });

            ui.separator();

            egui::CollapsingHeader::new("Headers")
                .default_open(true)
                .show(ui, |ui| {
                    let mut remove_idx = None;
                    for (i, h) in self.headers.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut h.enabled, "");
                            ui.add(
                                egui::TextEdit::singleline(&mut h.name)
                                    .hint_text("Header")
                                    .desired_width(160.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut h.value)
                                    .hint_text("value")
                                    .desired_width(260.0),
                            );
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.headers.remove(i);
                    }
                    if ui.button("+ Add header").clicked() {
                        self.headers.push(HeaderRow::empty());
                    }
                });

            egui::CollapsingHeader::new("Body")
                .default_open(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.body)
                            .desired_rows(6)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });

            ui.separator();
            ui.heading("Response");

            match &self.response {
                None if self.in_flight => {
                    ui.label("Sending…");
                }
                None => {
                    ui.weak("No response yet.");
                }
                Some(Err(e)) => {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("Error: {e}"));
                }
                Some(Ok(resp)) => {
                    ui.horizontal(|ui| {
                        let color = if resp.status < 300 {
                            egui::Color32::from_rgb(90, 200, 100)
                        } else if resp.status < 400 {
                            egui::Color32::from_rgb(230, 190, 60)
                        } else {
                            egui::Color32::from_rgb(220, 80, 80)
                        };
                        ui.colored_label(color, format!("{}", resp.status));
                        ui.label(format!("{:?}", resp.elapsed));
                        ui.label(format!("{} bytes", resp.body.len()));
                    });

                    egui::CollapsingHeader::new("Response Headers").show(ui, |ui| {
                        for (k, v) in &resp.headers {
                            ui.label(format!("{k}: {v}"));
                        }
                    });

                    let mut pretty = serde_json::from_str::<serde_json::Value>(&resp.body)
                        .ok()
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                        .unwrap_or_else(|| resp.body.clone());

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut pretty)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
            }

            if should_send {
                self.send(&ctx);
            }
        });

        if self.in_flight {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_get() {
        let app = CurlyApp::default_state();
        let request = app.build_request().unwrap();
        assert_eq!(request.method, Method::GET);
    }

    #[test]
    fn method_field_is_used_and_case_insensitive() {
        let mut app = CurlyApp::default_state();
        app.method = "post".to_string();
        let request = app.build_request().unwrap();
        assert_eq!(request.method, Method::POST);
    }

    #[test]
    fn invalid_method_errors() {
        let mut app = CurlyApp::default_state();
        app.method = "NOT A METHOD".to_string();
        assert!(app.build_request().is_err());
    }

    #[test]
    fn url_is_carried_through() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/get".to_string();
        let request = app.build_request().unwrap();
        assert_eq!(request.url, "https://example.com/get");
    }

    #[test]
    fn enabled_named_headers_are_included() {
        let mut app = CurlyApp::default_state();
        app.headers = vec![HeaderRow {
            name: "Content-Type".to_string(),
            value: "application/json".to_string(),
            enabled: true,
        }];
        let request = app.build_request().unwrap();
        assert_eq!(
            request.headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );
    }

    #[test]
    fn disabled_headers_are_excluded() {
        let mut app = CurlyApp::default_state();
        app.headers = vec![HeaderRow {
            name: "X-Skip".to_string(),
            value: "x".to_string(),
            enabled: false,
        }];
        let request = app.build_request().unwrap();
        assert!(request.headers.is_empty());
    }

    #[test]
    fn headers_with_blank_name_are_excluded() {
        let mut app = CurlyApp::default_state();
        app.headers = vec![HeaderRow {
            name: "   ".to_string(),
            value: "x".to_string(),
            enabled: true,
        }];
        let request = app.build_request().unwrap();
        assert!(request.headers.is_empty());
    }

    #[test]
    fn empty_body_produces_no_body() {
        let app = CurlyApp::default_state();
        let request = app.build_request().unwrap();
        assert!(request.body.is_none());
    }

    #[test]
    fn non_empty_body_is_sent_as_raw() {
        let mut app = CurlyApp::default_state();
        app.body = r#"{"a":1}"#.to_string();
        let request = app.build_request().unwrap();
        assert!(matches!(request.body, Some(Body::Raw(ref s)) if s == r#"{"a":1}"#));
    }
}
