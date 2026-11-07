mod caffeine;
#[cfg(target_os = "macos")]
mod tray;
mod view;

use caffeine::{Caffeine, CoffeeBreak};
use rune::{App, AppHandle, AppSpec, Key, KeyEvent, Tx, WindowOptions};
use std::time::Duration;

#[derive(Default)]
struct State {
    caffeine: Caffeine,
    coffee_break: CoffeeBreak,
    notice: Option<&'static str>,
    preview: bool,
    preview_active: bool,
    minutes_left: Option<u64>,
    next_health_check: Option<f64>,
    handle: Option<AppHandle<State>>,
    #[cfg(target_os = "macos")]
    tray: Option<tray::CoffeeTray>,
    tray_attempted: bool,
}

impl State {
    fn preview() -> Self {
        Self {
            preview: true,
            ..Self::default()
        }
    }

    fn active(&self) -> bool {
        if self.preview {
            self.preview_active
        } else {
            self.caffeine.active()
        }
    }

    fn toggle(&mut self, tx: &mut Tx) {
        self.notice = None;
        if self.preview {
            self.preview_active = !self.preview_active;
        } else {
            let result = if self.active() {
                self.caffeine.stop()
            } else {
                self.caffeine.start(self.coffee_break)
            };
            if let Err(error) = result {
                eprintln!("Caffettino: {error}");
                self.notice = Some("Couldn't change the coffee break. Try again.");
                if let Some(handle) = &self.handle {
                    handle.open();
                }
            }
        }
        self.minutes_left = self.caffeine.minutes_left();
        self.sync_tray();
        if !self.preview && self.active() {
            // Ensure poll runs after this handler, even if the window is hidden.
            tx.wake_after(Duration::ZERO);
        }
    }

    fn select_break(&mut self, coffee_break: CoffeeBreak) {
        if !self.active() {
            self.coffee_break = coffee_break;
            self.sync_tray();
        }
    }

    fn sync_tray(&self) {
        #[cfg(target_os = "macos")]
        if let Some(tray) = &self.tray {
            tray.update(self.active(), self.coffee_break);
        }
    }

    fn detail(&self) -> String {
        if let Some(notice) = self.notice {
            notice.to_owned()
        } else if self.active() {
            match self.minutes_left {
                Some(minutes) => format!("{minutes} min left. One less thing to think about."),
                None => "A little coffee. A little more possibility.".into(),
            }
        } else {
            "Your Mac can snooze. You set the pace.".into()
        }
    }
}

fn poll(state: &mut State, now: f64, tx: &mut Tx) -> bool {
    if state.preview {
        return false;
    }
    let mut changed = false;
    if !state.tray_attempted {
        state.tray_attempted = true;
        #[cfg(target_os = "macos")]
        if let Some(handle) = &state.handle {
            match tray::CoffeeTray::new(handle.clone()) {
                Ok(tray) => state.tray = Some(tray),
                Err(error) => {
                    eprintln!("Caffettino tray: {error}");
                    state.notice = Some("The menu bar cup couldn't open. Please relaunch.");
                }
            }
        }
        state.sync_tray();
        changed = true;
    }
    if !state.active() {
        state.next_health_check = None;
        return changed;
    }
    // Animation frames and stale timers must not each create a new timer chain.
    if state
        .next_health_check
        .is_some_and(|deadline| now < deadline)
    {
        return changed;
    }
    match state.caffeine.refresh() {
        Ok(true) => {
            state.notice = Some("Coffee break's over. Sweet dreams, Mac.");
            state.sync_tray();
            changed = true;
        }
        Err(error) => {
            eprintln!("Caffettino process: {error}");
            state.notice = Some("Couldn't check the coffee break. Try stopping it.");
            changed = true;
        }
        Ok(false) => {}
    }
    let minutes = state.caffeine.minutes_left();
    changed |= minutes != state.minutes_left;
    state.minutes_left = minutes;
    // Rune runs this timer even with the window closed, without redrawing it.
    if state.active() {
        state.next_health_check = Some(now + 1.0);
        tx.wake_after(Duration::from_secs(1));
    } else {
        state.next_health_check = None;
    }
    changed
}

fn on_key(_: &mut State, key: &KeyEvent, tx: &mut Tx) -> bool {
    match key.key {
        Key::Command('q') => {
            tx.quit();
            true
        }
        Key::Escape => {
            tx.close_window();
            true
        }
        _ => false,
    }
}

