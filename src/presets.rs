//! Saved coffee breaks and the hand-editable `presets.json` that keeps them between launches.
//!
//! The file is read once at launch and rewritten whole (temporary file, then rename) after each
//! change made in the app. Hand edits that break the rules are tidied in memory without touching
//! the file; a file that can't be read at all is kept as `presets.json.bad` before it's replaced.
use crate::caffeine::CoffeeBreak;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const MIN_MINUTES: u32 = 1;
pub const MAX_MINUTES: u32 = 24 * 60;
/// Timed presets that fit the window's single segment row beside "Until I stop" and "+".
pub const MAX_PRESETS: usize = 3;
const DEFAULT_PRESETS: [u32; 2] = [25, 60];
const VERSION: u32 = 1;
/// Matches `CFBundleIdentifier` in `assets/Info.plist`.
const BUNDLE_ID: &str = "com.matteogassend.caffettino";
const FILE: &str = "presets.json";

/// Whole minutes typed by a person: digits only, within 1–1440.
pub fn parse_minutes(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|&minutes| in_range(minutes))
}

/// `~/Library/Application Support/<bundle id>`, where the app keeps its files.
pub fn support_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| {
            PathBuf::from(home)
                .join("Library/Application Support")
                .join(BUNDLE_ID)
        })
}

fn in_range(minutes: u32) -> bool {
    (MIN_MINUTES..=MAX_MINUTES).contains(&minutes)
}

/// Timed presets, shortest first, without duplicates, at most [`MAX_PRESETS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presets(Vec<u32>);

impl Default for Presets {
    fn default() -> Self {
        Self(DEFAULT_PRESETS.to_vec())
    }
}

impl Presets {
    /// Keeps the shortest valid, distinct durations that fit.
    fn tidy(minutes: impl IntoIterator<Item = u64>) -> Self {
        let mut kept: Vec<u32> = minutes
            .into_iter()
            .filter_map(|minutes| u32::try_from(minutes).ok())
            .filter(|&minutes| in_range(minutes))
            .collect();
        kept.sort_unstable();
        kept.dedup();
        kept.truncate(MAX_PRESETS);
        Self(kept)
    }

    pub fn minutes(&self) -> &[u32] {
        &self.0
    }

    pub fn contains(&self, minutes: u32) -> bool {
        self.0.contains(&minutes)
    }

    pub fn is_full(&self) -> bool {
        self.0.len() >= MAX_PRESETS
    }

    /// Adds a valid duration in order. Returns false when it's invalid, saved already or full.
    pub fn add(&mut self, minutes: u32) -> bool {
        if !in_range(minutes) || self.contains(minutes) || self.is_full() {
            return false;
        }
        let at = self.0.partition_point(|&saved| saved < minutes);
        self.0.insert(at, minutes);
        true
    }

    pub fn remove(&mut self, minutes: u32) -> bool {
        let before = self.0.len();
        self.0.retain(|&saved| saved != minutes);
        self.0.len() != before
    }
}

/// Everything kept between launches. A timed `selected` that isn't a preset is a one-off break.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub presets: Presets,
    pub selected: CoffeeBreak,
}

/// The JSON layout. Missing fields take their defaults and unknown ones are ignored.
#[derive(Serialize, Deserialize)]
struct PresetsFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default = "default_presets")]
    presets: Vec<u64>,
    /// Minutes, or `null` for "Until I stop".
    #[serde(default)]
    selected: Option<u64>,
}

fn default_version() -> u32 {
    VERSION
}

fn default_presets() -> Vec<u64> {
    DEFAULT_PRESETS.map(u64::from).to_vec()
}

impl PresetsFile {
    fn into_saved(self) -> Saved {
        let selected = self
            .selected
            .and_then(|minutes| u32::try_from(minutes).ok())
            .filter(|&minutes| in_range(minutes))
            .map_or(CoffeeBreak::Unlimited, CoffeeBreak::Timed);
        Saved {
            presets: Presets::tidy(self.presets),
            selected,
        }
    }

