//! Pen pressure that the host reads from the system itself (#764).
//!
//! winit 0.30 hands a pen's pressure to egui only on Windows, as the force of a touch event; the
//! web build makes the same touch events from the browser's Pointer Events. On Linux under X11
//! (XWayland included), winit reports a pen as a mouse, so the desktop app reads the pressure
//! from XInput2 on a connection of its own and writes it into a [`StylusFeed`]
//! ([`crate::Services::stylus`]). The canvas uses it when a frame's events carry no touch force.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How long a press waits for the touch of a pen that hovers ([`StylusFeed::press`]).
const PRESS_WAIT: Duration = Duration::from_millis(5);

/// What the feed holds.
#[derive(Clone, Copy, Debug, Default)]
struct Pen {
    /// The pressure of the pen when a pen was the last pointer to move.
    latest: Option<f32>,
    /// The highest pressure set since the UI last read the feed, counted from the latest sample
    /// whose pressure rose above 0.
    peak: Option<f32>,
    /// The last pressure above 0 set since a pen was the last pointer to move (0 when it has not
    /// touched).
    touched: f32,
}

impl Pen {
    /// A pen hovers, and no touch has been set since the UI last read the feed.
    fn hovering(&self) -> bool {
        self.latest == Some(0.0) && self.peak.is_none()
    }
}

#[derive(Default)]
struct Shared {
    pen: Mutex<Pen>,
    /// Signaled on every [`StylusFeed::set`].
    set: Condvar,
}

/// The pen pressure the host last read, written from any thread and read on the UI thread. Cheap
/// to clone; the clones share the value.
#[derive(Clone, Default)]
pub struct StylusFeed(Arc<Shared>);

impl StylusFeed {
    fn lock(&self) -> MutexGuard<'_, Pen> {
        self.0.pen.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A pen moved with `pressure` (0..1; 0 while it hovers), or `None`: another pointer, such as
    /// a mouse, moved. A pressure outside 0..1 is clamped, and one that is not a number is a
    /// mouse's, 1.
    pub fn set(&self, pressure: Option<f32>) {
        let pressure = pressure.map(|p| if p.is_finite() { p.clamp(0.0, 1.0) } else { 1.0 });
        let mut pen = self.lock();
        match pressure {
            Some(p) if p > 0.0 => {
                let touching = pen.latest.is_some_and(|l| l > 0.0);
                pen.peak = Some(if touching { pen.peak.map_or(p, |q| q.max(p)) } else { p });
                pen.touched = p;
            }
            Some(_) => {}
            None => {
                pen.peak = None;
                pen.touched = 0.0;
            }
        }
        pen.latest = pressure;
        drop(pen);
        self.0.set.notify_all();
    }

    /// The pressure of a press the UI holds (a drag and its release) when a pen was the last
    /// pointer to move, otherwise `None`: the latest pressure, or, when the pen has lifted (0), the
    /// last pressure it touched with. The lift can reach the feed before the UI handles the last
    /// moves and the release of the press, which then keep the pen's pressure, not the lift's 0.
    /// The pressures set so far count as read for [`Self::press`].
    pub fn held(&self) -> Option<f32> {
        let mut pen = self.lock();
        pen.peak = None;
        let touched = pen.touched;
        pen.latest.map(|l| if l > 0.0 { l } else { touched })
    }

    /// The pressure of a press when a pen was the last pointer to move, otherwise `None`: the
    /// highest pressure set since the last read, counted from the latest touch, or else the latest
    /// pressure. A quick tap can touch and lift before the frame that handles its press, which
    /// then gets the tap's pressure, not the lift's 0. The host's reader can also set the touch
    /// just after the UI handles the press: when the pen still hovers, the press waits up to 5 ms
    /// for it, and gets 0 if none comes.
    pub fn press(&self) -> Option<f32> {
        self.press_within(PRESS_WAIT)
    }

