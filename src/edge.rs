use std::time::{Duration, Instant};

use crate::state::{Contact, Frame};

pub const TICK_HZ: u64 = 125;

// Physical sizes outside this range, or resolutions <= 1 unit/mm, are treated as
// unreliable. The fallback preserves the raw aspect ratio with a 100 mm long side.
const MIN_RELIABLE_RESOLUTION: f64 = 1.0;
const MIN_PLAUSIBLE_SIZE_MM: f64 = 10.0;
const MAX_PLAUSIBLE_SIZE_MM: f64 = 500.0;
const VIRTUAL_LONG_SIDE_MM: f64 = 100.0;
pub const DEFAULT_EDGE_PERCENT: f64 = 5.0;
const HYSTERESIS_MM: f64 = 1.0;
pub const DEFAULT_DWELL_MS: u64 = 150;
pub const DEFAULT_MAX_SPEED_SCALE: f64 = 0.3;
const REFERENCE_DPI: f64 = 1000.0;
const MM_PER_INCH: f64 = 25.4;
const FULL_TRAVERSE_SECONDS: f64 = 1.5;
const MIN_SPEED_FRACTION: f64 = 0.1;
const DEPTH_EXPONENT: f64 = 1.5;

#[derive(Clone, Copy, Debug)]
pub struct Axis {
    pub min: f64,
    pub max: f64,
    pub resolution: f64,
}

#[derive(Clone, Debug)]
pub struct Geometry {
    minimum: [f64; 2],
    span: [f64; 2],
    size_mm: [f64; 2],
    reported_resolution: bool,
}

impl Geometry {
    pub fn new(x: Axis, y: Axis) -> Result<Self, String> {
        let axes = [x, y];
        let span = [x.max - x.min, y.max - y.min];
        for (index, axis) in axes.iter().enumerate() {
            if !axis.min.is_finite()
                || !axis.max.is_finite()
                || !span[index].is_finite()
                || span[index] <= 0.0
            {
                return Err(format!("axis {index} must have a finite positive range"));
            }
        }
        let measured = [span[0] / x.resolution, span[1] / y.resolution];
        let reported_resolution = axes
            .iter()
            .all(|axis| axis.resolution.is_finite() && axis.resolution > MIN_RELIABLE_RESOLUTION)
            && measured.iter().all(|size| {
                size.is_finite() && (MIN_PLAUSIBLE_SIZE_MM..=MAX_PLAUSIBLE_SIZE_MM).contains(size)
            });
        let size_mm = if reported_resolution {
            measured
        } else {
            let longest = span[0].max(span[1]);
            [
                span[0] / longest * VIRTUAL_LONG_SIDE_MM,
                span[1] / longest * VIRTUAL_LONG_SIDE_MM,
            ]
        };
        if size_mm.iter().any(|size| !size.is_finite() || *size <= 0.0) {
            return Err("axis aspect ratio cannot be represented".into());
        }
        Ok(Self {
            minimum: [x.min, y.min],
            span,
            size_mm,
            reported_resolution,
        })
    }

    pub fn dimensions_mm(&self) -> [f64; 2] {
        self.size_mm
    }

    pub fn uses_reported_resolution(&self) -> bool {
        self.reported_resolution
    }

    fn position_mm(&self, position: [f64; 2]) -> Option<[f64; 2]> {
        let result = std::array::from_fn(|axis| {
            (position[axis] - self.minimum[axis]) / self.span[axis] * self.size_mm[axis]
        });
        result
            .iter()
            .all(|coordinate| coordinate.is_finite())
            .then_some(result)
    }

