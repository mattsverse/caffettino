//! The native adapter Rune currently lacks on macOS. All methods run on the UI thread.
use crate::{CoffeeBreak, State};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAccessibility, NSMenu, NSStatusItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use rune::AppHandle;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

struct MenuActivationIvars {
    menu: Retained<NSMenu>,
    status_item: Retained<NSStatusItem>,
}

define_class!(
    // NSObject has no subclassing requirements. AppKit calls this on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuActivationIvars]
    struct MenuActivation;

    impl MenuActivation {
        #[unsafe(method(openMenu:))]
        fn open_menu(&self, _sender: Option<&AnyObject>) {
            let ivars = self.ivars();
            if let Some(button) = ivars.status_item.button(self.mtm()) {
                // Retain native objects directly: don't hold tray-icon's RefCell borrow
                // across the menu's nested event loop, which can deliver app updates.
                ivars.status_item.setMenu(Some(&ivars.menu));
                // With a menu attached, AppKit tracks that menu instead of sending the
                // button's target/action. The button and menu are valid on this thread.
                unsafe { button.performClick(None) };
                ivars.status_item.setMenu(None);
            }
        }
    }
);

pub struct CoffeeTray {
    icon: TrayIcon,
    status: MenuItem,
    toggle: MenuItem,
    // NSControl's target is weak; keep it alive as long as the tray exists.
    _activation: Retained<MenuActivation>,
}

impl CoffeeTray {
    pub fn new(handle: AppHandle<State>) -> Result<Self, Box<dyn std::error::Error>> {
        let menu = Menu::new();
        let status = MenuItem::new("Caffettino is resting", false, None);
        let toggle = MenuItem::with_id("toggle", "Brew a coffee", true, None);
        let show = MenuItem::with_id("show", "Open Caffettino…", true, None);
        let quit = MenuItem::with_id("quit", "Quit Caffettino", true, None);
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &toggle,
            &show,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        let mtm = MainThreadMarker::new().ok_or("tray creation must run on the main thread")?;
        // ContextMenu::ns_menu returns the live NSMenu owned by this Menu. Retain it
        // before moving the Menu into the tray so native activation can use it safely.
        let native_menu = unsafe { Retained::retain(menu.ns_menu().cast::<NSMenu>()) }
            .ok_or("the tray menu is unavailable")?;
        let icon = TrayIconBuilder::new()
            .with_id("caffettino")
            .with_icon_templated(cup_icon(false))
            .with_tooltip("Caffettino • Resting\nClick to brew. Right-click for the menu.")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        let status_item = icon
            .ns_status_item()
            .ok_or("the native status item is unavailable")?;
        let button = status_item
            .button(mtm)
            .ok_or("the native status button is unavailable")?;
        let activation = unsafe {
            let allocated = MenuActivation::alloc(mtm).set_ivars(MenuActivationIvars {
                menu: native_menu,
                status_item,
            });
            let target: Retained<MenuActivation> = msg_send![super(allocated), init];
            // The selector above has the NSControl action signature and target is retained
            // by CoffeeTray. The library's mouse overlay keeps handling physical clicks.
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(openMenu:)));
            target
        };
        button.setAccessibilityLabel(Some(&NSString::from_str("Caffettino")));
        button.setAccessibilityHelp(Some(&NSString::from_str("Open the coffee break menu")));
        let click_handle = handle.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                click_handle.update(State::toggle);
            }
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
            "toggle" => handle.update(State::toggle),
            "show" => handle.open(),
            "quit" => handle.quit(),
            _ => {}
        }));
        Ok(Self {
            icon,
            status,
            toggle,
            _activation: activation,
        })
    }

    pub fn update(&self, active: bool, coffee_break: CoffeeBreak) {
        self.status.set_text(if active {
            "Your Mac is wide awake"
        } else {
            "Caffettino is resting"
        });
        self.toggle.set_text(if active {
            "Let it snooze"
        } else {
            "Brew a coffee"
        });
        let state = if active { "Awake" } else { "Resting" };
        let _ = self.icon.set_tooltip(Some(format!(
            "Caffettino • {state} • {}\nClick to toggle. Right-click for the menu.",
            coffee_break.label()
        )));
        let _ = self.icon.set_icon_templated(Some(cup_icon(active)));
    }
}

/// A 2× template icon, drawn in code so it follows the menu bar's light/dark tint.
fn cup_icon(active: bool) -> Icon {
    let mut lines = vec![
        ([4.0, 9.0], [4.8, 17.0]),
        ([4.8, 17.0], [14.0, 17.0]),
        ([14.0, 17.0], [14.8, 9.0]),
        ([14.8, 9.0], [4.0, 9.0]),
        ([15.0, 10.0], [18.0, 10.5]),
        ([18.0, 10.5], [18.5, 13.0]),
        ([18.5, 13.0], [14.5, 15.0]),
        ([3.0, 19.5], [16.0, 19.5]),
    ];
    if active {
        lines.extend([
            ([7.0, 6.5], [6.0, 4.5]),
            ([6.0, 4.5], [7.0, 2.5]),
            ([11.5, 6.5], [10.5, 4.5]),
            ([10.5, 4.5], [11.5, 2.5]),
        ]);
    }
    let mut pixels = Vec::with_capacity(44 * 44 * 4);
    for y in 0_u8..44 {
        for x in 0_u8..44 {
            let p = [f32::from(x) / 2.0 + 0.25, f32::from(y) / 2.0 + 0.25];
            let distance = lines
                .iter()
                .map(|&(a, b)| {
                    let delta = [b[0] - a[0], b[1] - a[1]];
                    let t = (((p[0] - a[0]) * delta[0] + (p[1] - a[1]) * delta[1])
                        / (delta[0] * delta[0] + delta[1] * delta[1]))
                        .clamp(0.0, 1.0);
                    (p[0] - a[0] - t * delta[0]).hypot(p[1] - a[1] - t * delta[1])
                })
                .fold(f32::INFINITY, f32::min);
            let alpha = ((1.0 - distance) * 2.0).clamp(0.0, 1.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "Alpha is clamped to 0..=1; truncation quantizes it to a pixel byte"
            )]
            let alpha_byte = (alpha * 255.0) as u8;
            pixels.extend_from_slice(&[0, 0, 0, alpha_byte]);
        }
    }
    Icon::from_rgba(pixels, 44, 44).expect("fixed-size cup pixels")
}
