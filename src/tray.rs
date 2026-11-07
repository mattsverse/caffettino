//! The native adapter Rune currently lacks on macOS. All methods run on the UI thread.
use crate::{CoffeeBreak, Selection, State};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage, NSMenu, NSStatusBarButton,
    NSStatusItem, NSWindowDidChangeOcclusionStateNotification, NSWindowOcclusionState, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSInteger, NSNotificationCenter, NSObject, NSSize, NSString, NSTimer,
};
use rune::AppHandle;
use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
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

/// Steam frames per loop, and how long each stays on screen.
const STEAM_FRAMES: u8 = 6;
const STEAM_FRAME_SECONDS: f64 = 0.25;

struct SteamIvars {
    button: Retained<NSStatusBarButton>,
    // Decoded once: swapping a ready NSImage skips tray-icon's PNG encode/decode per frame.
    idle: Retained<NSImage>,
    frames: Vec<Retained<NSImage>>,
    frame: Cell<usize>,
    brewing: Cell<bool>,
    // Runs only while brewing and visible; it retains `Steam` until invalidated.
    timer: RefCell<Option<Retained<NSTimer>>>,
}

define_class!(
    // NSObject has no subclassing requirements. NSTimer and the default notification
    // center call these on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SteamIvars]
    struct Steam;

    impl Steam {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            let ivars = self.ivars();
            let next = (ivars.frame.get() + 1) % ivars.frames.len();
            ivars.frame.set(next);
            ivars.button.setImage(Some(&ivars.frames[next]));
        }

        #[unsafe(method(occlusionChanged:))]
        fn occlusion_changed(&self, _notification: Option<&AnyObject>) {
            self.sync();
        }
    }
);

impl Steam {
    fn set_brewing(&self, brewing: bool) {
        let ivars = self.ivars();
        if ivars.brewing.replace(brewing) == brewing {
            return;
        }
        ivars.frame.set(0);
        ivars.button.setImage(Some(if brewing {
            &ivars.frames[0]
        } else {
            &ivars.idle
        }));
        self.sync();
    }

    /// Runs the timer only while it would change something on screen: not while the menu
    /// bar is hidden (full screen, auto-hide, behind the notch), nor with Reduce Motion.
    fn sync(&self) {
        let ivars = self.ivars();
        let visible = ivars.button.window().is_none_or(|window| {
            window
                .occlusionState()
                .contains(NSWindowOcclusionState::Visible)
        });
        let animate = ivars.brewing.get()
            && visible
            && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        let mut timer = ivars.timer.borrow_mut();
        match (animate, timer.is_some()) {
            (true, false) => {
                // `tick:` has the NSTimer action signature, and the timer retains its target.
                let scheduled = unsafe {
                    NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                        STEAM_FRAME_SECONDS,
                        self,
                        sel!(tick:),
                        None,
                        true,
                    )
                };
                // Let macOS coalesce wakeups; a few ms of jitter is invisible here.
                scheduled.setTolerance(STEAM_FRAME_SECONDS / 4.0);
                *timer = Some(scheduled);
            }
            (false, true) => {
                if let Some(timer) = timer.take() {
                    timer.invalidate();
                }
            }
            _ => {}
        }
    }
}

/// Menu position of the first break item: after the status line and its separator.
const FIRST_BREAK: usize = 2;
const BREAK_ID: &str = "break:";

/// One checkable break in the menu: its id suffix and title.
#[derive(Clone, PartialEq, Eq)]
struct BreakItem {
    id: String,
    title: String,
}

/// The breaks the menu lists, and which one is checked.
fn break_items(state: &State) -> (Vec<BreakItem>, Option<usize>) {
    let mut items: Vec<_> = state
        .presets
        .minutes()
        .iter()
        .map(|&minutes| CoffeeBreak::Timed(minutes))
        .chain([CoffeeBreak::Unlimited])
        .map(|coffee_break| BreakItem {
            id: break_id(coffee_break),
            title: coffee_break.label(),
        })
        .collect();
    let checked = match state.selection {
        Selection::Break(selected) => {
            let id = break_id(selected);
            items.iter().position(|item| item.id == id)
        }
        Selection::Custom => state.coffee_break().map(|custom| {
            items.push(BreakItem {
                id: "custom".into(),
                title: format!("{} (custom)", custom.label()),
            });
            items.len() - 1
        }),
    };
    (items, checked)
}