    fn edge_width_mm(&self, percent: [f64; 2], configured: Option<f64>) -> [f64; 2] {
        std::array::from_fn(|axis| {
            let width = self.size_mm[axis] * (percent[axis] / 100.0);
            configured.map_or(width, |cap| width.min(cap))
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Optional positive mm cap applied after the independent axis percentages.
    pub edge_width_mm: Option<f64>,
    pub edge_percent: [f64; 2],
    pub max_speed_scale: f64,
    pub dwell: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            edge_width_mm: None,
            edge_percent: [DEFAULT_EDGE_PERCENT; 2],
            max_speed_scale: DEFAULT_MAX_SPEED_SCALE,
            dwell: Duration::from_millis(DEFAULT_DWELL_MS),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AxisMotion {
    direction: i8,
    entered_at: Option<Instant>,
    depth: f64,
}

impl AxisMotion {
    fn update(&mut self, position: f64, length: f64, width: f64, now: Instant, dwell: Duration) {
        let margin = if self.ready(now, dwell) {
            HYSTERESIS_MM
        } else {
            0.0
        };
        let retained = match self.direction {
            -1 => position <= width + margin,
            1 => position >= length - width - margin,
            _ => false,
        };
        if !retained {
            let direction = if position <= width {
                -1
            } else if position >= length - width {
                1
            } else {
                0
            };
            self.direction = direction;
            self.entered_at = (direction != 0).then_some(now);
        }
        self.depth = match self.direction {
            -1 => ((width - position) / width).clamp(0.0, 1.0),
            1 => ((position - (length - width)) / width).clamp(0.0, 1.0),
            _ => 0.0,
        };
    }

    fn ready(&self, now: Instant, dwell: Duration) -> bool {
        self.entered_at
            .is_some_and(|entered| now.saturating_duration_since(entered) >= dwell)
    }
}

#[derive(Debug)]
pub struct Motion {
    geometry: Geometry,
    settings: Settings,
    edge_width_mm: [f64; 2],
    max_speed: f64,
    contact_id: Option<i32>,
    axes: [AxisMotion; 2],
    components: [f64; 2],
}

impl Motion {
    pub fn new(geometry: Geometry, settings: Settings) -> Self {
        let edge_width_mm = geometry.edge_width_mm(settings.edge_percent, settings.edge_width_mm);
        let max_speed = geometry.size_mm[0].max(geometry.size_mm[1])
            * (REFERENCE_DPI / MM_PER_INCH)
            / FULL_TRAVERSE_SECONDS
            * settings.max_speed_scale;
        log::debug!("motion settings: size_mm={:?} edge_percent={:?} edge_width_mm={edge_width_mm:?} hysteresis_mm={HYSTERESIS_MM} dwell_ms={} max_speed_px_s={max_speed:.3}", geometry.size_mm, settings.edge_percent, settings.dwell.as_millis());
        Self {
            geometry,
            settings,
            edge_width_mm,
            max_speed,
            contact_id: None,
            axes: [AxisMotion::default(); 2],
            components: [0.0; 2],
        }
    }

    fn eligible_position(&self, contact: &Contact) -> Option<[f64; 2]> {
        let Some(origin) = contact
            .origin
            .and_then(|origin| self.geometry.position_mm(origin))
        else {
            log::trace!(
                "activation blocked: contact={} unknown or invalid origin",
                contact.id
            );
            return None;
        };
        for (axis, coordinate) in origin.iter().enumerate() {
            // An origin on an edge boundary is not strictly in the interior.
            if *coordinate <= self.edge_width_mm[axis]
                || *coordinate >= self.geometry.size_mm[axis] - self.edge_width_mm[axis]
            {
                log::trace!("activation blocked: contact={} started in edge zone origin_mm={origin:?} axis={axis} width_mm={:.3}", contact.id, self.edge_width_mm[axis]);
                return None;
            }
        }
        let position = contact
            .position
            .and_then(|position| self.geometry.position_mm(position));
        if position.is_none() {
            log::trace!(
                "activation blocked: contact={} unknown or invalid current position",
                contact.id
            );
        }
        position
    }

    /// Track one interior-started contact independently of physical button state.
    /// Contact origins belong to the tracker and survive multifinger intervals.
    pub fn update(&mut self, frame: &Frame, now: Instant) {
        if frame.contacts.len() != 1 || self.edge_width_mm.iter().any(|width| *width <= 0.0) {
            log::trace!(
                "activation blocked: fingers={} edge_width_mm={:?}",
                frame.contacts.len(),
                self.edge_width_mm
            );
            self.reset();
            return;
        }
        let contact = &frame.contacts[0];
        let Some(position) = self.eligible_position(contact) else {
            self.reset();
            return;
        };
        if self.contact_id != Some(contact.id) {
            self.reset();
            self.contact_id = Some(contact.id);
        }
        for (axis, coordinate) in position.iter().enumerate() {
            self.axes[axis].update(
                *coordinate,
                self.geometry.size_mm[axis],
                self.edge_width_mm[axis],
                now,
                self.settings.dwell,
            );
            let state = self.axes[axis];
            self.components[axis] = f64::from(state.direction)
                * self.max_speed
                * (MIN_SPEED_FRACTION
                    + (1.0 - MIN_SPEED_FRACTION) * state.depth.powf(DEPTH_EXPONENT));
        }
        log::trace!(
            "edge evaluation: contact={} position_mm={position:?} directions={:?} depths={:?} dwell_elapsed_ms={:?} active={} velocity_px_s={:?}",
            contact.id, self.axes.map(|axis| axis.direction), self.axes.map(|axis| axis.depth),
            self.axes.map(|axis| axis.entered_at.map(|entered| now.saturating_duration_since(entered).as_millis())),
            self.active(now), self.velocity(now)
        );
    }

    pub fn velocity(&self, now: Instant) -> [f64; 2] {
        std::array::from_fn(|axis| {
            let state = self.axes[axis];
            if !state.ready(now, self.settings.dwell) {
                return 0.0;
            }
            self.components[axis]
        })
    }

    /// Earliest dwell boundary. A caller scheduling timers must ignore boundaries
    /// <= its current time: elapsed boundaries stay available until state changes.
    pub fn deadline(&self) -> Option<Instant> {
        self.axes
            .iter()
            .filter_map(|axis| axis.entered_at?.checked_add(self.settings.dwell))
            .min()
    }

    pub fn active(&self, now: Instant) -> bool {
        self.max_speed > 0.0
            && self
                .axes
                .iter()
                .any(|axis| axis.ready(now, self.settings.dwell))
    }

    pub fn reset(&mut self) {
        self.contact_id = None;
        self.axes = [AxisMotion::default(); 2];
        self.components = [0.0; 2];
    }
}

#[derive(Debug, Default)]
pub struct Accumulator {
    fraction: [f64; 2],
}

impl Accumulator {
    pub fn step(&mut self, velocity: [f64; 2], dt: Duration) -> [i32; 2] {
        let seconds = dt.as_secs_f64();
        std::array::from_fn(|axis| {
            let distance = velocity[axis] * seconds + self.fraction[axis];
            // Saturation discards movement beyond the event representation rather
            // than carrying an unbounded backlog into later ticks.
            let bounded = distance.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
            let integer = bounded as i32;
            self.fraction[axis] = if bounded.is_finite() {
                bounded - f64::from(integer)
            } else {
                0.0
            };
            integer
        })
    }

    pub fn reset(&mut self) {
        self.fraction = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    fn geometry() -> Geometry {
        Geometry::new(
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
        .unwrap()
    }

    fn frame(position: [f64; 2]) -> Frame {
        Frame {
            button_left: false,
            contacts: vec![Contact {
                id: 1,
                position: Some(position),
                origin: Some([500.0, 300.0]),
            }],
        }
    }

    fn motion() -> Motion {
        Motion::new(geometry(), Settings::default())
    }

    #[test]
    fn physical_geometry_and_zone_width() {
        let geometry = geometry();
        assert!(geometry.uses_reported_resolution());
        assert_eq!(geometry.dimensions_mm(), [100.0, 60.0]);
        assert_eq!(geometry.edge_width_mm([5.0; 2], None), [5.0, 3.0]);
        assert_eq!(geometry.edge_width_mm([10.0, 2.0], None), [10.0, 1.2]);
        assert_eq!(geometry.edge_width_mm([10.0, 2.0], Some(4.0)), [4.0, 1.2]);
        assert_eq!(geometry.edge_width_mm([5.0; 2], Some(2.0)), [2.0, 2.0]);
    }

    #[test]
    fn fallback_preserves_ratio_and_rejects_implausible_resolution() {
        for resolution in [0.0, 1.0, f64::NAN, 10000.0] {
            let geometry = Geometry::new(
                Axis {
                    min: 100.0,
                    max: 2100.0,
                    resolution,
                },
                Axis {
                    min: -200.0,
                    max: 800.0,
                    resolution: 10.0,
                },
            )
            .unwrap();
            assert!(!geometry.uses_reported_resolution());
            assert_eq!(geometry.dimensions_mm(), [100.0, 50.0]);
            assert_eq!(geometry.position_mm([1100.0, 300.0]), Some([50.0, 25.0]));
            assert_eq!(geometry.edge_width_mm([5.0; 2], None), [5.0, 2.5]);
        }
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        let good = Axis {
            min: 0.0,
            max: 1000.0,
            resolution: 10.0,
        };
        for max in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(Geometry::new(Axis { max, ..good }, good).is_err());
        }
    }

    #[test]
    fn default_trigger_is_five_percent_from_each_axis_edge() {
        let now = Instant::now();
        for (outside, boundary, axis, direction) in [
            ([50.1, 300.0], [50.0, 300.0], 0, -1.0),
            ([949.9, 300.0], [950.0, 300.0], 0, 1.0),
            ([500.0, 30.1], [500.0, 30.0], 1, -1.0),
            ([500.0, 569.9], [500.0, 570.0], 1, 1.0),
        ] {
            let mut motion = motion();
            motion.update(&frame(outside), now);
            assert_eq!(motion.deadline(), None);
            assert_eq!(motion.velocity(now + Duration::from_secs(1)), [0.0; 2]);
            motion.update(&frame(boundary), now);
            let ready = now + Duration::from_millis(DEFAULT_DWELL_MS);
            assert!(!motion.active(ready - Duration::from_nanos(1)));
            let velocity = motion.velocity(ready);
            assert!(
                velocity[axis] * direction > 0.0,
                "boundary {boundary:?}: {velocity:?}"
            );
            assert_eq!(velocity[1 - axis], 0.0);
        }
    }

    #[test]
    fn dwell_activates_without_another_frame() {
        let now = Instant::now();
        let mut motion = motion();
        motion.update(&frame([50.0, 300.0]), now);
        motion.update(&frame([50.0, 300.0]), now + Duration::from_millis(75));
        let deadline = now + Duration::from_millis(DEFAULT_DWELL_MS);
        assert_eq!(motion.deadline(), Some(deadline));
        assert!(!motion.active(deadline - Duration::from_nanos(1)));
        assert!(motion.active(deadline));
        assert!(motion.velocity(deadline)[0] < 0.0);
        assert_eq!(motion.velocity(deadline)[1], 0.0);
    }

    #[test]
    fn depth_curve_and_all_four_sides() {
        let now = Instant::now();
        let mut motion = Motion::new(
            geometry(),
            Settings {
                max_speed_scale: 1.0,
                dwell: Duration::ZERO,
                ..Settings::default()
            },
        );
        let maximum = 100.0 * (REFERENCE_DPI / MM_PER_INCH) / FULL_TRAVERSE_SECONDS;
        for (position, expected) in [
            ([0.0, 300.0], [-maximum, 0.0]),
            ([1000.0, 300.0], [maximum, 0.0]),
            ([500.0, 0.0], [0.0, -maximum]),
            ([500.0, 600.0], [0.0, maximum]),
        ] {
            motion.update(&frame(position), now);
            let velocity = motion.velocity(now);
            close(velocity[0], expected[0]);
            close(velocity[1], expected[1]);
        }
        motion.reset();
        motion.update(&frame([50.0, 300.0]), now);
        close(motion.axes[0].depth, 0.0);
        close(motion.velocity(now)[0], -maximum * MIN_SPEED_FRACTION);
        motion.update(&frame([25.0, 300.0]), now);
        close(motion.axes[0].depth, 0.5);
        close(
            motion.velocity(now)[0],
            -maximum * (0.1 + 0.9 * 0.5_f64.powf(1.5)),
        );
        motion.update(&frame([-10.0, 300.0]), now);
        close(motion.axes[0].depth, 1.0);
        close(motion.velocity(now)[0], -maximum);
    }

    #[test]
    fn leaving_zone_during_dwell_cancels_pending_activation() {
        let now = Instant::now();
        let mut motion = motion();
        motion.update(&frame([0.0, 300.0]), now);
        let left = now + Duration::from_millis(75);
        motion.update(&frame([55.0, 300.0]), left);
        assert!(!motion.active(now + Duration::from_millis(200)));
        assert_eq!(motion.deadline(), None);
        motion.update(&frame([0.0, 300.0]), left);
        assert!(!motion.active(left + Duration::from_millis(149)));
        assert!(motion.active(left + Duration::from_millis(150)));
    }

    #[test]
    fn hysteresis_retains_direction_at_minimum_speed_then_clears() {
        let now = Instant::now();
        let mut motion = motion();
        motion.update(&frame([0.0, 300.0]), now);
        let ready = now + motion.settings.dwell;
        motion.update(&frame([59.0, 300.0]), ready);
        assert!(motion.active(ready));
        assert_eq!(motion.axes[0].depth, 0.0);
        close(
            motion.velocity(ready)[0],
            -motion.max_speed * MIN_SPEED_FRACTION,
        );
        motion.update(&frame([61.0, 300.0]), ready);
        assert!(!motion.active(ready));
        assert_eq!(motion.deadline(), None);
        motion.update(&frame([0.0, 300.0]), ready);
        assert!(!motion.active(ready));
        assert!(motion.active(ready + motion.settings.dwell));
    }

    #[test]
    fn corner_axes_have_independent_dwell_and_direction_changes() {
        let now = Instant::now();
        let mut motion = motion();
        motion.update(&frame([0.0, 300.0]), now);
        let halfway = now + Duration::from_millis(75);
        motion.update(&frame([0.0, 0.0]), halfway);
        let ready = now + motion.settings.dwell;
        let velocity = motion.velocity(ready);
        assert!(velocity[0] < 0.0);
        assert_eq!(velocity[1], 0.0);
        motion.update(&frame([1000.0, 0.0]), ready);
        assert_eq!(motion.velocity(ready)[0], 0.0);
        assert!(motion.velocity(halfway + motion.settings.dwell)[1] < 0.0);
        let velocity = motion.velocity(ready + motion.settings.dwell);
        assert!(velocity[0] > 0.0 && velocity[1] < 0.0);
    }

    #[test]
    fn edge_origins_never_become_eligible() {
        let now = Instant::now();
        let mut motion = motion();
        for origin in [
            [0.0, 300.0],
            [50.0, 300.0],
            [1000.0, 300.0],
            [500.0, 0.0],
            [500.0, 600.0],
        ] {
            let mut contact = frame([500.0, 300.0]);
            contact.contacts[0].origin = Some(origin);
            motion.update(&contact, now);
            contact.contacts[0].position = Some([0.0, 300.0]);
            motion.update(&contact, now);
            assert!(!motion.active(now + motion.settings.dwell));
            assert_eq!(motion.deadline(), None);
        }
    }

    #[test]
    fn button_transitions_do_not_gate_dwell_or_motion() {
        let now = Instant::now();
        let mut motion = motion();
        let mut contact = frame([500.0, 300.0]);
        contact.button_left = false;
        motion.update(&contact, now);
        assert_eq!(motion.deadline(), None);
        let entered = now + Duration::from_millis(10);
        contact.contacts[0].position = Some([950.0, 300.0]);
        motion.update(&contact, entered);
        let deadline = entered + motion.settings.dwell;
        assert_eq!(motion.deadline(), Some(deadline));
        contact.button_left = true;
        motion.update(&contact, entered + Duration::from_millis(75));
        contact.button_left = false;
        motion.update(&contact, entered + Duration::from_millis(100));
        assert_eq!(motion.deadline(), Some(deadline));
        assert!(!motion.active(deadline - Duration::from_nanos(1)));
        let velocity = motion.velocity(deadline);
        assert!(velocity[0] > 0.0);
        assert_eq!(velocity[1], 0.0);
        contact.button_left = true;
        motion.update(&contact, deadline);
        contact.button_left = false;
        motion.update(&contact, deadline + Duration::from_millis(1));
        assert_eq!(
            motion.velocity(deadline + Duration::from_millis(1)),
            velocity
        );
    }

    #[test]
    fn multifinger_and_lift_reset_dwell() {
        let now = Instant::now();
        let single = frame([0.0, 300.0]);
        let mut multiple = frame([0.0, 300.0]);
        multiple.contacts.push(Contact {
            id: 2,
            position: None,
            origin: None,
        });
        let lifted = Frame {
            button_left: false,
            contacts: Vec::new(),
        };
        for interrupt in [multiple, lifted] {
            let mut motion = motion();
            motion.update(&single, now);
            let ready = now + motion.settings.dwell;
            assert!(motion.active(ready));
            motion.update(&interrupt, ready);
            assert!(!motion.active(ready));
            assert_eq!(motion.deadline(), None);
            motion.update(&single, ready);
            assert!(!motion.active(ready));
            assert!(motion.active(ready + motion.settings.dwell));
        }
    }

    #[test]
    fn changed_contact_and_unknown_position_reset_dwell() {
        let now = Instant::now();
        let mut motion = motion();
        let mut contact = frame([0.0, 300.0]);
        motion.update(&contact, now);
        let ready = now + motion.settings.dwell;
        contact.contacts[0].id = 2;
        motion.update(&contact, ready);
        assert!(!motion.active(ready));
        assert!(motion.active(ready + motion.settings.dwell));
        contact.contacts[0].position = None;
        motion.update(&contact, ready);
        assert_eq!(motion.deadline(), None);
        contact.contacts[0].position = Some([0.0, 300.0]);
        contact.contacts[0].origin = None;
        motion.update(&contact, ready);
        assert_eq!(motion.deadline(), None);
    }

    #[test]
    fn fractional_carry_is_signed_and_resettable() {
        let mut accumulator = Accumulator::default();
        let tick = Duration::from_millis(250);
        for _ in 0..3 {
            assert_eq!(accumulator.step([1.0, -1.0], tick), [0, 0]);
        }
        assert_eq!(accumulator.step([1.0, -1.0], tick), [1, -1]);
        for _ in 0..3 {
            assert_eq!(accumulator.step([1.0, -1.0], tick), [0, 0]);
        }
        accumulator.reset();
        assert_eq!(accumulator.step([1.0, -1.0], tick), [0, 0]);
        assert_eq!(accumulator.step([-1.0, 1.0], tick), [0, 0]);
        assert_eq!(accumulator.fraction, [0.0, 0.0]);
    }
}
