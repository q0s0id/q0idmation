//! Reveal timing shared by the desktop and browser playback bars.
pub const HIDE_AFTER: f64 = 3.0;
pub const SLIDE_SECONDS: f32 = 0.22;

#[derive(Default)]
pub(crate) struct Reveal {
    last_activity: Option<f64>,
}

impl Reveal {
    pub fn show(&mut self, now: f64) {
        self.last_activity = Some(now);
    }

    pub fn visible(
        &mut self,
        now: f64,
        playing: bool,
        inside: bool,
        active: bool,
        held: bool,
        pinned: bool,
    ) -> bool {
        if active || held || self.last_activity.is_none() {
            self.show(now);
        }
        pinned
            || !playing
            || held
            || (inside && now - self.last_activity.unwrap_or(now) < HIDE_AFTER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_hides_after_rest_and_movement_reveals_it() {
        let mut bar = Reveal::default();
        assert!(bar.visible(10.0, true, true, false, false, false));
        assert!(!bar.visible(13.1, true, true, false, false, false));
        assert!(bar.visible(14.0, true, true, true, false, false));
        assert!(!bar.visible(14.1, true, false, false, false, false));
    }

    #[test]
    fn pause_hover_and_pin_keep_controls_accessible() {
        let mut bar = Reveal::default();
        bar.show(0.0);
        assert!(bar.visible(20.0, false, false, false, false, false));
        assert!(bar.visible(20.0, true, true, false, true, false));
        assert!(bar.visible(30.0, true, false, false, false, true));
        assert!(!bar.visible(30.0, true, true, false, false, false));
    }
}