fn break_id(coffee_break: CoffeeBreak) -> String {
    match coffee_break {
        CoffeeBreak::Timed(minutes) => minutes.to_string(),
        CoffeeBreak::Unlimited => "unlimited".into(),
    }
}

fn select_from_menu(state: &mut State, id: &str) {
    match id {
        "unlimited" => state.select_break(CoffeeBreak::Unlimited),
        "custom" => state.select(Selection::Custom),
        minutes => {
            if let Ok(minutes) = minutes.parse() {
                state.select_break(CoffeeBreak::Timed(minutes));
            }
        }
    }
    // AppKit flips a check item when it's picked, even when the selection stays put.
    state.sync_tray();
}

/// Routes the cup's left-click and the menu's items to the app.
fn set_event_handlers(handle: AppHandle<State>) {
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
        id => {
            if let Some(id) = id.strip_prefix(BREAK_ID) {
                let id = id.to_owned();
                handle.update(move |state: &mut State, _: &mut rune::Tx| {
                    select_from_menu(state, &id);
                });
            }
        }
    }));
}

pub struct CoffeeTray {
    icon: TrayIcon,
    menu: Menu,
    status: MenuItem,
    toggle: MenuItem,
    breaks: RefCell<Vec<(BreakItem, CheckMenuItem)>>,
    steam: Retained<Steam>,
    // NSControl's target is weak; keep it alive as long as the tray exists.
    _activation: Retained<MenuActivation>,
}

impl Drop for CoffeeTray {
    fn drop(&mut self) {
        // The notification center doesn't retain observers, and the timer retains `steam`.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(&self.steam) };
        self.steam.set_brewing(false);
    }
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
        let idle_pixels = cup_pixels(None);
        let idle = template_image(&idle_pixels).ok_or("the cup image is unavailable")?;
        let icon = TrayIconBuilder::new()
            .with_id("caffettino")
            .with_icon_templated(Icon::from_rgba(idle_pixels, ICON_PIXELS, ICON_PIXELS)?)
            .with_tooltip("Caffettino • Resting\nClick to brew. Right-click for the menu.")
            .with_menu(Box::new(menu.clone()))
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
        set_event_handlers(handle);
        let frames = (0..STEAM_FRAMES)
            .map(|frame| {
                let phase = f32::from(frame) / f32::from(STEAM_FRAMES);
                template_image(&cup_pixels(Some(phase)))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("the steam images are unavailable")?;
        let window = button.window();
        let steam = Steam::alloc(mtm).set_ivars(SteamIvars {
            button,
            idle,
            frames,
            frame: Cell::new(0),
            brewing: Cell::new(false),
            timer: RefCell::new(None),
        });
        let steam: Retained<Steam> = unsafe { msg_send![super(steam), init] };
        if let Some(window) = window {
            // `occlusionChanged:` takes the notification, and Drop removes the observer.
            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    &steam,
                    sel!(occlusionChanged:),
                    Some(NSWindowDidChangeOcclusionStateNotification),
                    Some(&window),
                );
            }
        }
        Ok(Self {
            icon,
            menu,
            status,
            toggle,
            breaks: RefCell::new(Vec::new()),
            steam,
            _activation: activation,
        })
    }

    pub fn update(&self, state: &State) {
        let active = state.active();
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
        let (items, checked) = break_items(state);
        self.sync_breaks(items);
        for (index, (_, item)) in self.breaks.borrow().iter().enumerate() {
            item.set_checked(checked == Some(index));
            // Like the window, the running break can't change until it's stopped.
            item.set_enabled(!active);
        }
        let state_text = if active { "Awake" } else { "Resting" };
        let selected = state
            .coffee_break()
            .map_or_else(|| "Custom".into(), CoffeeBreak::label);
        let _ = self.icon.set_tooltip(Some(format!(
            "Caffettino • {state_text} • {selected}\nClick to toggle. Right-click for the menu."
        )));
        self.steam.set_brewing(active);
    }

    /// Rebuilds the break items only when the list itself changed.
    fn sync_breaks(&self, items: Vec<BreakItem>) {
        let mut breaks = self.breaks.borrow_mut();
        if breaks.iter().map(|(item, _)| item).eq(items.iter()) {
            return;
        }
        for (_, old) in breaks.drain(..) {
            if let Err(error) = self.menu.remove(&old) {
                eprintln!("Caffettino tray: {error}");
            }
        }
        for (offset, item) in items.into_iter().enumerate() {
            let check = CheckMenuItem::with_id(
                format!("{BREAK_ID}{}", item.id),
                &item.title,
                true,
                false,
                None,
            );
            if let Err(error) = self.menu.insert(&check, FIRST_BREAK + offset) {
                eprintln!("Caffettino tray: {error}");
            }
            breaks.push((item, check));
        }
    }
}

