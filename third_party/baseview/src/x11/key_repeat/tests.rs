use super::KeyRepeats;
use x11rb::protocol::xproto::{KeyPressEvent, KeyReleaseEvent, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::Event;

const A: u8 = 38;
const S: u8 = 39;
const WINDOW: u32 = 42;

fn press(detail: u8, time: u32) -> KeyPressEvent {
    KeyPressEvent {
        response_type: KEY_PRESS_EVENT,
        detail,
        sequence: 0,
        time,
        root: 1,
        event: WINDOW,
        child: 0,
        root_x: 0,
        root_y: 0,
        event_x: 0,
        event_y: 0,
        state: Default::default(),
        same_screen: true,
    }
}

fn release(detail: u8, time: u32) -> KeyReleaseEvent {
    KeyReleaseEvent { response_type: KEY_RELEASE_EVENT, ..press(detail, time) }
}

/// What a handler is told of `events`, each with the one after it, as the event loop reads
/// them: `(keycode, down, repeat)`.
fn told(keys: &mut KeyRepeats, events: &[Event]) -> Vec<(u8, bool, bool)> {
    let mut out = Vec::new();
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::KeyPress(e) => out.push((e.detail, true, keys.press(e))),
            Event::KeyRelease(e) => {
                out.extend(keys.release(e, events.get(i + 1)).then_some((e.detail, false, false)))
            }
            _ => {}
        }
    }
    out
}

/// Without detectable auto-repeat: A held through 30 of the server's repeats, each a release and
/// a press at one time, then let go. One press, 30 repeats, one release.
#[test]
fn the_servers_release_and_press_pairs_are_repeats() {
    let mut events = vec![Event::KeyPress(press(A, 1000))];
    for k in 0..30 {
        let t = 1660 + 40 * k;
        events.push(Event::KeyRelease(release(A, t)));
        events.push(Event::KeyPress(press(A, t)));
    }
    events.push(Event::KeyRelease(release(A, 3000)));
    let told = told(&mut KeyRepeats::default(), &events);
    assert_eq!(told.len(), 32);
    assert_eq!(told[0], (A, true, false), "the first press is no repeat");
    assert!(told[1..31].iter().all(|&t| t == (A, true, true)), "{:?}", told);
    assert_eq!(told[31], (A, false, false), "let go once, at the end");
}

/// With detectable auto-repeat: the presses alone, then the release.
#[test]
fn a_press_of_a_key_down_is_a_repeat() {
    let mut events = vec![Event::KeyPress(press(A, 1000))];
    events.extend((0..30).map(|k| Event::KeyPress(press(A, 1660 + 40 * k))));
    events.push(Event::KeyRelease(release(A, 3000)));
    let told = told(&mut KeyRepeats::default(), &events);
    assert_eq!(told[0], (A, true, false));
    assert!(told[1..31].iter().all(|&t| t == (A, true, true)));
    assert_eq!(told[31..], [(A, false, false)]);
}

/// A key let go and struck again by hand, however quickly, is struck again: the press comes at
/// a later time, or arrives only after the release has been handled.
#[test]
fn a_quick_press_again_is_no_repeat() {
    let events = [
        Event::KeyPress(press(A, 1000)),
        Event::KeyRelease(release(A, 1100)),
        Event::KeyPress(press(A, 1101)),
        Event::KeyRelease(release(A, 1200)),
    ];
    assert_eq!(
        told(&mut KeyRepeats::default(), &events),
        [(A, true, false), (A, false, false), (A, true, false), (A, false, false)]
    );
    // The release the last event read: the press after it not yet come.
    let mut keys = KeyRepeats::default();
    assert!(!keys.press(&press(A, 1000)));
    assert!(keys.release(&release(A, 1100), None));
    assert!(!keys.press(&press(A, 1100)));
}

/// Another key's press at the same time (two keys let go and struck together, or one key let
/// go as another is struck) leaves the release as it is.
#[test]
fn only_one_keys_pair_at_one_time_is_a_repeat() {
    let events = [
        Event::KeyPress(press(A, 1000)),
        Event::KeyRelease(release(A, 1200)),
        Event::KeyPress(press(S, 1200)),
        Event::KeyRelease(release(S, 1300)),
    ];
    assert_eq!(
        told(&mut KeyRepeats::default(), &events),
        [(A, true, false), (A, false, false), (S, true, false), (S, false, false)]
    );
    // Held together, each repeats alone.
    let events = [
        Event::KeyPress(press(A, 1000)),
        Event::KeyPress(press(S, 1010)),
        Event::KeyPress(press(S, 1670)),
        Event::KeyRelease(release(A, 1700)),
        Event::KeyPress(press(S, 1710)),
    ];
    assert_eq!(
        told(&mut KeyRepeats::default(), &events),
        [(A, true, false), (S, true, false), (S, true, true), (A, false, false), (S, true, true)]
    );
}

/// The keys forgotten (the focus gone): a key's release missed meanwhile, its next press is
/// struck, not taken for a repeat.
#[test]
fn a_key_forgotten_is_struck_afresh() {
    let mut keys = KeyRepeats::default();
    assert!(!keys.press(&press(A, 1000)));
    keys.forget();
    assert!(!keys.press(&press(A, 5000)));
    assert!(keys.press(&press(A, 5660)));
}
