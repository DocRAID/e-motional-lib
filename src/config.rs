use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::edge::{Settings, DEFAULT_DWELL_MS, DEFAULT_EDGE_PERCENT, DEFAULT_MAX_SPEED_SCALE};

const MAX_POSITIVE_SETTING: f64 = 1_000_000.0;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub edge_percent_x: f64,
    pub edge_percent_y: f64,
    pub edge_width_mm: Option<f64>,
    pub max_speed_scale: f64,
    pub dwell_ms: u64,
    pub devices: Vec<Calibration>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            edge_percent_x: DEFAULT_EDGE_PERCENT,
            edge_percent_y: DEFAULT_EDGE_PERCENT,
            edge_width_mm: None,
            max_speed_scale: DEFAULT_MAX_SPEED_SCALE,
            dwell_ms: DEFAULT_DWELL_MS,
            devices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub id: String,
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("edge_percent_x", self.edge_percent_x),
            ("edge_percent_y", self.edge_percent_y),
        ] {
            if !value.is_finite() || value <= 0.0 || value >= 50.0 {
                return Err(format!(
                    "{name} must be finite and strictly between 0 and 50"
                ));
            }
        }
        for (name, value) in [
            ("edge_width_mm", self.edge_width_mm),
            ("max_speed_scale", Some(self.max_speed_scale)),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || value <= 0.0 || value > MAX_POSITIVE_SETTING {
                    return Err(format!(
                        "{name} must be finite, greater than 0, and at most {MAX_POSITIVE_SETTING}"
                    ));
                }
            }
        }
        let mut ids = HashSet::new();
        for calibration in &self.devices {
            if calibration.id.trim().is_empty() {
                return Err("devices entries must have a nonempty id".to_owned());
            }
            if !ids.insert(calibration.id.as_str()) {
                return Err(format!("duplicate device id {:?}", calibration.id));
            }
            for (axis, min, max) in [
                ("x", calibration.min_x, calibration.max_x),
                ("y", calibration.min_y, calibration.max_y),
            ] {
                let span = max - min;
                if !min.is_finite() || !max.is_finite() || !span.is_finite() || span <= 0.0 {
                    return Err(format!(
                        "device {:?}: min_{axis} and max_{axis} must be finite with a positive finite span",
                        calibration.id
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn settings(&self) -> Settings {
        Settings {
            edge_percent: [self.edge_percent_x, self.edge_percent_y],
            edge_width_mm: self.edge_width_mm,
            max_speed_scale: self.max_speed_scale,
            dwell: Duration::from_millis(self.dwell_ms),
        }
    }
}

pub fn path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| "HOME is not set; cannot locate ~/.config/e-motional-lib.toml".to_owned())?;
    Ok(PathBuf::from(home).join(".config/e-motional-lib.toml"))
}

pub fn load(path: &Path) -> Result<Config, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            log::debug!("no config at {}; using built-in defaults", path.display());
            return Ok(Config::default());
        }
        Err(error) => return Err(format!("cannot read config {}: {error}", path.display())),
    };
    let config: Config = toml::from_str(&text)
        .map_err(|error| format!("cannot parse config {}: {error}", path.display()))?;
    config
        .validate()
        .map_err(|error| format!("invalid config {}: {error}", path.display()))?;
    log::info!("loaded configuration {}", path.display());
    Ok(config)
}

pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    config
        .validate()
        .map_err(|error| format!("cannot save config {}: {error}", path.display()))?;
    let text = toml::to_string_pretty(config)
        .map_err(|error| format!("cannot serialize config {}: {error}", path.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| format!("config path {} has no file name", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create config directory {}: {error}",
            parent.display()
        )
    })?;

    let mut temporary = None;
    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = std::ffi::OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        let candidate = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create temporary config in {}: {error}",
                    parent.display()
                ));
            }
        }
    }
    let (temporary_path, mut file): (PathBuf, File) = temporary.ok_or_else(|| {
        format!(
            "cannot create a unique temporary config in {} after 128 attempts",
            parent.display()
        )
    })?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)
    })();
    if let Err(error) = result {
        let cleanup = fs::remove_file(&temporary_path);
        let cleanup_error = match cleanup {
            Err(error) if error.kind() != io::ErrorKind::NotFound => format!(
                "; cannot remove temporary file {}: {error}",
                temporary_path.display()
            ),
            _ => String::new(),
        };
        return Err(format!(
            "cannot save config {}: {error}{cleanup_error}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            loop {
                let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "e-motional-lib-config-test-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create test directory: {error}"),
                }
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn calibration() -> Calibration {
        Calibration {
            id: "test-touchpad".to_owned(),
            min_x: 120.0,
            max_x: 6120.0,
            min_y: -250.0,
            max_y: 3750.0,
        }
    }

    #[test]
    fn partial_config_changes_horizontal_zone_without_changing_vertical_default() {
        use crate::edge::{Axis, Geometry, Motion};
        use crate::state::{Contact, Frame};
        use std::time::Instant;

        let config: Config = toml::from_str("edge_percent_x = 10\n").unwrap();
        config.validate().unwrap();
        let now = Instant::now();
        for (position, expected) in [
            ([920.0, 300.0], [true, false]),
            ([500.0, 560.0], [false, false]),
            ([500.0, 575.0], [false, true]),
        ] {
            let geometry = Geometry::new(
                Axis {
                    min: 0.0,
                    max: 1000.0,
                    resolution: 10.0,
                },
                Axis {
                    min: 0.0,
                    max: 600.0,
                    resolution: 10.0,
                },
            )
            .unwrap();
            let mut motion = Motion::new(geometry, config.settings());
            let frame = Frame {
                button_left: false,
                contacts: vec![Contact {
                    id: 1,
                    position: Some(position),
                    origin: Some([500.0, 300.0]),
                }],
            };
            motion.update(&frame, now);
            assert!(!motion.active(now));
            let velocity = motion.velocity(now + Duration::from_millis(DEFAULT_DWELL_MS));
            assert_eq!(velocity.map(|component| component > 0.0), expected);
        }
    }

    #[test]
    fn unknown_fields_and_incomplete_calibrations_are_rejected() {
        assert!(toml::from_str::<Config>("edge_percent = 5.0").is_err());
        assert!(toml::from_str::<Config>("dwell_ms = -1").is_err());
        assert!(toml::from_str::<Config>("[[devices]]\nid = 'pad'").is_err());
        let text = toml::to_string(&Config {
            devices: vec![calibration()],
            ..Config::default()
        })
        .unwrap();
        assert!(toml::from_str::<Config>(&format!("{text}resolution = 10.0\n")).is_err());
    }

    #[test]
    fn percentage_validation_checks_both_strict_boundaries() {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            0.0,
            50.0,
            51.0,
        ] {
            for axis in 0..2 {
                let mut config = Config::default();
                if axis == 0 {
                    config.edge_percent_x = invalid;
                } else {
                    config.edge_percent_y = invalid;
                }
                assert!(config.validate().is_err(), "axis={axis}, value={invalid}");
            }
        }
        Config {
            edge_percent_x: f64::MIN_POSITIVE,
            edge_percent_y: 49.999,
            ..Config::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn positive_settings_are_finite_and_bounded() {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            0.0,
            1_000_001.0,
        ] {
            assert!(Config {
                edge_width_mm: Some(invalid),
                ..Config::default()
            }
            .validate()
            .is_err());
            assert!(Config {
                max_speed_scale: invalid,
                ..Config::default()
            }
            .validate()
            .is_err());
        }
        Config {
            edge_width_mm: Some(MAX_POSITIVE_SETTING),
            max_speed_scale: MAX_POSITIVE_SETTING,
            dwell_ms: u64::MAX,
            ..Config::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn calibrations_require_unique_ids_and_positive_finite_spans() {
        let valid = calibration();
        let config = Config {
            devices: vec![valid.clone()],
            ..Config::default()
        };
        config.validate().unwrap();
        for id in ["", "  "] {
            let mut bad = config.clone();
            bad.devices[0].id = id.to_owned();
            assert!(bad.validate().is_err());
        }
        let mut duplicate = config.clone();
        duplicate.devices.push(valid.clone());
        assert!(duplicate.validate().is_err());
        for (min, max) in [
            (1.0, 1.0),
            (2.0, 1.0),
            (f64::NAN, 10.0),
            (0.0, f64::NAN),
            (f64::NEG_INFINITY, 10.0),
            (0.0, f64::INFINITY),
            (-f64::MAX, f64::MAX),
        ] {
            for axis in 0..2 {
                let mut bad = config.clone();
                if axis == 0 {
                    bad.devices[0].min_x = min;
                    bad.devices[0].max_x = max;
                } else {
                    bad.devices[0].min_y = min;
                    bad.devices[0].max_y = max;
                }
                assert!(bad.validate().is_err(), "axis={axis}, bounds={min}..{max}");
            }
        }
    }

    #[test]
    fn missing_config_does_not_create_anything() {
        let directory = TestDirectory::new();
        let parent = directory.0.join("not-created");
        let config = load(&parent.join("config.toml")).unwrap();
        assert_eq!(config.edge_percent_x, DEFAULT_EDGE_PERCENT);
        assert!(!parent.exists());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn invalid_existing_files_are_rejected_without_overwriting() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        for text in [
            "not valid TOML [",
            "edge_percent_y = 50.0\n",
            "edge_width_mm = nan\n",
            "unknown_setting = true\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
        fs::write(&path, [0xff]).unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn save_creates_parents_and_atomically_replaces_readable_config() {
        let directory = TestDirectory::new();
        let path = directory.0.join("nested/config.toml");
        save(&path, &Config::default()).unwrap();
        let config = Config {
            edge_percent_x: 8.0,
            edge_percent_y: 12.0,
            edge_width_mm: Some(3.0),
            max_speed_scale: 2.0,
            dwell_ms: 240,
            devices: vec![calibration()],
        };
        save(&path, &config).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.edge_percent_x, config.edge_percent_x);
        assert_eq!(loaded.edge_percent_y, config.edge_percent_y);
        assert_eq!(loaded.edge_width_mm, config.edge_width_mm);
        assert_eq!(loaded.max_speed_scale, config.max_speed_scale);
        assert_eq!(loaded.dwell_ms, config.dwell_ms);
        assert_eq!(loaded.devices, config.devices);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn rejected_save_preserves_existing_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        let original = "# existing configuration\nedge_percent_x = 4.0\n";
        fs::write(&path, original).unwrap();
        let invalid = Config {
            edge_percent_x: 0.0,
            ..Config::default()
        };
        assert!(save(&path, &invalid).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn filesystem_failures_preserve_existing_content_and_remove_temporary_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "original").unwrap();
        assert!(save(&path, &Config::default()).is_err());
        assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "original");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);

        let parent_file = directory.0.join("not-a-directory");
        fs::write(&parent_file, "untouched").unwrap();
        assert!(save(&parent_file.join("config.toml"), &Config::default()).is_err());
        assert_eq!(fs::read_to_string(parent_file).unwrap(), "untouched");
    }
}
