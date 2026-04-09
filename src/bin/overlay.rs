use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell as layer_shell;
use gtk4_layer_shell::LayerShell;

#[derive(Debug, Clone)]
struct Args {
    socket_path: PathBuf,
    foreground: bool,
}

fn default_socket_path() -> PathBuf {
    core_ipc::default_socket_path()
}

fn print_usage() {
    println!(
        r#"Usage:
  voice-cmd-overlay [--fg] [--socket <path>]

Options:
  --fg             Run in foreground (default: daemonized)
  --socket <path>  IPC socket path
  -h, --help       Show this help"#
    );
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut socket_path = default_socket_path();
    let mut foreground = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                if let Some(value) = args.next() {
                    socket_path = PathBuf::from(value);
                }
            }
            "--fg" => foreground = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {}
        }
    }

    Args {
        socket_path,
        foreground,
    }
}

fn daemonize(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    let log_path = core_logging::overlay_log_path();

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--fg");
    cmd.arg("--socket");
    cmd.arg(&args.socket_path);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(log_file.try_clone()?));
    cmd.stderr(Stdio::from(log_file));

    // Start overlay in a detached session so it survives caller exit.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    cmd.spawn()?;
    println!(
        "overlay started in background (logs at {})",
        log_path.display()
    );
    Ok(())
}

fn read_status(socket: &PathBuf) -> (bool, f32) {
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(mut stream) => {
            let _ = stream.write_all(b"STATUS\n");
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut stream, &mut buf);
            let recording = buf.contains("recording=true");
            let energy = buf
                .split("energy=")
                .last()
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0);
            (recording, energy)
        }
        Err(err) => {
            eprintln!("overlay: status read failed: {err}");
            (false, 0.0)
        }
    }
}

fn run_foreground(socket_path: PathBuf) {
    eprintln!("overlay: starting with socket={}", socket_path.display());

    let app = gtk::Application::builder()
        .application_id("io.voice_cmd.overlay")
        .build();

    app.connect_activate(move |app| {
        eprintln!("overlay: gtk application activated");
        if let Some(display) = gtk::gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(
                r#"
window.voice-cmd-overlay {
  background-color: transparent;
  box-shadow: none;
}
box.voice-cmd-overlay {
  background-color: transparent;
}
drawingarea.voice-cmd-overlay {
  background-color: transparent;
}
"#,
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Voice Cmd Overlay")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("voice-cmd-overlay");

        if !layer_shell::is_supported() {
            eprintln!("overlay: gtk4-layer-shell not supported by compositor/session");
        } else {
            eprintln!("overlay: layer-shell is supported");
        }

        window.init_layer_shell();
        window.set_layer(layer_shell::Layer::Top);
        window.set_anchor(layer_shell::Edge::Top, true);
        window.set_anchor(layer_shell::Edge::Left, false);
        window.set_anchor(layer_shell::Edge::Right, false);
        window.set_margin(layer_shell::Edge::Top, 8);
        window.set_margin(layer_shell::Edge::Left, 0);
        window.set_margin(layer_shell::Edge::Right, 0);
        window.set_keyboard_mode(layer_shell::KeyboardMode::None);

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.add_css_class("voice-cmd-overlay");
        container.set_halign(gtk::Align::Center);
        container.set_valign(gtk::Align::Start);
        container.set_hexpand(true);
        container.set_vexpand(false);

        let waveform_area = gtk::DrawingArea::new();
        waveform_area.add_css_class("voice-cmd-overlay");
        waveform_area.set_content_width(120);
        waveform_area.set_content_height(48);

        let energy_history = std::sync::Arc::new(std::sync::Mutex::new(vec![0.0f32; 7]));

        let history_ptr = energy_history.clone();
        waveform_area.set_draw_func(move |_, cr, width, height| {
            let w = width as f64;
            let h = height as f64;

            // Draw pill background
            let radius = h / 2.0;
            cr.set_source_rgba(0.1, 0.1, 0.1, 0.7);
            cr.new_sub_path();
            cr.arc(radius, radius, radius, 0.5 * std::f64::consts::PI, 1.5 * std::f64::consts::PI);
            cr.arc(w - radius, radius, radius, 1.5 * std::f64::consts::PI, 0.5 * std::f64::consts::PI);
            cr.close_path();
            let _ = cr.fill();

            // Draw pill border
            cr.set_source_rgba(1.0, 0.2, 0.2, 0.8);
            cr.set_line_width(2.0);
            cr.new_sub_path();
            cr.arc(radius, radius, radius, 0.5 * std::f64::consts::PI, 1.5 * std::f64::consts::PI);
            cr.arc(w - radius, radius, radius, 1.5 * std::f64::consts::PI, 0.5 * std::f64::consts::PI);
            cr.close_path();
            let _ = cr.stroke();

            // Draw bars
            let history = history_ptr.lock().unwrap();
            let num_bars = history.len();
            let bar_spacing = 6.0;
            let bar_width = 4.0;
            let total_bars_width = (num_bars as f64 * bar_width) + ((num_bars - 1) as f64 * bar_spacing);
            let start_x = (w - total_bars_width) / 2.0;

            for (i, &energy) in history.iter().enumerate() {
                let x = start_x + (i as f64 * (bar_width + bar_spacing));
                // Scale energy for visibility. RMS is typically small.
                let min_height = 4.0;
                let max_height = h - 16.0;
                let bar_h = (min_height + (energy as f64 * 150.0) * max_height).min(max_height);
                let y = (h - bar_h) / 2.0;

                cr.set_source_rgba(1.0, 0.3, 0.3, 0.9);
                // Rounded rectangle for each bar
                let r = bar_width / 2.0;
                cr.new_sub_path();
                cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
                cr.arc(x + bar_width - r, y + r, r, 1.5 * std::f64::consts::PI, 2.0 * std::f64::consts::PI);
                cr.arc(x + bar_width - r, y + bar_h - r, r, 0.0, 0.5 * std::f64::consts::PI);
                cr.arc(x + r, y + bar_h - r, r, 0.5 * std::f64::consts::PI, std::f64::consts::PI);
                cr.close_path();
                let _ = cr.fill();
            }
        });

        container.append(&waveform_area);
        window.set_child(Some(&container));
        window.show();
        eprintln!("overlay: window initialized and shown once");

        let socket_path = socket_path.clone();
        let window = window.clone();
        let mut last_recording = true;
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let (recording, energy) = read_status(&socket_path);
            if recording != last_recording {
                eprintln!("overlay: recording state changed -> {recording}");
                last_recording = recording;
            }

            {
                let mut history = energy_history.lock().unwrap();
                history.remove(0);
                history.push(energy);
            }
            waveform_area.queue_draw();

            if recording {
                window.show();
            } else {
                window.hide();
            }
            glib::ControlFlow::Continue
        });
    });

    app.run_with_args(&[] as &[&str]);
}

fn main() {
    let args = parse_args();
    if !args.foreground {
        if let Err(err) = daemonize(&args) {
            eprintln!("overlay: failed to daemonize: {err}");
            std::process::exit(1);
        }
        return;
    }

    run_foreground(args.socket_path);
}
