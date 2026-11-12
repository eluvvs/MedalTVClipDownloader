#![windows_subsystem = "windows"]

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use eframe::egui;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Config {
    last_directory: String,
}

enum DownloadMessage {
    Progress(f32),
    Success(String),
    Error(String),
}

struct ClipDownloaderApp {
    url_input: String,
    progress: f32,
    is_downloading: bool,
    status_text: String,
    rx: Option<Receiver<DownloadMessage>>,
    stop_signal: Arc<AtomicBool>,
}

impl Default for ClipDownloaderApp {
    fn default() -> Self {
        Self {
            url_input: String::new(),
            progress: 0.0,
            is_downloading: false,
            status_text: String::new(),
            rx: None,
            stop_signal: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn get_config_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        PathBuf::from(appdata).join("MedalTVClipDownloader")
    } else if let Some(home) = dirs::home_dir() {
        home.join(".config").join("MedalTVClipDownloader")
    } else {
        PathBuf::from("MedalTVClipDownloader")
    }
}

fn get_config_file() -> PathBuf {
    get_config_dir().join("config.json")
}

fn load_last_directory() -> PathBuf {
    let config_path = get_config_file();
    if config_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<Config>(&contents) {
                let p = PathBuf::from(config.last_directory);
                if p.is_dir() {
                    return p;
                }
            }
        }
    }

    if let Some(d) = dirs::download_dir() {
        if d.is_dir() {
            return d;
        }
    }
    if let Some(v) = dirs::video_dir() {
        if v.is_dir() {
            return v;
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn save_last_directory(dir: &std::path::Path) {
    let dir_str = dir.to_string_lossy().to_string();
    let config = Config {
        last_directory: dir_str,
    };
    let config_dir = get_config_dir();
    let _ = std::fs::create_dir_all(&config_dir);
    if let Ok(json_str) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(get_config_file(), json_str);
    }
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "medal_clip".to_string();
    }

    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let cleaned: String = trimmed
        .chars()
        .filter(|c| !invalid_chars.contains(c) && !c.is_control())
        .collect();

    let cleaned = cleaned.trim_matches(|c| c == '.' || c == ' ');
    if cleaned.is_empty() {
        return "medal_clip".to_string();
    }

    if cleaned.len() > 150 {
        let truncated: String = cleaned.chars().take(150).collect();
        let trimmed = truncated.trim_matches(|c| c == '.' || c == ' ');
        if trimmed.is_empty() {
            "medal_clip".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        cleaned.to_string()
    }
}

fn fetch_clip_info(raw_input: &str) -> Result<(String, String), String> {
    let mut url = raw_input.trim().to_string();
    if url.is_empty() {
        return Err("Please enter a valid Medal clip URL or ID.".to_string());
    }

    if !url.contains("medal") {
        if !url.contains('/') {
            url = format!("https://medal.tv/clips/{}", url);
        } else {
            return Err("Invalid Medal clip URL or ID.".to_string());
        }
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{}", url);
    }

    let clean_url = url
        .replace("?theater=true", "")
        .replace("&theater=true", "");

    let clip_re = Regex::new(r"/clips/([a-zA-Z0-9_\-]+)").unwrap();
    let clip_id = clip_re
        .captures(&clean_url)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let mut title: Option<String> = None;
    let mut file_url: Option<String> = None;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to initialize network client: {e}"))?;

    // 1. Attempt Medal Content API fetch
    if let Some(ref id) = clip_id {
        let api_url = format!("https://medal.tv/api/content/{}", id);
        if let Ok(res) = client
            .get(&api_url)
            .header("Accept", "application/json")
            .send()
        {
            if res.status().is_success() {
                if let Ok(json_val) = res.json::<serde_json::Value>() {
                    if let Some(t) = json_val
                        .get("contentTitle")
                        .or_else(|| json_val.get("videoTitle"))
                        .and_then(|v| v.as_str())
                    {
                        title = Some(t.to_string());
                    }
                    if let Some(u) = json_val.get("contentUrl").and_then(|v| v.as_str()) {
                        file_url = Some(u.to_string());
                    }
                }
            }
        }
    }

    // 2. Fallback to HTML webpage scraping
    if file_url.is_none() || title.is_none() {
        if let Ok(res) = client.get(&clean_url).send() {
            if let Ok(html_text) = res.text() {
                if file_url.is_none() {
                    let file_re = Regex::new(r#""contentUrl":"([^"]+)""#).unwrap();
                    if let Some(caps) = file_re.captures(&html_text) {
                        file_url = Some(caps[1].to_string());
                    }
                }

                if title.is_none() {
                    let title_re =
                        Regex::new(r#""(?:contentTitle|videoTitle)":\s*"([^"]+)""#).unwrap();
                    if let Some(caps) = title_re.captures(&html_text) {
                        title = Some(caps[1].to_string());
                    }
                }

                if title.is_none() {
                    let og_re = Regex::new(
                        r#"<meta[^>]*property=["']og:title["'][^>]*content=["'](.*?)["']"#,
                    )
                    .unwrap();
                    if let Some(caps) = og_re.captures(&html_text) {
                        let raw_title = html_escape::decode_html_entities(&caps[1]).to_string();
                        let clean_title = Regex::new(r"(?i)\s*-\s*Clipped.*with\s*Medal\.tv.*$")
                            .unwrap()
                            .replace(&raw_title, "")
                            .to_string();
                        let clean_title = Regex::new(r"(?i)\s*\|\s*Medal\.tv.*$")
                            .unwrap()
                            .replace(&clean_title, "")
                            .to_string();
                        title = Some(clean_title.trim().to_string());
                    }
                }
            }
        }
    }

    let download_url = file_url.ok_or_else(|| {
        "Could not find a downloadable video stream for this Medal clip. Please check the URL."
            .to_string()
    })?;

    let final_title = sanitize_filename(&title.unwrap_or_else(|| "medal_clip".to_string()));
    Ok((final_title, download_url))
}

fn run_download(
    url_input: String,
    tx: Sender<DownloadMessage>,
    stop_signal: Arc<AtomicBool>,
) {
    let (clip_title, download_url) = match fetch_clip_info(&url_input) {
        Ok(info) => info,
        Err(e) => {
            let _ = tx.send(DownloadMessage::Error(e));
            return;
        }
    };

    let last_dir = load_last_directory();
    let default_name = format!("{}.mp4", clip_title);

    let save_path = rfd::FileDialog::new()
        .set_directory(&last_dir)
        .set_file_name(&default_name)
        .add_filter("Video Files", &["mp4"])
        .add_filter("All Files", &["*"])
        .save_file();

    let save_path = match save_path {
        Some(p) => p,
        None => {
            // User cancelled file dialog
            let _ = tx.send(DownloadMessage::Progress(0.0));
            return;
        }
    };

    if let Some(parent) = save_path.parent() {
        save_last_directory(parent);
    }

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(DownloadMessage::Error(format!("Failed to build client: {e}")));
            return;
        }
    };

    let mut response = match client.get(&download_url).send() {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(DownloadMessage::Error(format!("Download request failed: {e}")));
            return;
        }
    };

    if !response.status().is_success() {
        let _ = tx.send(DownloadMessage::Error(format!(
            "Server returned error status: {}",
            response.status()
        )));
        return;
    }

    let total_bytes = response.content_length();
    let mut file = match File::create(&save_path) {
        Ok(f) => f,
        Err(e) => {
            let _ = tx.send(DownloadMessage::Error(format!("Failed to create destination file: {e}")));
            return;
        }
    };

    let mut buffer = [0u8; 1024 * 64]; // 64 KB buffer
    let mut bytes_written: u64 = 0;

    loop {
        if stop_signal.load(Ordering::Relaxed) {
            let _ = tx.send(DownloadMessage::Error("Download stopped".to_string()));
            return;
        }

        match response.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(e) = file.write_all(&buffer[..n]) {
                    let _ = tx.send(DownloadMessage::Error(format!("Write error: {e}")));
                    return;
                }
                bytes_written += n as u64;

                if let Some(total) = total_bytes {
                    if total > 0 {
                        let pct = (bytes_written as f32) / (total as f32);
                        let _ = tx.send(DownloadMessage::Progress(pct.min(1.0)));
                    }
                } else {
                    let _ = tx.send(DownloadMessage::Progress(0.5));
                }
            }
            Err(e) => {
                let _ = tx.send(DownloadMessage::Error(format!("Stream reading error: {e}")));
                return;
            }
        }
    }

    let _ = tx.send(DownloadMessage::Progress(1.0));
    let _ = tx.send(DownloadMessage::Success(save_path.to_string_lossy().to_string()));
}

