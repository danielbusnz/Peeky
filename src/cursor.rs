use gtk::gdk::Display;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, CssProvider, Label, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

const APP_ID: &str = "com.tabby.cursor-mvp";

pub fn cursor(x: i32, y: i32) -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| {
        let provider = CssProvider::new();
        provider.load_from_data(
            "window { background: transparent; }
             label { color: #d97757; font-size: 24px; }",
        );
        gtk::style_context_add_provider_for_display(
            &Display::default().expect("could not connect to a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });

    app.connect_activate(move |app| {
        let window = ApplicationWindow::builder()
            .application(app)
            .default_width(40)
            .default_height(40)
            .build();

        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_margin(Edge::Top, y);
        window.set_margin(Edge::Left, x);

        let label = Label::new(Some("●"));
        window.set_child(Some(&label));

        window.connect_realize(|window| {
            if let Some(surface) = window.surface() {
                let empty_region = gtk::cairo::Region::create();
                surface.set_input_region(Some(&empty_region));
            }
        });

        window.present();
        println!("[gtk] cursor window presented");
    });

    app.run()
}