fn main() {
    let preview = rune::tooling::Args::parse().headless();
    if !cfg!(target_os = "macos") && !preview {
        eprintln!("Caffettino's menu bar and caffeinate command require macOS.");
        std::process::exit(1);
    }
    rune::tooling::set_app_id("caffettino");
    let spec = AppSpec {
        min_size: Some(view::SIZE),
        poll: Some(poll),
        on_key: Some(on_key),
        scenes: &[
            ("awake", |state| {
                state.preview_active = true;
            }),
            ("timed", |state| {
                state.preview_active = true;
                state.coffee_break = CoffeeBreak::Short;
                state.minutes_left = Some(25);
            }),
            ("error", |state| {
                state.notice = Some("Couldn't change the coffee break. Try again.");
            }),
        ],
        ..AppSpec::new("Caffettino", view::SIZE, State::preview, view::view)
    };
    let app = App::new(spec)
        .accessory()
        .standard_menus()
        .tab_visits_controls(true)
        .keep_running()
        .release_gpu_when_closed()
        .window_options(WindowOptions {
            resizable: false,
            max_size: Some(view::SIZE),
            ..Default::default()
        });
    let handle = app.handle();
    let app = app
        .init(move || State {
            preview,
            handle: Some(handle),
            ..State::default()
        })
        .on_reopen(on_reopen);
    if let Err(error) = app.try_run() {
        eprintln!("Couldn't open Caffettino: {error}");
        std::process::exit(1);
    }
}

fn on_reopen(state: &mut State, _: bool, _: &mut Tx) -> bool {
    if let Some(handle) = &state.handle {
        handle.open();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};

    fn click(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn key(rt: &mut Runtime<State>, key: Key) {
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(0.0);
    }

    #[test]
    fn accessibility_can_explore_an_unselected_duration_without_focus_snapping_back() {
        let mut rt = Runtime::new(
            State::preview(),
            view::view,
            view::SIZE,
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        assert!(rt.accessibility_action("duration-0", rune::accesskit::Action::Focus, None));
        rt.frame(0.0);
        assert!(
            rt.elements()
                .iter()
                .any(|el| el.key.as_deref() == Some("duration-0") && el.focused)
        );
        assert_eq!(
            rt.state.coffee_break,
            CoffeeBreak::Unlimited,
            "exploration doesn't select"
        );
        assert!(rt.accessibility_action("duration-0", rune::accesskit::Action::Click, None));
        rt.frame(0.0);
        assert_eq!(rt.state.coffee_break, CoffeeBreak::Short);
    }

    #[test]
    fn duration_keyboard_entry_uses_selection_and_arrows_select_and_wrap() {
        let mut rt = Runtime::new(
            State::preview(),
            view::view,
            view::SIZE,
            Env::new(false, 1.0),
        );
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);
        key(&mut rt, Key::Tab);
        let focused = |rt: &Runtime<State>| {
            rt.elements()
                .into_iter()
                .find(|el| el.focused)
                .unwrap()
                .key
                .unwrap()
        };
        assert_eq!(focused(&rt), "duration-2");
        key(&mut rt, Key::Right);
        assert_eq!(focused(&rt), "duration-0");
        assert_eq!(rt.state.coffee_break, CoffeeBreak::Short);
        key(&mut rt, Key::Left);
        assert_eq!(rt.state.coffee_break, CoffeeBreak::Unlimited);
        key(&mut rt, Key::Tab);
        assert_eq!(focused(&rt), "brew");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn animation_does_not_multiply_background_health_check_timers() {
        let mut rt = Runtime::new(
            State {
                tray_attempted: true,
                ..State::default()
            },
            view::view,
            view::SIZE,
            Env::new(false, 1.0),
        );
        rt.poll = Some(poll);
        rt.frame(0.0);
        click(&mut rt, "brew");
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
        }
        let end = rt.clock + 5.0;
        let mut wakes = 0;
        while let Some(deadline) = rt.wake_deadline().filter(|deadline| *deadline < end) {
            rt.frame((deadline - rt.clock).max(0.0) + 0.00001);
            wakes += 1;
            assert!(
                wakes <= 6,
                "more than one health check per second after settling"
            );
        }
        assert!(
            wakes >= 4,
            "health checks must continue while the app is idle"
        );
        click(&mut rt, "brew");
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            rt.wake_deadline().is_none(),
            "stopping lets the event loop sleep"
        );
    }

    #[test]
    fn window_controls_select_a_break_toggle_and_lock_the_running_duration() {
        let mut rt = Runtime::new(
            State::preview(),
            view::view,
            view::SIZE,
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        click(&mut rt, "duration-0");
        assert_eq!(rt.state.coffee_break, CoffeeBreak::Short);
        click(&mut rt, "brew");
        assert!(rt.state.active());
        click(&mut rt, "duration-1");
        assert_eq!(rt.state.coffee_break, CoffeeBreak::Short);
        click(&mut rt, "brew");
        assert!(!rt.state.active());
        click(&mut rt, "duration-1");
        assert_eq!(rt.state.coffee_break, CoffeeBreak::Hour);
        assert!(
            !rt.state.caffeine.active(),
            "headless controls never launch caffeinate"
        );
    }
}