    fn press_within(&self, wait: Duration) -> Option<f32> {
        let mut pen = self.lock();
        if pen.hovering() {
            pen = self.wait_for_touch(pen, wait);
        }
        let peak = pen.peak.take();
        pen.latest.map(|l| peak.unwrap_or(l))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wait_for_touch<'a>(&'a self, pen: MutexGuard<'a, Pen>, wait: Duration) -> MutexGuard<'a, Pen> {
        match self.0.set.wait_timeout_while(pen, wait, |p| p.hovering()) {
            Ok((pen, _)) => pen,
            Err(e) => e.into_inner().0,
        }
    }

    /// The browser's main thread can't wait, and the web build sets no feed.
    #[cfg(target_arch = "wasm32")]
    fn wait_for_touch<'a>(&'a self, pen: MutexGuard<'a, Pen>, _wait: Duration) -> MutexGuard<'a, Pen> {
        pen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feed_keeps_the_last_pressure_within_0_to_1() {
        let feed = StylusFeed::default();
        assert_eq!(feed.held(), None, "no pen until one moves");
        let shared = feed.clone();
        shared.set(Some(0.25));
        assert_eq!(feed.held(), Some(0.25), "the clones share the value");
        feed.set(Some(1.5));
        assert_eq!(feed.held(), Some(1.0));
        feed.set(Some(f32::NAN));
        assert_eq!(feed.held(), Some(1.0), "a pressure that is not a number is a mouse's");
        feed.set(None);
        feed.set(Some(-0.5));
        assert_eq!(feed.press_within(Duration::ZERO), Some(0.0));
        feed.set(None);
        assert_eq!((feed.held(), feed.press()), (None, None), "a mouse moved");
    }

    #[test]
    fn a_held_press_keeps_the_last_touch_when_the_lift_comes_first() {
        let feed = StylusFeed::default();
        feed.set(Some(0.0));
        assert_eq!(feed.held(), Some(0.0), "a pen that has not touched");
        // The pen moves and lifts before the UI handles the last move and the release.
        for p in [0.5, 0.6, 0.0] {
            feed.set(Some(p));
        }
        assert_eq!(feed.held(), Some(0.6));
        feed.set(Some(0.3));
        assert_eq!(feed.held(), Some(0.3), "a touch again: its own pressure");
        feed.set(None);
        assert_eq!(feed.held(), None, "a mouse moved");
        feed.set(Some(0.0));
        assert_eq!(feed.held(), Some(0.0), "the pen's touch before the mouse doesn't count");
    }

    #[test]
    fn a_press_takes_the_highest_pressure_since_the_last_read() {
        let feed = StylusFeed::default();
        feed.set(Some(0.0));
        assert_eq!(feed.press(), Some(0.0), "a hovering pen");
        // A tap: it touches, presses harder, and lifts before the frame that handles the press.
        for p in [0.4, 0.7, 0.3, 0.0] {
            feed.set(Some(p));
        }
        assert_eq!(feed.press(), Some(0.7));
        assert_eq!(feed.press(), Some(0.0), "the press read the tap: the latest pressure");
        // A new touch counts from its own pressure, not the touch before it.
        for p in [0.9, 0.0, 0.2, 0.5, 0.3] {
            feed.set(Some(p));
        }
        assert_eq!(feed.press(), Some(0.5));
        // Pressures the UI has read do not count for the next press.
        feed.set(Some(0.8));
        assert_eq!(feed.held(), Some(0.8));
        feed.set(Some(0.4));
        assert_eq!(feed.press(), Some(0.4));
        feed.set(None);
        assert_eq!(feed.press(), None, "a mouse presses with no pen pressure");
    }

    #[test]
    fn a_press_waits_for_the_touch_of_a_hovering_pen() {
        let feed = StylusFeed::default();
        feed.set(Some(0.0));
        let reader = feed.clone();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            reader.set(Some(0.6));
        });
        assert_eq!(feed.press_within(Duration::from_secs(10)), Some(0.6), "the touch set while the press waited");
        late.join().unwrap();
        // No touch comes: the hover's 0 once the wait ends.
        feed.set(Some(0.0));
        let start = std::time::Instant::now();
        assert_eq!(feed.press_within(Duration::from_millis(30)), Some(0.0));
        assert!(start.elapsed() >= Duration::from_millis(30));
        // A pen that touched, or a mouse, does not wait.
        feed.set(Some(0.3));
        let start = std::time::Instant::now();
        assert_eq!(feed.press_within(Duration::from_secs(10)), Some(0.3));
        feed.set(None);
        assert_eq!(feed.press_within(Duration::from_secs(10)), None);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