    fn from_saved(saved: &Saved) -> Self {
        Self {
            version: VERSION,
            presets: saved
                .presets
                .minutes()
                .iter()
                .copied()
                .map(u64::from)
                .collect(),
            selected: match saved.selected {
                CoffeeBreak::Timed(minutes) => Some(u64::from(minutes)),
                CoffeeBreak::Unlimited => None,
            },
        }
    }
}

/// The outcome of reading the store at launch.
pub struct Loaded {
    pub saved: Saved,
    /// The file exists but couldn't be read; the defaults stand in for it.
    pub unreadable: bool,
}

/// Where presets live: nowhere (previews and tests) or a JSON file.
#[derive(Debug, Default)]
pub enum Store {
    #[default]
    Memory,
    File {
        path: PathBuf,
        /// The file on disk couldn't be read: keep it as `.bad` before replacing it.
        keep_bad: bool,
    },
}

impl Store {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self::File {
            path: path.into(),
            keep_bad: false,
        }
    }

    /// `~/Library/Application Support/<bundle id>/presets.json`, or memory without a home.
    pub fn open() -> Self {
        support_dir().map_or(Self::Memory, |dir| Self::at(dir.join(FILE)))
    }

    pub fn load(&mut self) -> Loaded {
        let Self::File { path, keep_bad } = self else {
            return Loaded {
                saved: Saved::default(),
                unreadable: false,
            };
        };
        let parsed = match std::fs::read_to_string(&*path) {
            Ok(json) => {
                serde_json::from_str::<PresetsFile>(&json).map_err(|error| error.to_string())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Loaded {
                    saved: Saved::default(),
                    unreadable: false,
                };
            }
            Err(error) => Err(error.to_string()),
        };
        match parsed {
            Ok(file) => Loaded {
                saved: file.into_saved(),
                unreadable: false,
            },
            Err(error) => {
                eprintln!("Caffettino presets: {}: {error}", path.display());
                *keep_bad = true;
                Loaded {
                    saved: Saved::default(),
                    unreadable: true,
                }
            }
        }
    }

    /// Writes `saved` whole. On an error the file is left as it was.
    pub fn save(&mut self, saved: &Saved) -> io::Result<()> {
        let Self::File { path, keep_bad } = self else {
            return Ok(());
        };
        let mut json = serde_json::to_string_pretty(&PresetsFile::from_saved(saved))
            .map_err(|error| io::Error::other(error.to_string()))?;
        json.push('\n');
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if *keep_bad {
            match std::fs::rename(&*path, with_suffix(path, ".bad")) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => *keep_bad = false,
            }
        }
        write_atomic(path, json.as_bytes())
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = with_suffix(path, &format!(".{}.tmp", std::process::id()));
    let result = std::fs::File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A scratch directory removed when dropped.
    pub struct Dir(pub PathBuf);

    impl Dir {
        pub fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "caffettino-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        pub fn file(&self) -> PathBuf {
            self.0.join("app").join(FILE)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn load_json(dir: &Dir, json: &str) -> (Store, Loaded) {
        let path = dir.file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, json).unwrap();
        let mut store = Store::at(path);
        let loaded = store.load();
        (store, loaded)
    }

    #[test]
    fn typed_minutes_are_digits_within_a_day() {
        assert_eq!(parse_minutes("40"), Some(40));
        assert_eq!(parse_minutes(" 90 "), Some(90));
        assert_eq!(parse_minutes("1440"), Some(1440));
        for bad in ["", "0", "1441", "-5", "+5", "4.5", "abc", "99999999999"] {
            assert_eq!(parse_minutes(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn presets_stay_sorted_distinct_and_capped() {
        let mut presets = Presets::default();
        assert!(presets.add(40));
        assert_eq!(presets.minutes(), [25, 40, 60]);
        assert!(presets.is_full());
        assert!(!presets.add(10), "a fourth preset doesn't fit");
        assert!(presets.remove(25));
        assert!(!presets.add(40), "duplicates are refused");
        assert!(!presets.add(0) && !presets.add(1441));
        assert!(presets.add(5));
        assert_eq!(presets.minutes(), [5, 40, 60]);
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = Dir::new("missing");
        let mut store = Store::at(dir.file());
        let loaded = store.load();
        assert!(!loaded.unreadable);
        assert_eq!(loaded.saved, Saved::default());
        assert_eq!(loaded.saved.presets.minutes(), [25, 60]);
        assert_eq!(loaded.saved.selected, CoffeeBreak::Unlimited);
    }

    #[test]
    fn saves_round_trip_as_readable_json() {
        let dir = Dir::new("roundtrip");
        let mut store = Store::at(dir.file());
        let saved = Saved {
            presets: Presets::tidy([10, 90]),
            selected: CoffeeBreak::Timed(40),
        };
        store.save(&saved).unwrap();
        let json = std::fs::read_to_string(dir.file()).unwrap();
        assert!(json.contains("\"version\": 1"), "{json}");
        assert!(json.ends_with('\n'));
        assert_eq!(Store::at(dir.file()).load().saved, saved);

        store
            .save(&Saved {
                selected: CoffeeBreak::Unlimited,
                ..saved
            })
            .unwrap();
        let json = std::fs::read_to_string(dir.file()).unwrap();
        assert!(json.contains("\"selected\": null"), "{json}");
        let leftovers: Vec<_> = std::fs::read_dir(dir.file().parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [FILE], "no temporary file is left behind");
    }

    #[test]
    fn hand_edits_are_tidied_in_memory_without_rewriting_the_file() {
        let dir = Dir::new("tidy");
        let json = r#"{ "presets": [90, 0, 25, 25, 5000, 10, 45], "selected": 7, "note": "hi" }"#;
        let (_, loaded) = load_json(&dir, json);
        assert!(!loaded.unreadable);
        assert_eq!(loaded.saved.presets.minutes(), [10, 25, 45]);
        assert_eq!(
            loaded.saved.selected,
            CoffeeBreak::Timed(7),
            "a one-off break"
        );
        assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), json);
    }

    #[test]
    fn missing_fields_take_defaults_and_bad_selections_mean_until_i_stop() {
        let dir = Dir::new("fields");
        let (_, loaded) = load_json(&dir, "{}");
        assert_eq!(loaded.saved, Saved::default());
        let (_, loaded) = load_json(&dir, r#"{ "version": 1, "presets": [], "selected": 0 }"#);
        assert_eq!(loaded.saved.presets.minutes(), [] as [u32; 0]);
        assert_eq!(loaded.saved.selected, CoffeeBreak::Unlimited);
        let (_, loaded) = load_json(&dir, r#"{ "selected": 99999 }"#);
        assert_eq!(loaded.saved.selected, CoffeeBreak::Unlimited);
    }

    #[test]
    fn an_unreadable_file_is_kept_as_bad_only_when_replaced() {
        let dir = Dir::new("bad");
        for json in [
            r#"{ "presets": [25,"#,
            r#"{ "presets": ["soon"] }"#,
            "[1, 2]",
        ] {
            let (mut store, loaded) = load_json(&dir, json);
            assert!(loaded.unreadable, "{json}");
            assert_eq!(loaded.saved, Saved::default());
            assert_eq!(
                std::fs::read_to_string(dir.file()).unwrap(),
                json,
                "left alone at launch"
            );
            store.save(&Saved::default()).unwrap();
            let bad = std::fs::read_to_string(with_suffix(&dir.file(), ".bad")).unwrap();
            assert_eq!(bad, json);
            assert_eq!(Store::at(dir.file()).load().saved, Saved::default());
            store.save(&Saved::default()).unwrap();
            assert_eq!(
                std::fs::read_to_string(with_suffix(&dir.file(), ".bad")).unwrap(),
                json,
                "later saves don't overwrite the backup"
            );
        }
    }
}
