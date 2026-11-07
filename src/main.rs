mod caffeine;
mod presets;
#[cfg(target_os = "macos")]
mod tray;
mod view;

use caffeine::{Caffeine, CoffeeBreak, format_minutes};
use presets::{Presets, Saved, Store, parse_minutes};
use rune::{App, AppHandle, AppSpec, Key, KeyEvent, TextEdit, Tx, WindowOptions};
use std::time::Duration;

/// What the duration row has selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selection {
    /// A preset or "Until I stop".
    Break(CoffeeBreak),
    /// The "+" segment: whatever is typed in the minutes field.
    Custom,
}

impl Default for Selection {
    fn default() -> Self {
        Self::Break(CoffeeBreak::Unlimited)
    }
}

#[derive(Default)]
struct State {
    caffeine: Caffeine,
    presets: Presets,
    selection: Selection,
    custom: TextEdit,
    store: Store,
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
        Self::restored(Saved::default(), Store::Memory, true)
    }

    /// Opens the presets file, falling back to the defaults if it can't be read.
    fn launch(preview: bool, handle: AppHandle<State>) -> Self {
        let mut store = if preview {
            Store::Memory
        } else {
            Store::open()
        };
        let loaded = store.load();
        let mut state = Self::restored(loaded.saved, store, preview);
        if loaded.unreadable {
            state.notice = Some("Couldn't read presets.json — using defaults.");
        }
        state.handle = Some(handle);
        state
    }

    /// A saved one-off break comes back typed into the "+" field, never running.
    fn restored(saved: Saved, store: Store, preview: bool) -> Self {
        let selection = match saved.selected {
            CoffeeBreak::Timed(minutes) if !saved.presets.contains(minutes) => Selection::Custom,
            selected => Selection::Break(selected),
        };
        let custom = match (selection, saved.selected) {
            (Selection::Custom, CoffeeBreak::Timed(minutes)) => TextEdit::new(minutes.to_string()),
            _ => TextEdit::new(""),
        };
        Self {
            presets: saved.presets,
            selection,
            custom,
            store,
            preview,
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

    /// The break Brew would start: `None` while "+" holds no valid duration.
    fn coffee_break(&self) -> Option<CoffeeBreak> {
        match self.selection {
            Selection::Break(coffee_break) => Some(coffee_break),
            Selection::Custom => parse_minutes(&self.custom.text).map(CoffeeBreak::Timed),
        }
    }

    fn toggle(&mut self, tx: &mut Tx) {
        self.notice = None;
        let starting = !self.active();
        let result = match (starting, self.coffee_break()) {
            (true, None) => {
                self.notice = Some("Type 1–1440 minutes to brew a custom break.");
                self.open_window();
                Ok(())
            }
            _ if self.preview => {
                self.preview_active = !self.preview_active;
                Ok(())
            }
            (true, Some(coffee_break)) => self.caffeine.start(coffee_break),
            (false, _) => self.caffeine.stop(),
        };
        if let Err(error) = result {
            eprintln!("Caffettino: {error}");
            self.notice = Some("Couldn't change the coffee break. Try again.");
            self.open_window();
        } else if starting && self.active() && self.selection == Selection::Custom {
            // A one-off break is remembered once it has been brewed.
            self.persist();
        }
        self.minutes_left = self.caffeine.minutes_left();
        self.sync_tray();
        if !self.preview && self.active() {
            // Ensure poll runs after this handler, even if the window is hidden.
            tx.wake_after(Duration::ZERO);
        }
    }

    fn open_window(&self) {
        if let Some(handle) = &self.handle {
            handle.open();
        }
    }

    fn select(&mut self, selection: Selection) {
        if !self.active() && self.selection != selection {
            self.selection = selection;
            if selection != Selection::Custom || self.coffee_break().is_some() {
                self.persist();
            }
            self.sync_tray();
        }
    }

    fn select_break(&mut self, coffee_break: CoffeeBreak) {
        self.select(Selection::Break(coffee_break));
    }

    /// The typed duration Save would add, or select if it's saved already.
    fn savable(&self) -> Option<u32> {
        let minutes = parse_minutes(&self.custom.text)?;
        (self.selection == Selection::Custom
            && !self.active()
            && (self.presets.contains(minutes) || !self.presets.is_full()))
        .then_some(minutes)
    }

    fn save_preset(&mut self) {
        let Some(minutes) = self.savable() else {
            return;
        };
        self.presets.add(minutes);
        self.selection = Selection::Break(CoffeeBreak::Timed(minutes));
        self.custom = TextEdit::new("");
        self.persist();
        self.sync_tray();
    }

    /// The selected preset Remove would delete.
    fn removable(&self) -> Option<u32> {
        match self.selection {
            Selection::Break(CoffeeBreak::Timed(minutes))
                if !self.active() && self.presets.contains(minutes) =>
            {
                Some(minutes)
            }
            _ => None,
        }
    }

    fn remove_preset(&mut self) {
        if let Some(minutes) = self.removable() {
            self.presets.remove(minutes);
            self.selection = Selection::Break(CoffeeBreak::Unlimited);
            self.persist();
            self.sync_tray();
        }
    }

    fn persist(&mut self) {
        let saved = Saved {
            presets: self.presets.clone(),
            selected: self.coffee_break().unwrap_or_default(),
        };
        if let Err(error) = self.store.save(&saved) {
            eprintln!("Caffettino presets: {error}");
            self.notice = Some("Couldn't save your presets.");
        }
    }

    fn sync_tray(&self) {
        #[cfg(target_os = "macos")]
        if let Some(tray) = &self.tray {
            tray.update(self);
        }
    }

    fn detail(&self) -> String {
        if let Some(notice) = self.notice {
            notice.to_owned()
        } else if self.active() {
            match self.minutes_left {
                Some(minutes) => format!(
                    "{} left. One less thing to think about.",
                    format_minutes(minutes)
                ),
                None => "A little coffee. A little more possibility.".into(),
            }
        } else if self.selection == Selection::Custom {
            match parse_minutes(&self.custom.text) {
                None => "Type 1–1440 minutes.".into(),
                Some(minutes) if self.presets.is_full() && !self.presets.contains(minutes) => {
                    "Remove a preset to save another.".into()
                }
                Some(_) => "Brew it once, or save it for later.".into(),
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

fn on_key(state: &mut State, key: &KeyEvent, tx: &mut Tx) -> bool {
    match key.key {
        Key::Command('s') => {
            state.save_preset();
            true
        }
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
                state.selection = Selection::Break(CoffeeBreak::Timed(25));
                state.minutes_left = Some(25);
            }),
            ("custom", |state| {
                state.selection = Selection::Custom;
                state.custom = TextEdit::new("40");
            }),
            ("full", |state| {
                state.presets = Presets::default();
                state.presets.add(90);
                state.selection = Selection::Break(CoffeeBreak::Timed(90));
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
        .init(move || State::launch(preview, handle))
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
        // Land layout motion so the next click finds controls where they rest.
        rt.settle_all();
        rt.frame(0.0);
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
            rt.state.coffee_break(),
            Some(CoffeeBreak::Unlimited),
            "exploration doesn't select"
        );
        assert!(rt.accessibility_action("duration-0", rune::accesskit::Action::Click, None));
        rt.frame(0.0);
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Timed(25)));
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
        assert_eq!(focused(&rt), "duration-custom", "focus stays in the row");
        assert_eq!(rt.state.selection, Selection::Custom);
        key(&mut rt, Key::Right);
        assert_eq!(focused(&rt), "duration-0");
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Timed(25)));
        key(&mut rt, Key::Left);
        key(&mut rt, Key::Left);
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Unlimited));
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
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Timed(25)));
        click(&mut rt, "brew");
        assert!(rt.state.active());
        click(&mut rt, "duration-1");
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Timed(25)));
        click(&mut rt, "brew");
        assert!(!rt.state.active());
        click(&mut rt, "duration-1");
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Timed(60)));
        assert!(
            !rt.state.caffeine.active(),
            "headless controls never launch caffeinate"
        );
    }

    fn preview_runtime(state: State) -> Runtime<State> {
        let mut rt = Runtime::new(state, view::view, view::SIZE, Env::new(false, 1.0));
        rt.on_key = Some(on_key);
        rt.set_tab_visits_controls(true);
        rt.frame(0.0);
        rt
    }

    fn type_minutes(rt: &mut Runtime<State>, minutes: &str) {
        rt.update(|s, _| {
            s.custom = TextEdit::new(minutes);
            s.custom.focused = true;
        });
        rt.frame(0.0);
    }

    fn command(rt: &mut Runtime<State>, c: char) {
        key(rt, Key::Command(c));
    }

    fn keys(rt: &Runtime<State>) -> Vec<String> {
        rt.elements().into_iter().filter_map(|el| el.key).collect()
    }

    fn file_state(dir: &presets::tests::Dir) -> State {
        let mut store = Store::at(dir.file());
        let loaded = store.load();
        State::restored(loaded.saved, store, true)
    }

    #[test]
    fn plus_opens_an_editor_that_brews_a_one_off_break_on_return() {
        let mut rt = preview_runtime(State::preview());
        assert!(!keys(&rt).contains(&"custom-minutes".to_owned()));
        click(&mut rt, "duration-custom");
        assert_eq!(rt.state.selection, Selection::Custom);
        assert!(keys(&rt).contains(&"custom-minutes".to_owned()));
        assert_eq!(rt.state.coffee_break(), None, "nothing typed yet");
        click(&mut rt, "brew");
        assert!(!rt.state.active(), "an empty field can't brew");
        assert!(rt.state.detail().contains("1–1440"));
        type_minutes(&mut rt, "1441");
        assert_eq!(rt.state.coffee_break(), None);
        type_minutes(&mut rt, "40");
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Timed(40)));
        key(&mut rt, Key::Enter);
        assert!(rt.state.active(), "Return in the field brews");
        assert_eq!(
            rt.state.presets.minutes(),
            [25, 60],
            "a one-off isn't saved"
        );
    }

    #[test]
    fn saving_adds_a_sorted_preset_and_selects_it_until_the_row_is_full() {
        let mut rt = preview_runtime(State::preview());
        click(&mut rt, "duration-custom");
        type_minutes(&mut rt, "40");
        click(&mut rt, "save-preset");
        assert_eq!(rt.state.presets.minutes(), [25, 40, 60]);
        assert_eq!(rt.state.selection, Selection::Break(CoffeeBreak::Timed(40)));
        assert_eq!(rt.state.custom.text, "");

        click(&mut rt, "duration-custom");
        type_minutes(&mut rt, "90");
        assert_eq!(rt.state.savable(), None, "three presets is the limit");
        assert_eq!(rt.state.detail(), "Remove a preset to save another.");
        command(&mut rt, 's');
        assert_eq!(rt.state.presets.minutes(), [25, 40, 60]);
        type_minutes(&mut rt, "25");
        command(&mut rt, 's');
        assert_eq!(
            rt.state.selection,
            Selection::Break(CoffeeBreak::Timed(25)),
            "saving a saved duration selects it"
        );
    }

    #[test]
    fn remove_deletes_the_selected_preset_while_resting() {
        let mut rt = preview_runtime(State::preview());
        assert!(
            !keys(&rt).contains(&"remove-preset".to_owned()),
            "Until I stop stays"
        );
        click(&mut rt, "duration-0");
        click(&mut rt, "brew");
        assert!(
            !keys(&rt).contains(&"remove-preset".to_owned()),
            "not while brewing"
        );
        click(&mut rt, "brew");
        click(&mut rt, "remove-preset");
        assert_eq!(rt.state.presets.minutes(), [60]);
        assert_eq!(rt.state.selection, Selection::Break(CoffeeBreak::Unlimited));
        click(&mut rt, "duration-0");
        click(&mut rt, "remove-preset");
        assert_eq!(rt.state.presets.minutes(), [] as [u32; 0]);
        click(&mut rt, "duration-0");
        assert_eq!(rt.state.coffee_break(), Some(CoffeeBreak::Unlimited));
    }

    #[test]
    fn changes_are_saved_and_a_brewed_one_off_comes_back_typed_in() {
        let dir = presets::tests::Dir::new("state");
        let mut rt = preview_runtime(file_state(&dir));
        click(&mut rt, "duration-custom");
        type_minutes(&mut rt, "45");
        command(&mut rt, 's');
        click(&mut rt, "duration-custom");
        type_minutes(&mut rt, "7");
        key(&mut rt, Key::Enter);
        drop(rt);

        let state = file_state(&dir);
        assert_eq!(state.presets.minutes(), [25, 45, 60]);
        assert_eq!(state.selection, Selection::Custom);
        assert_eq!(state.custom.text, "7");
        assert!(!state.active(), "a restored break never starts by itself");

        let mut rt = preview_runtime(state);
        click(&mut rt, "duration-1");
        drop(rt);
        let state = file_state(&dir);
        assert_eq!(state.selection, Selection::Break(CoffeeBreak::Timed(45)));
    }

    #[test]
    fn remaining_time_reads_in_hours_once_past_an_hour() {
        let mut state = State::preview();
        state.preview_active = true;
        state.minutes_left = Some(90);
        assert_eq!(
            state.detail(),
            "1 h 30 left. One less thing to think about."
        );
    }
}
