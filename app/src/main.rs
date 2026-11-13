#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
mod account;
mod audio;
mod camera;
mod capture;
mod discovery;
mod encryption;
mod feedback;
mod guest;
mod home_logo;
mod icons;
mod participants;
mod protocol;
mod session;
mod smoke;
mod sounds;
mod source_preview;
mod ui;
mod updates;

use gpui::{prelude::*, *};
use std::borrow::Cow;

struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "logo.svg" => Some(Cow::Borrowed(include_bytes!("../assets/logo.svg"))),
            "icon.svg" => Some(Cow::Borrowed(include_bytes!("../assets/icon.svg"))),
            _ => icons::load(path).map(Cow::Owned),
        })
    }
    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(["logo.svg", "icon.svg"]
            .into_iter()
            .chain(icons::PATHS.iter().copied())
            .map(Into::into)
            .collect())
    }
}
actions!(telinha, [Quit]);

fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--apply-update") {
        return updates::worker_main();
    }
    if std::env::args().nth(1).as_deref() == Some("--source-preview") {
        return source_preview::worker_main();
    }
    if std::env::args().nth(1).as_deref() == Some("--camera-worker") {
        return camera::worker_main();
    }
    if std::env::args().nth(1).as_deref() == Some("--camera-probe") {
        return camera::probe();
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "telinha=info".into()),
        )
        .init();
    if std::env::args().any(|arg| arg == "--media-smoke") {
        return smoke::run();
    }
    if std::env::args().any(|arg| arg == "--viewer-smoke") {
        return smoke::viewer();
    }
    if std::env::args().any(|arg| arg == "--check-fonts") {
        let fonts = Application::new().text_system().all_font_names();
        anyhow::ensure!(
            !fonts.is_empty(),
            "O sistema de texto não carregou nenhuma fonte."
        );
        println!(
            "Sistema de texto pronto: {} famílias de fontes.",
            fonts.len()
        );
        return Ok(());
    }
    if let Err(error) = protocol::register() {
        tracing::warn!("Não deu pra registrar os links da telinha: {error}");
    }
    let initial = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));
    let server =
        std::env::var("TELINHA_SERVER_URL").unwrap_or_else(|_| "https://telinha.tech".into());
    let backend = session::Backend::start(server);
    let (links, link_input) = async_channel::unbounded();
    let application = Application::new().with_assets(Assets);
    application.on_open_urls(move |urls| {
        for url in urls {
            let _ = links.try_send(url);
        }
    });
    application.run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1120.), px(700.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(600.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("telinha".into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| ui::Telinha::new(backend, link_input, initial, cx));
                window.focus(&view.focus_handle(cx));
                view
            },
        )
        .expect("janela da telinha");
        cx.activate(true);
    });
    Ok(())
}