impl eframe::App for ClipDownloaderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll messages from background download thread
        if let Some(ref rx) = self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    DownloadMessage::Progress(p) => {
                        self.progress = p;
                        if p == 0.0 && self.is_downloading {
                            // Cancelled
                            self.is_downloading = false;
                        }
                    }
                    DownloadMessage::Success(saved_path) => {
                        self.is_downloading = false;
                        self.progress = 1.0;
                        rfd::MessageDialog::new()
                            .set_title("Success")
                            .set_description(&format!("File saved successfully:\n{}", saved_path))
                            .set_buttons(rfd::MessageButtons::Ok)
                            .show();
                    }
                    DownloadMessage::Error(err_msg) => {
                        self.is_downloading = false;
                        self.progress = 0.0;
                        rfd::MessageDialog::new()
                            .set_title("Error")
                            .set_description(&err_msg)
                            .set_buttons(rfd::MessageButtons::Ok)
                            .show();
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            ui.add_space(5.0);

            // URL input row
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Enter the URL of the clip:").size(13.0));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.url_input)
                        .hint_text("Paste clip URL or ID...")
                        .desired_width(190.0),
                );
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.is_downloading {
                    self.start_download(ctx);
                }
            });

            ui.add_space(8.0);

            // Progress bar
            let progress_bar = egui::ProgressBar::new(self.progress)
                .show_percentage()
                .animate(self.is_downloading);
            ui.add_sized([ui.available_width(), 20.0], progress_bar);

            ui.add_space(10.0);

            // Buttons centered
            ui.vertical_centered(|ui| {
                let download_btn = egui::Button::new(
                    egui::RichText::new("Download Video").color(egui::Color32::WHITE),
                )
                .min_size(egui::vec2(180.0, 28.0))
                .fill(egui::Color32::from_rgb(0x1f, 0x53, 0x8d));

                if ui.add_enabled(!self.is_downloading, download_btn).clicked() {
                    self.start_download(ctx);
                }

                ui.add_space(4.0);

                let exit_btn = egui::Button::new("Exit")
                    .min_size(egui::vec2(180.0, 26.0))
                    .fill(egui::Color32::from_rgb(0x42, 0x42, 0x42));

                if ui.add(exit_btn).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });

        if self.is_downloading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