/// Two wisps of steam, `phase` (0..1) of the way through their sway, as line segments.
/// The wave travels upward and the wisps sway out of step so they look like rising steam.
fn steam_lines(phase: f32) -> Vec<([f32; 2], [f32; 2])> {
    const STEPS: u8 = 8;
    const BOTTOM: f32 = 6.5;
    const HEIGHT: f32 = 4.0;
    let mut lines = Vec::with_capacity(2 * usize::from(STEPS));
    for (base, offset) in [(6.5, 0.0), (11.0, 0.35)] {
        let point = |step: u8| {
            let rise = f32::from(step) / f32::from(STEPS);
            // Anchored at the cup, the wisp sways more toward its tip.
            let sway = 0.25 + 0.6 * rise;
            let angle = TAU * (0.5 * rise - phase - offset);
            [base + sway * angle.sin(), BOTTOM - HEIGHT * rise]
        };
        lines.extend((0..STEPS).map(|step| (point(step), point(step + 1))));
    }
    lines
}

/// The icon's side in pixels: 22 points at 2×, the menu bar's height.
const ICON_PIXELS: u32 = 44;

/// Wraps black RGBA cup pixels in a 22pt template image that follows the menu bar's tint.
fn template_image(pixels: &[u8]) -> Option<Retained<NSImage>> {
    let side = NSInteger::try_from(ICON_PIXELS).ok()?;
    // Null planes ask AppKit to allocate a buffer of the described packed RGBA layout.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            side * 4,
            32,
        )
    }?;
    let data = rep.bitmapData();
    if data.is_null() || pixels.len() != (ICON_PIXELS * ICON_PIXELS * 4) as usize {
        return None;
    }
    // The buffer holds exactly side × side × 4 bytes. The pixels are black, so their
    // premultiplied and straight alpha forms are identical.
    unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), data, pixels.len()) };
    let points = f64::from(ICON_PIXELS) / 2.0;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(points, points));
    image.addRepresentation(&rep);
    image.setTemplate(true);
    Some(image)
}

/// Black RGBA pixels of the cup at 2×, with alpha carrying the shape.
/// `steam` is the animation phase (0..1) of the wisps, or `None` for a resting cup.
fn cup_pixels(steam: Option<f32>) -> Vec<u8> {
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
    if let Some(phase) = steam {
        lines.extend(steam_lines(phase));
    }
    let mut pixels = Vec::with_capacity((ICON_PIXELS * ICON_PIXELS * 4) as usize);
    for y in 0..ICON_PIXELS {
        for x in 0..ICON_PIXELS {
            #[expect(
                clippy::cast_precision_loss,
                reason = "Pixel coordinates are below 44 and exact in f32"
            )]
            let p = [x as f32 / 2.0 + 0.25, y as f32 / 2.0 + 0.25];
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
    pixels
}
