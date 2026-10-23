# baseview, patched

The `baseview` crate from [RustAudio/baseview](https://github.com/RustAudio/baseview) at
commit `9a0b42c09d712777b2edb4c5e0cb6baf21e988f0` (MIT OR Apache-2.0, `LICENSE-MIT`,
`LICENSE-APACHE`), the revision the editor took from git until 0.1.0. The examples, the CI
and the manifest's `[[example]]` table are left out; its workspace holds only the resolver
setting its own tests need (so that a test build on macOS or Windows does not turn on the X11
backend's features), and the dev-dependencies are the tests' below. The manifest names the
repository, for the notices, and allows two of upstream's own
compiler warnings (`non_snake_case` in `src/win/drop_target.rs`, and
`mismatched_lifetime_syntaxes`). `src/macos/window.rs` has one `use` list in the order
today's rustfmt gives it, so that `cargo fmt --all` leaves the tree as it is.

Six changes for Windows (1 to 3, 5, 6 and 9), in `src/win/window.rs`, `src/win/drop_target.rs`,
`src/win/escape.rs`, `src/event.rs` and `src/window.rs` (the first three the CA-72's
`docs/decisions.md` R26, R28 and R34), four for macOS (4, 7, 8 and 11) and two for Linux (10 and
12, below). Changes 8 to 12 are the CA-72's, numbered as it numbers them (`docs/decisions.md`
K-CA72-0.2.8); their tests are here, run as each says, and the kit's CI runs them.

1. **The window procedure never calls the handler while it is busy.** baseview keeps the
   window's handler in a `RefCell` and borrows it for each event. A message can be sent to
   the window from within one of the handler's own calls: a VST3 host may resize the
   editor's window from within `IPlugFrame::resizeView()`, which the editor asks for from its
   event handler; `SetFocus` sends focus messages; a modal loop dispatches the window's
   messages. For any such message `wnd_proc` ran the deferred tasks (a resize the handler
   had asked for: `SetWindowPos`, so a `WM_SIZE`), and `WM_SIZE` borrowed the handler again:
   a panic inside the window procedure, which cannot unwind, so the host's process aborted
   (0xC0000409). The CA-72 0.1.0 did this in Sonar as its presets' drawer opened. Now:
   - an event that arrives while the handler is busy waits in a queue (`pending_events`),
     and a frame then is skipped;
   - the waiting events, then the deferred tasks, run only when the handler is not busy: in
     the `wnd_proc` call whose handler call has just returned (or after a drag and drop
     event). Each in its order; an event waiting is given before a task asked for earlier
     runs (a `Resize` after the `Resized` of a host's `WM_SIZE`), which is harmless;
   - `Window::focus()` is a deferred task, as upstream did in
     [#252](https://github.com/RustAudio/baseview/pull/252) (2026-05-25): "a panic will
     occur when calling `Window.focus` while handling events";
   - `WM_MOUSEMOVE` no longer keeps the pointer's "outside" flag borrowed across the
     handler's call;
   - a drag and drop event that finds the handler busy is dropped and the drop refused;
   - each `wnd_proc` call holds a reference to the window's state until it returns. Closing
     the window (`DestroyWindow` for `BV_WINDOW_MUST_CLOSE`) sends `WM_NCDESTROY` from within
     the call, which freed the state while that call still read it afterwards (upstream
     did, through its deferred tasks), and a host destroying its window while the handler
     was busy freed the handler under its own borrow. `WM_NCDESTROY` also empties both
     queues.

   #252 alone does not do: a host that resizes the editor's window from within
   `resizeView()` still delivered `WM_SIZE` to the busy handler (the CA-72's tests
   `the_drawer_opens_and_shuts_where_the_host_resizes_the_editors_window` and
   `the_grip_resizes_where_the_host_resizes_the_editors_window` abort with #252's change
   only).

2. **The handler is told when Windows asks for the window to be drawn again.** baseview left
   `WM_PAINT` to `DefWindowProcW`, which validates the window without drawing anything, and
   told the handler nothing. Whatever invalidates the window (a host repainting its own, a
   capture with `PrintWindow`) has Windows ask, and a parent window without
   `WS_CLIPCHILDREN` (a dialog, as a host's may be) has by then painted its background over
   it; a handler that draws only when its frame changes, as the CA-72's editor does, left
   that there (in REAPER's FX window, 240, 240, 240, until something in the panel changed). Now
   `WM_PAINT` gives the handler a new event, `WindowEvent::Damaged`, as any event is given
   (queued while the handler is busy, and given once its call returns), and `DefWindowProcW`
   then validates what the handler has not drawn (softbuffer validates the window as it
   presents). macOS keeps a window's pixels and sends nothing; on X11 baseview ignored
   `Expose` (the CA-72's editor shows its frame again every 250 ms there, R19) until change 12.

   The CA-72's test `the_panel_is_shown_again_as_windows_repaints_its_window` fails without
   this change (the host's background left on the editor's window).

3. **The window takes every key while its handler holds the keyboard.** A host's window may be
   a dialog (REAPER's FX window is), whose message loop asks the window with the keyboard
   which keys it takes (`WM_GETDLGCODE`) and keeps the others for itself; baseview left that
   to `DefWindowProcW`, which takes none of them. So with the CA-72's presets' drawer or MIDI
   Learn holding the keyboard, REAPER 7.82 on Windows moved its focus to one of its own buttons
   at an arrow key, and the drawer's list never had it. Now `Window::set_wants_keys(true)`
   has the window answer `DLGC_WANTALLKEYS` (and the arrows, Tab and characters), and `false`,
   the default, leaves the question to `DefWindowProcW` as before, so that the dialog keeps its
   keys while the handler does not want them. On macOS and X11 the call does nothing. REAPER's
   CLAP window processes Escape before this question; change 6 handles that case.

   The CA-72's test `the_editor_asks_a_dialog_for_every_key_only_while_it_holds_them` checks
   the answer with the drawer and a control's menu open and shut.

And one for macOS, in `src/macos/view.rs` (the CA-72's `docs/decisions.md` R56, 2026-10-11):

4. **A press or release tells the handler where the pointer is first.** baseview's
   `mouseDown:` and its kind gave the handler the button alone, leaving the pointer where the
   last `mouseMoved:` had put it. Bitwig Studio's floating plug-in window, while its own window
   is in front, gets no `mouseMoved:` as the pointer comes onto it, and its first click (which
   `acceptsFirstMouse:` lets through, as it should) came before the handler knew the pointer was
   there: the CA-72's editor took it where the pointer had last been seen, and a click on a
   switch did nothing until a second one. Now each press and release gives the handler a
   `CursorMoved` at the event's own place first (`cursor_at`, which `mouseMoved:` uses too). Tried in Bitwig 5.2.7 on macOS 27: a click
   in Bitwig's window, then one on the CA-72's RETRIGGER, three times, each switched it (none
   did before); a drag begun the same way moved VOICES.

5. **On Windows too, a press or release tells the handler where the pointer is first.** The
   fourth change took Windows to tell a window of the pointer before a press; it does not
   always. A press that comes with no `WM_MOUSEMOVE` before it (the pointer set where it is, as
   a test's harness sets it, or a window come under a pointer that has not moved since) reached
   the handler with the pointer where it had last been seen: in Cubase 15, a click on the
   CA-72's POLY tab, the pointer put there at once, did nothing, and the next click took. Now
   `WM_LBUTTONDOWN` and the other buttons' messages give the handler a `CursorMoved` at the
   message's own place (its `lParam`, as `WM_MOUSEMOVE` reads it) first. The count of buttons held
   (which keeps the pointer captured) is read after that call, so that a press or release a
   modal loop run from the handler lets through is still counted. The CA-72's test
   `a_press_with_no_move_before_it_lands_where_it_is` (`crates/ca72-plugin/src/editor.rs`,
   Windows) presses POLY with no move before it; without this change nothing is set.

6. **A focused editor that holds the keyboard gets Escape before the host does** (in
   `src/win/escape.rs` and `src/win/window.rs`, 2026-10-11, the CA-74's). In REAPER
   7.82, CLAP, Escape closed the CA-74's FX window with QUALITY's menu, settings, or a readout
   edit open. The existing dialog answer was already `0x87` (`DLGC_WANTALLKEYS` and the other
   flags), and the baseline window-procedure trace showed neither an Escape message nor a
   dialog-code query before the host closed the window. Changing that answer cannot fix a
   key the host consumes before dispatching or querying it. Microsoft's
   [GetMsgProc contract](https://learn.microsoft.com/en-us/windows/win32/winmsg/getmsgproc)
   allows a `WH_GETMESSAGE` hook to see and replace a retrieved message before the host's
   processing; [Surge's REAPER CLAP fix](https://github.com/surge-synthesizer/surge/pull/8368)
   uses a message hook too. No code from that implementation is copied here.

   Each DLL and GUI thread now registers a hook for its parented windows. It delivers only a
   removed `WM_KEYDOWN` for Escape, for the focused registered HWND, while its existing
   `wants_keys` flag is true. It first replaces the queued message with `WM_NULL`, then sends
   the original to that window procedure, holding its state alive and no registry borrow
   across the handler call. Peeks, other windows and keys, and idle Escape keep their old
   path. The last window's destruction or GUI thread teardown removes the hook; window
   destruction after the registry's TLS teardown safely skips the unavailable registry.
   All this code is Windows-only; neither macOS nor Linux behavior changes.

   Three native Windows tests cover retrieval, peeking, focus, idle Escape, Space, multiple
   editors, destruction inside the handler and destruction after thread-local teardown.
   Disabling hook registration makes the two retrieval tests fail; all three pass with it.
   The renamed Highstead ESC build passed REAPER 7.82 CLAP menu and readout Escape, Enter and
   Backspace; idle Escape closes the FX window and Space controls transport, as in its VST3.
   REAPER VST3 menu Escape and readout typing, and Cubase Pro 15 VST3 menu Escape also passed.
   The manifest enables winapi's `processthreadsapi` for the GUI thread ID and allows upstream's
   existing manual `MouseCursor::default` under `derivable_impls` for standalone Clippy checks.

7. **On macOS, the pointer's look is set** (the CA-72's `docs/decisions.md` R-MAC-FIXES).
   Upstream's macOS `Window::set_mouse_cursor` was `todo!()`: a call panicked, so the editor
   could not show the resize arrows over its grip there (R-SIZE). Now `src/macos/cursor.rs`
   maps each `MouseCursor` to an `NSCursor` (the diagonal resize arrows to AppKit's
   `_windowResizeNorthWestSouthEastCursor` and `…NorthEastSouthWest…`, the ones its own window
   corners use, asked for only where `NSCursor` answers to them; looks AppKit has nothing like,
   the arrow). `src/macos/window.rs` keeps the look asked for (`WindowInner::cursor`) and shows
   it; `src/macos/view.rs` shows it again on each `mouseMoved:` (a host's window may have set
   its own) and in a new `cursorUpdate:` (the tracking area already asked for it). Windows and
   Linux are unchanged.

8. **On macOS, the view's layer takes its window's scale** (in `src/macos/view.rs`, the CA-72's
   `docs/decisions.md` R-TWO-SCREENS, its issue #48). As the window moves to a screen of another
   scale (a Retina laptop's to a 1x monitor's), AppKit tells the view
   (`viewDidChangeBackingProperties:`, whose handler sends `Resized` at the new scale) but leaves
   the layer-backed view's layer at the scale it was made with; calling NSView's own implementation
   first does not change that. A softbuffer surface lays its frame out at that layer's scale (its
   sublayer follows the view's layer's `contentsScale`), so the editor's frame, drawn for the new
   scale, showed at half its size in the window's top left corner on the 1x monitor, whatever the
   host, until the editor was closed (the other way, by the same arithmetic, it would show at twice
   its size, cut off; not seen). Now the handler sets the layer's `contentsScale` to the window's
   `backingScaleFactor`, with no animation, before it sends `Resized`, whether or not the size in
   pixels changed (a window opened on a 1x screen with its layer made at 2 sends none). And
   `becomeFirstResponder` no longer messages a nil window: `viewWillMoveToWindow:` makes the view
   its new window's first responder before the view is in it, and a debug build's check of null
   pointers stopped the process there (the message to nil answered NO, as it still does).

   `tests/backing_scale.rs` (the CA-72's, macOS, on the main thread: `harness = false`) opens
   baseview's view in a window, leaves the view's layer at the other scale as AppKit does, tells
   the view, and checks that the layer and softbuffer's take the window's scale and that the frame
   then shown fills the view; without the layer's scale set it fails ("the view's layer left at
   another scale"). The CA-72 tried it on macOS 27 with a Retina screen and a 4K monitor switched
   to 1x: its standalone and Ableton Live 12.2.7 (VST3) drew at half size on the 1x monitor
   before, and whole after.

And one more for Windows, in `src/win/window.rs` (the CA-72's `docs/decisions.md`
R-WIN-STANDALONE, its issue 64, 2026-10-18):

9. **A window of its own has no frame to drag and no maximise box.** A window baseview opens
   with no parent (an application's, as nih-plug's standalones open theirs) had `WS_SIZEBOX` and
   `WS_MAXIMIZEBOX`. Its handler sizes what it holds and is told nothing it can follow when the
   frame is dragged or the window maximised: the CA-72's editor stayed as it was, in the
   window's top left corner, black beyond it (or cut off, the window made smaller). Now such a
   window has neither; its handler sizes it (`Window::resize`), as on macOS, whose window
   baseview never made resizable. A parented window (a plug-in's editor in a host) is
   unchanged. `a_window_of_its_own_has_no_frame_to_drag_and_no_maximise_box` (in
   `src/win/window.rs`, Windows, in a desktop session) opens one of each; with the two styles
   put back it fails ("a frame to drag"). The CA-72's own test of its standalone's window at
   every scale stays with it.

   This copy is also the one nih-plug's standalones open their window with since nih-plug's
   change 21 (`../nih-plug/PATCHES.md`), without the `opengl` feature, which neither window
   uses; they had taken upstream's `579130e` from git, whose X11 backend is this one's and
   whose Windows and macOS backends lack this copy's changes.

And one more for Linux, in `src/x11/` (the CA-72's issue 49, R-LINUX-ICON, 2026-10-18):

10. **Linux standalone windows identify themselves before mapping.** The X11 backend mapped its
   window before setting `WM_NAME` and never set `WM_CLASS`, so a dock could not match it to the
   application's desktop entry (`StartupWMClass`) and showed no icon for it. It now sets the
   application's class (`set_application_class`, the first name given) or, without one, the
   initial title as both the instance and class (`STRING/8`, two NUL-terminated strings), then
   its title, then maps. A window titled otherwise, such as an error window, so keeps the
   application's class. Only top-level windows receive the class; parented plug-in editors
   leave application identity to their host. Six portable tests
   (`src/x11/window_identity/tests.rs`, on every platform, x11rb a dev-dependency) record the
   actual X11 requests, covering the payload, order, parented windows, a title alone, a window
   titled apart from its application, the first class staying, and failure before mapping.
   The CA-72's check that its own name is its launcher's `StartupWMClass` stays with it.

And one more for macOS, in `src/macos/application.rs` and `src/macos/window.rs` (the CA-72's
`docs/decisions.md` R-QUIT, 2026-10-19):

11. **A window of its own is an application's: its menus, and quitting it closes its window.**
   `open_blocking` gave the application no main menu, so no Quit item and no ⌘Q: the key went
   to the view, whose handler had no use for it, and the CA-72's standalone could be closed
   only by its window's close button or the Dock's Quit. And AppKit's own quitting
   (`terminate:`, which the Dock's Quit and logging out reach through the quit Apple event)
   ends the process within AppKit, so nothing after `open_blocking` would run. Now the
   application gets, once a process, the menus every macOS application has (its own: About,
   Hide ⌘H, Hide Others ⌥⌘H, Show All, Quit ⌘Q; and a Window menu: Minimize ⌘M, Close ⌘W),
   named with the window's title, and a delegate. `applicationShouldTerminate:` (the Quit item,
   ⌘Q) and the quit Apple event, whose handler the delegate installs in place of AppKit's in
   `applicationWillFinishLaunching:` (the Dock's Quit, AppleScript's `quit`, logging out,
   restarting, shutting down), each close baseview's windows as their close buttons do (the
   view, each window's delegate, asked `windowShouldClose:`), so that `open_blocking` returns
   as it does then. The first answers `NSTerminateCancel`, leaving the process to end of
   itself; the Apple event is answered with no error, where AppKit's cancelled `terminate:`
   would answer `userCanceledErr` and cancel a logout. AppKit takes a key with Command for the
   main menu before any view, so ⌘Q quits whatever the handler does with the keys. Two fixes
   that closing the window from these needs: `close()` posts an event after `stop:`, which
   `run` notices only as an event ends (a close from an Apple event or a timer left it waiting
   for one); and the frame timer holds a reference to the window's state for the frame, as
   the view's other callbacks do (`from_view`), so that a handler that closes a window of its
   own in `on_frame` no longer frees the state under the frame. A parented window (a plug-in's
   editor in a host) is unchanged: none of this runs for it.

   `tests/mac_quit.rs` (the CA-72's, macOS, on the main thread) opens a window of its own,
   posts ⌘Q as the keyboard gives it, and checks that the window closed, `open_blocking`
   returned and the view never had the key; checks the menus' titles, keys, modifiers and
   actions; and sends the quit Apple event from another process, as the Dock does, awaiting its
   answer: the window closed, answered with no error. With the delegate's Apple event handler
   left out, it fails ("0 -128" for "0 none").

And one more for Linux, in `src/x11/event_loop.rs` (the CA-72's `docs/decisions.md`
R-LINUX-RESIZE, its issue #65, 2026-10-20):

12. **On X11, the handler is told when the X server asks for the window to be drawn again.**
   `EXPOSURE` was in the window's event mask, but `Expose` fell through to `_ => {}`. Now an
   `Expose` (any of a series) marks the events being drained, and once they are handled, after
   the `Resized` they may have brought, the handler gets `WindowEvent::Damaged`, once, as
   change 2 gives it on Windows. Under XWayland the editor's window (32 bits deep, so kept in a
   pixmap of its own by the Composite extension) is sent one as it is mapped and each time it
   takes a new size, and never as it is uncovered, the server putting its kept pixels back
   itself; a server without compositing sends one where it was covered, too. In Bitwig 5.2.7 on
   Linux, which grows its plug-in window after the editor's own has grown, the CA-72's panel
   grown at a click left its lower part black for about 250 ms, until its periodic repaint;
   shown again at the `Damaged` that follows, it was whole 8 to 40 ms after the click.

   `tests/x11_expose.rs` (Linux, ignored: it needs an X server, and opens only its own windows,
   off the screen and unmanaged; `-- --ignored` runs it) grows a window parented in a host's
   window of its own three times and waits for `Damaged` at its new size; with the `Expose` arm
   taken out it times out. The CA-72's test of its editor's panel
   (`a_panel_grown_is_shown_again_as_x_asks`) stays with it.

Upstream's master has since rewritten the Windows backend and the handler's interface; to
move to it, the editor must be ported, and the tests above show whether it still needs this.
The rewrite tells its handler of each `WM_PAINT` and draws then (`WindowHandler::damage`,
[#344](https://github.com/RustAudio/baseview/pull/344), 2026-10-04), as the second change does.
