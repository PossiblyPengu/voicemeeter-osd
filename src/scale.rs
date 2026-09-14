//! Pure conversions between Voicemeeter's dB values, display percentages and
//! meter amplitudes. Kept free of Win32 so it can be unit tested.

/// Maps a fader gain in dB onto 0-100 across the configured range.
pub fn gain_to_percent(gain: f32, min_db: f32, max_db: f32) -> i32 {
    let range = (max_db - min_db).max(1.0);
    let pct = ((gain - min_db) / range * 100.0).round();
    pct.clamp(0.0, 100.0) as i32
}

/// Fraction of the track a given dB value sits at, for drawing marks.
pub fn gain_fraction(gain: f32, min_db: f32, max_db: f32) -> f32 {
    let range = (max_db - min_db).max(1.0);
    ((gain - min_db) / range).clamp(0.0, 1.0)
}

/// Voicemeeter reports levels as linear amplitude; convert to a 0-1 meter
/// position on a dB scale so quiet signal is still visible.
pub fn amplitude_to_meter(amplitude: f32, floor_db: f32) -> f32 {
    if amplitude <= 0.000_015 {
        return 0.0;
    }
    let db = 20.0 * amplitude.log10();
    ((db - floor_db) / -floor_db).clamp(0.0, 1.0)
}

/// Standard delta for one detent of a notched mouse wheel.
pub const WHEEL_DELTA: i32 = 120;

/// Turns raw wheel deltas into whole steps. Precision touchpads and
/// free-spinning wheels report fractions of a detent; dividing each event
/// by 120 would throw those away, so the remainder carries over.
#[derive(Default)]
pub struct WheelSteps {
    remainder: i32,
}

impl WheelSteps {
    pub fn feed(&mut self, delta: i32) -> i32 {
        // A change of direction starts fresh, so half a notch one way
        // doesn't swallow the first notch back.
        if (delta > 0 && self.remainder < 0) || (delta < 0 && self.remainder > 0) {
            self.remainder = 0;
        }
        self.remainder += delta;
        let steps = self.remainder / WHEEL_DELTA;
        self.remainder -= steps * WHEEL_DELTA;
        steps
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

pub fn format_db(gain: Option<f32>) -> String {
    match gain {
        Some(g) => format!("{g:.1} dB"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_spans_the_configured_range() {
        assert_eq!(gain_to_percent(-60.0, -60.0, 12.0), 0);
        assert_eq!(gain_to_percent(12.0, -60.0, 12.0), 100);
        // Unity sits high on a -60..+12 fader, which surprises people.
        assert_eq!(gain_to_percent(0.0, -60.0, 12.0), 83);
    }

    #[test]
    fn percent_clamps_outside_the_range() {
        assert_eq!(gain_to_percent(-90.0, -60.0, 12.0), 0);
        assert_eq!(gain_to_percent(40.0, -60.0, 12.0), 100);
    }

    #[test]
    fn percent_survives_a_degenerate_range() {
        // max <= min would divide by zero without the guard.
        let p = gain_to_percent(0.0, 0.0, 0.0);
        assert!((0..=100).contains(&p));
    }

    #[test]
    fn fraction_matches_percent() {
        assert!((gain_fraction(-24.0, -60.0, 12.0) - 0.5).abs() < 0.01);
    }

    #[test]
    fn silence_reads_as_empty_meter() {
        assert_eq!(amplitude_to_meter(0.0, -60.0), 0.0);
    }

    #[test]
    fn unity_amplitude_fills_the_meter() {
        assert!((amplitude_to_meter(1.0, -60.0) - 1.0).abs() < 0.001);
    }

    #[test]
    fn half_amplitude_is_about_minus_six_db() {
        // -6 dB of -60 dB floor => 90% of the way up.
        let m = amplitude_to_meter(0.5, -60.0);
        assert!((m - 0.9).abs() < 0.01, "got {m}");
    }

    #[test]
    fn easing_is_bounded_and_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        let mut previous = 0.0;
        for step in 0..=10 {
            let value = ease_out_cubic(step as f32 / 10.0);
            assert!(value >= previous);
            previous = value;
        }
    }

    #[test]
    fn whole_notches_step_immediately() {
        let mut wheel = WheelSteps::default();
        assert_eq!(wheel.feed(120), 1);
        assert_eq!(wheel.feed(-240), -2);
    }

    #[test]
    fn fractional_deltas_add_up() {
        let mut wheel = WheelSteps::default();
        let steps: i32 = (0..6).map(|_| wheel.feed(40)).sum();
        assert_eq!(steps, 2);
    }

    #[test]
    fn reversing_discards_the_partial_notch() {
        let mut wheel = WheelSteps::default();
        assert_eq!(wheel.feed(100), 0);
        assert_eq!(wheel.feed(-120), -1);
    }

    #[test]
    fn db_formatting_has_one_decimal() {
        assert_eq!(format_db(Some(-26.0)), "-26.0 dB");
        assert_eq!(format_db(None), "");
    }
}