impl ClipDownloaderApp {
    fn start_download(&mut self, ctx: &egui::Context) {
        let input = self.url_input.trim().to_string();
        if input.is_empty() {
            rfd::MessageDialog::new()
                .set_title("Error")
                .set_description("Please enter a valid Medal clip URL or ID.")
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
        }

        self.is_downloading = true;
        self.progress = 0.0;
        self.status_text = "Fetching clip info...".to_string();

        let (tx, rx) = channel();
        self.rx = Some(rx);

        let stop = Arc::new(AtomicBool::new(false));
        self.stop_signal = Arc::clone(&stop);

        let ctx_clone = ctx.clone();
        thread::spawn(move || {
            run_download(input, tx, stop);
            ctx_clone.request_repaint();
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 200.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_title("Clip Downloader :)"),
        ..Default::default()
    };

    eframe::run_native(
        "Clip Downloader :)",
        native_options,
        Box::new(|cc| {
            // Apply matching dark theme (#302f2f)
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(0x30, 0x2f, 0x2f);
            visuals.window_fill = egui::Color32::from_rgb(0x30, 0x2f, 0x2f);
            visuals.extreme_bg_color = egui::Color32::from_rgb(0x24, 0x24, 0x24);
            cc.egui_ctx.set_visuals(visuals);

            Ok(Box::new(ClipDownloaderApp::default()))
        }),
    )
}
