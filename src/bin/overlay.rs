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
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("voicetext.sock");
    }
    PathBuf::from("/tmp/voicetext.sock")
}

fn print_usage() {
    println!("Usage:");
    println!("  voicetext-overlay [--fg] [--socket <path>]");
    println!();
    println!("Options:");
    println!("  --fg             Run in foreground (default: daemonized)");
    println!("  --socket <path>  IPC socket path");
    println!("  -h, --help       Show this help");
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
    let log_path = directories::ProjectDirs::from("io", "voicetext", "voicetext")
        .and_then(|proj| proj.state_dir().map(|dir| dir.join("overlay.log")))
        .unwrap_or_else(|| PathBuf::from("/tmp/voicetext-overlay.log"));

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

fn read_status(socket: &PathBuf) -> bool {
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(mut stream) => {
            let _ = stream.write_all(b"STATUS\n");
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut stream, &mut buf);
            eprintln!("overlay: status response='{}'", buf.trim());
            buf.contains("recording=true")
        }
        Err(err) => {
            eprintln!("overlay: status read failed: {err}");
            false
        }
    }
}

fn run_foreground(socket_path: PathBuf) {
    eprintln!("overlay: starting with socket={}", socket_path.display());

    let app = gtk::Application::builder()
        .application_id("io.voicetext.overlay")
        .build();

    app.connect_activate(move |app| {
        eprintln!("overlay: gtk application activated");
        if let Some(display) = gtk::gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(
                "
window.voicetext-overlay {
  background-color: transparent;
  box-shadow: none;
}
box.voicetext-overlay {
  background-color: transparent;
}
drawingarea.voicetext-overlay {
  background-color: transparent;
}
",
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Voicetext Overlay")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("voicetext-overlay");

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
        container.add_css_class("voicetext-overlay");
        container.set_halign(gtk::Align::Center);
        container.set_valign(gtk::Align::Start);
        container.set_hexpand(true);
        container.set_vexpand(false);

        let circle = gtk::DrawingArea::new();
        circle.add_css_class("voicetext-overlay");
        circle.set_content_width(48);
        circle.set_content_height(48);
        circle.set_draw_func(|_, cr, width, height| {
            let radius = (width.min(height) as f64) / 2.0 - 1.0;
            cr.set_source_rgba(1.0, 0.15, 0.15, 0.45);
            cr.arc(
                width as f64 / 2.0,
                height as f64 / 2.0,
                radius,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.fill();

            cr.set_source_rgba(1.0, 0.4, 0.4, 0.7);
            cr.set_line_width(2.0);
            cr.arc(
                width as f64 / 2.0,
                height as f64 / 2.0,
                radius,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.stroke();
        });

        container.append(&circle);
        window.set_child(Some(&container));
        window.show();
        eprintln!("overlay: window initialized and shown once");

        let socket_path = socket_path.clone();
        let window = window.clone();
        let mut last_recording = true;
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let recording = read_status(&socket_path);
            if recording != last_recording {
                eprintln!("overlay: recording state changed -> {recording}");
                last_recording = recording;
            }
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
