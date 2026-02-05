use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell as layer_shell;
use gtk4_layer_shell::LayerShell;

fn default_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("voicetext.sock");
    }
    PathBuf::from("/tmp/voicetext.sock")
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

fn socket_arg() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--socket" {
            if let Some(value) = args.next() {
                return PathBuf::from(value);
            }
        }
    }
    default_socket_path()
}

fn main() {
    let socket_path = socket_arg();
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

    app.run();
}
