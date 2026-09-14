//! The jukebox's only presence on a Windows or macOS desktop: a tray icon — a menu-bar icon
//! on macOS, with no Dock icon — and the dialogs it needs. Mirrors src/main/tray.ts, without
//! the window: the console opens in the default browser.

use std::sync::Arc;
use std::time::{Duration, Instant};

use jukebox_server::net::{FolderPicker, Network};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const APP_NAME: &str = "Kyylan Jukebox";
/// How often the guest links are checked against the machine's current addresses — a
/// laptop moving between networks.
const REFRESH_LINKS_EVERY: Duration = Duration::from_secs(30);

/// The console: the web UI, from the host, so it can finish setup and use the folder picker.
pub fn console_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

pub fn open_console(port: u16) {
    if let Err(err) = open::that_detached(console_url(port)) {
        tracing::warn!(%err, "couldn't open the console in a browser");
    }
}

/// An error before the tray exists, as Electron's `dialog.showErrorBox` showed it.
pub fn error_dialog(message: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(APP_NAME)
        .set_description(message)
        .show();
}

/// The host's "choose a music folder" dialog.
pub struct DialogFolderPicker;

impl FolderPicker for DialogFolderPicker {
    fn pick_folder(&self) -> Option<String> {
        // The asynchronous dialog runs on the main thread's event loop, where macOS needs it,
        // while this request's thread waits for the answer.
        let picked = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Choose a music folder")
                .set_can_create_directories(true)
                .pick_folder(),
        )?;
        Some(picked.path().to_string_lossy().into_owned())
    }
}

/// The links guests open, one per LAN address.
pub fn guest_links(addresses: &[String], port: u16) -> Vec<String> {
    addresses
        .iter()
        .map(|ip| format!("http://{ip}:{port}"))
        .collect()
}

struct TrayMenu {
    menu: Menu,
    open: MenuId,
    quit: MenuId,
    links: Vec<(MenuId, String)>,
}

fn build_menu(links: &[String]) -> TrayMenu {
    let menu = Menu::new();
    let open = MenuItem::new("Open jukebox console", true, None);
    let quit = MenuItem::new(format!("Quit {APP_NAME}"), true, None);
    let mut copy_items = Vec::new();
    let append = |item: &dyn tray_icon::menu::IsMenuItem| {
        menu.append(item).expect("building the tray menu");
    };
    append(&MenuItem::new(APP_NAME, false, None));
    append(&PredefinedMenuItem::separator());
    append(&open);
    append(&PredefinedMenuItem::separator());
    append(&MenuItem::new("Guests can join at", false, None));
    if links.is_empty() {
        append(&MenuItem::new("No network address found", false, None));
    }
    for link in links {
        let item = MenuItem::new(format!("Copy guest link — {link}"), true, None);
        append(&item);
        copy_items.push((item.id().clone(), link.clone()));
    }
    append(&PredefinedMenuItem::separator());
    append(&quit);
    TrayMenu {
        open: open.id().clone(),
        quit: quit.id().clone(),
        links: copy_items,
        menu,
    }
}

fn icon() -> Icon {
    // The template image macOS tints for light and dark menu bars; at twice the size there,
    // for Retina displays.
    let bytes: &[u8] = if cfg!(target_os = "macos") {
        include_bytes!("../../../build/tray@2x.png")
    } else {
        include_bytes!("../../../build/tray.png")
    };
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().expect("the tray icon is a PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("the tray icon's size")];
    let info = reader.next_frame(&mut rgba).expect("the tray icon decodes");
    rgba.truncate(info.buffer_size());
    Icon::from_rgba(rgba, info.width, info.height).expect("the tray icon is RGBA")
}

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

pub struct Desktop {
    pub port: u16,
    pub network: Arc<dyn Network>,
    /// Setup isn't done yet: open the console so the host can do it.
    pub first_run: bool,
    /// Runs when the jukebox is quit from the menu, before the process ends.
    pub on_quit: Box<dyn FnOnce()>,
}

/// Runs the tray on this thread — the main thread, which macOS requires — until Quit.
pub fn run(desktop: Desktop) -> ! {
    // Changed only on macOS, to hide the Dock icon.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Tray(event));
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let Desktop {
        port,
        network,
        first_run,
        on_quit,
    } = desktop;
    let mut on_quit = Some(on_quit);
    let mut tray: Option<TrayIcon> = None;
    let mut menu: Option<TrayMenu> = None;
    let mut links = Vec::new();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + REFRESH_LINKS_EVERY);
        match event {
            // The earliest the tray may be created on macOS.
            Event::NewEvents(StartCause::Init) => {
                links = guest_links(&network.lan_addresses(), port);
                let built = build_menu(&links);
                tray = Some(
                    TrayIconBuilder::new()
                        .with_icon(icon())
                        .with_icon_as_template(cfg!(target_os = "macos"))
                        .with_tooltip(APP_NAME)
                        .with_menu(Box::new(built.menu.clone()))
                        // macOS menu-bar icons open their menu on a click; on Windows a
                        // click opens the console and the menu is on the right button.
                        .with_menu_on_left_click(cfg!(target_os = "macos"))
                        .build()
                        .expect("creating the tray icon"),
                );
                menu = Some(built);
                if first_run {
                    open_console(port);
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let current = guest_links(&network.lan_addresses(), port);
                if current != links {
                    links = current;
                    let built = build_menu(&links);
                    if let Some(tray) = &tray {
                        tray.set_menu(Some(Box::new(built.menu.clone())));
                    }
                    menu = Some(built);
                }
            }
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            })) if cfg!(windows) => open_console(port),
            Event::UserEvent(UserEvent::Menu(event)) => {
                let Some(menu) = &menu else { return };
                if event.id == menu.open {
                    open_console(port);
                } else if event.id == menu.quit {
                    tray.take();
                    if let Some(quit) = on_quit.take() {
                        quit();
                    }
                    *control_flow = ControlFlow::Exit;
                } else if let Some((_, link)) = menu.links.iter().find(|(id, _)| *id == event.id) {
                    match arboard::Clipboard::new().and_then(|mut c| c.set_text(link.clone())) {
                        Ok(()) => {}
                        Err(err) => tracing::warn!(%err, "couldn't copy the guest link"),
                    }
                }
            }
            _ => {}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_links_use_every_address_and_the_running_port() {
        assert_eq!(
            guest_links(&["192.168.1.20".into(), "10.0.0.5".into()], 8080),
            ["http://192.168.1.20:8080", "http://10.0.0.5:8080"]
        );
        assert!(guest_links(&[], 8080).is_empty());
        assert_eq!(console_url(8094), "http://127.0.0.1:8094/");
    }
}
