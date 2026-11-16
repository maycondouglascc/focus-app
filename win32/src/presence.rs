//! Pure rules for when the user is away. No Windows calls here, so every
//! rule is unit-tested; main.rs feeds it the real events.

/// Reasons recording is paused. Several can hold at once.
pub const IDLE: u8 = 1;
pub const LOCKED: u8 = 2;
pub const SLEEP: u8 = 4;
pub const DISPLAY_OFF: u8 = 8;

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// Nothing visible changed (already paused, or still paused by another reason).
    None,
    /// First reason appeared: close the current run.
    Paused,
    /// Last reason cleared: resync with the foreground window.
    Resumed,
}

#[derive(Default)]
pub struct Presence {
    reasons: u8,
}

impl Presence {
    pub fn set(&mut self, reason: u8, on: bool) -> Change {
        let was_paused = self.paused();
        if on {
            self.reasons |= reason;
        } else {
            self.reasons &= !reason;
        }
        match (was_paused, self.paused()) {
            (false, true) => Change::Paused,
            (true, false) => Change::Resumed,
            _ => Change::None,
        }
    }

    pub fn has(&self, reason: u8) -> bool {
        self.reasons & reason != 0
    }

    pub fn paused(&self) -> bool {
        self.reasons != 0
    }
}

/// Milliseconds since the last input. GetLastInputInfo and GetTickCount are
/// 32-bit and wrap after 49.7 days, so subtract with wrapping on the same clock.
pub fn idle_ms(now_tick: u32, last_input_tick: u32) -> u64 {
    now_tick.wrapping_sub(last_input_tick) as u64
}

/// True when the machine slept without telling us: wall-clock time advanced
/// by more than the unbiased interrupt time (which does not count sleep).
pub fn slept_between(
    prev_wall_ms: i64,
    prev_unbiased_ms: i64,
    wall_ms: i64,
    unbiased_ms: i64,
    limit_ms: i64,
) -> bool {
    (wall_ms - prev_wall_ms) - (unbiased_ms - prev_unbiased_ms) > limit_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_reason_pauses_last_one_resumes() {
        let mut p = Presence::default();
        assert_eq!(p.set(IDLE, true), Change::Paused);
        assert_eq!(p.set(LOCKED, true), Change::None);
        assert_eq!(p.set(IDLE, false), Change::None);
        assert!(p.paused());
        assert_eq!(p.set(LOCKED, false), Change::Resumed);
        assert!(!p.paused());
    }

    #[test]
    fn repeating_a_reason_is_a_no_op() {
        let mut p = Presence::default();
        assert_eq!(p.set(SLEEP, true), Change::Paused);
        assert_eq!(p.set(SLEEP, true), Change::None);
        assert_eq!(p.set(DISPLAY_OFF, false), Change::None);
        assert!(p.has(SLEEP));
    }

    #[test]
    fn idle_survives_tick_counter_wraparound() {
        // Last input just before the 32-bit counter wrapped, now just after.
        assert_eq!(idle_ms(500, u32::MAX - 499), 1000);
        assert_eq!(idle_ms(10_000, 4_000), 6_000);
        assert_eq!(idle_ms(7, 7), 0);
    }

    #[test]
    fn detects_sleep_without_notification() {
        // Awake the whole minute: both clocks advance together.
        assert!(!slept_between(0, 0, 60_000, 60_000, 30_000));
        // Slept 2 h: wall moved 2 h, the unbiased clock barely moved.
        assert!(slept_between(0, 0, 7_200_000, 60_000, 30_000));
        // A small drift under the limit is ignored.
        assert!(!slept_between(0, 0, 60_000, 45_000, 30_000));
    }
}
