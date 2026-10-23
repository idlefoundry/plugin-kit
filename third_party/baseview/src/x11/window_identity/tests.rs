use super::{set_identity_and_map, top_level_class};
use std::cell::RefCell;
use std::convert::TryInto;
use std::io::IoSlice;
use x11rb::connection::{
    BufWithFds, DiscardMode, ReplyOrError, RequestConnection, RequestKind, SequenceNumber,
};
use x11rb::cookie::{Cookie, CookieWithFds, VoidCookie};
use x11rb::errors::{ConnectionError, ParseError};
use x11rb::protocol::xproto::{AtomEnum, CHANGE_PROPERTY_REQUEST, MAP_WINDOW_REQUEST};
use x11rb::utils::RawFdContainer;
use x11rb::x11_utils::{ExtensionInformation, TryParse, TryParseFd, X11Error};

#[derive(Default)]
struct RecordingConnection {
    requests: RefCell<Vec<Vec<u8>>>,
    fail_after: Option<usize>,
}

impl RequestConnection for RecordingConnection {
    type Buf = Vec<u8>;

    fn send_request_without_reply(
        &self, bufs: &[IoSlice<'_>], fds: Vec<RawFdContainer>,
    ) -> Result<VoidCookie<'_, Self>, ConnectionError> {
        assert!(fds.is_empty());
        let mut requests = self.requests.borrow_mut();
        if self.fail_after == Some(requests.len()) {
            return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe).into());
        }
        requests.push(bufs.iter().flat_map(|buf| buf.iter().copied()).collect());
        Ok(VoidCookie::new(self, requests.len() as SequenceNumber))
    }

    fn discard_reply(&self, _: SequenceNumber, _: RequestKind, _: DiscardMode) {}

    fn send_request_with_reply<R: TryParse>(
        &self, _: &[IoSlice<'_>], _: Vec<RawFdContainer>,
    ) -> Result<Cookie<'_, Self, R>, ConnectionError> {
        panic!("identity setup must not request replies")
    }

    fn send_request_with_reply_with_fds<R: TryParseFd>(
        &self, _: &[IoSlice<'_>], _: Vec<RawFdContainer>,
    ) -> Result<CookieWithFds<'_, Self, R>, ConnectionError> {
        panic!("identity setup must not request file descriptors")
    }

    fn prefetch_extension_information(&self, _: &'static str) -> Result<(), ConnectionError> {
        panic!("identity setup must use core X11 requests")
    }

    fn extension_information(
        &self, _: &'static str,
    ) -> Result<Option<ExtensionInformation>, ConnectionError> {
        panic!("identity setup must use core X11 requests")
    }

    fn wait_for_reply_or_raw_error(
        &self, _: SequenceNumber,
    ) -> Result<ReplyOrError<Vec<u8>>, ConnectionError> {
        panic!("unexpected reply")
    }

    fn wait_for_reply(&self, _: SequenceNumber) -> Result<Option<Vec<u8>>, ConnectionError> {
        panic!("unexpected reply")
    }

    fn wait_for_reply_with_fds_raw(
        &self, _: SequenceNumber,
    ) -> Result<ReplyOrError<BufWithFds<Vec<u8>>, Vec<u8>>, ConnectionError> {
        panic!("unexpected reply")
    }

    fn check_for_raw_error(&self, _: SequenceNumber) -> Result<Option<Vec<u8>>, ConnectionError> {
        panic!("unexpected round trip")
    }

    fn maximum_request_bytes(&self) -> usize {
        65535 * 4
    }

    fn prefetch_maximum_request_bytes(&self) {}

    fn parse_error(&self, _: &[u8]) -> Result<X11Error, ParseError> {
        panic!("unexpected error")
    }

    fn parse_event(&self, _: &[u8]) -> Result<x11rb::protocol::Event, ParseError> {
        panic!("unexpected event")
    }
}

fn word(request: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(request[offset..offset + 4].try_into().unwrap())
}

fn assert_property(request: &[u8], atom: AtomEnum, data: &[u8]) {
    assert_eq!(request[0], CHANGE_PROPERTY_REQUEST);
    assert_eq!(request[1], 0, "replace the property");
    assert_eq!(word(request, 4), 42, "the correct window");
    assert_eq!(word(request, 8), u32::from(atom));
    assert_eq!(word(request, 12), u32::from(AtomEnum::STRING));
    assert_eq!(request[16], 8, "ICCCM STRING/8");
    assert_eq!(word(request, 20), data.len() as u32);
    assert_eq!(&request[24..24 + data.len()], data);
}

fn assert_map(request: &[u8]) {
    assert_eq!(request[0], MAP_WINDOW_REQUEST);
    assert_eq!(word(request, 4), 42);
}

#[test]
fn a_standalone_window_is_given_its_class_and_title_before_it_is_mapped() {
    let conn = RecordingConnection::default();
    set_identity_and_map(&conn, 42, "Instrument", Some("Instrument")).unwrap();
    let requests = conn.requests.borrow();
    assert_eq!(requests.len(), 3);
    assert_property(&requests[0], AtomEnum::WM_CLASS, b"Instrument\0Instrument\0");
    assert_property(&requests[1], AtomEnum::WM_NAME, b"Instrument");
    assert_map(&requests[2]);
}

#[test]
fn parented_editors_do_not_claim_a_standalone_identity() {
    let conn = RecordingConnection::default();
    set_identity_and_map(&conn, 42, "Instrument", None).unwrap();
    let requests = conn.requests.borrow();
    assert_eq!(requests.len(), 2);
    assert_property(&requests[0], AtomEnum::WM_NAME, b"Instrument");
    assert_map(&requests[1]);
}

#[test]
fn the_initial_title_supplies_the_identity_without_a_product_specific_backend() {
    let conn = RecordingConnection::default();
    let class = top_level_class("Other instrument", None);
    set_identity_and_map(&conn, 42, "Other instrument", Some(class)).unwrap();
    assert_property(
        &conn.requests.borrow()[0],
        AtomEnum::WM_CLASS,
        b"Other instrument\0Other instrument\0",
    );
}

#[test]
fn a_window_titled_otherwise_keeps_the_applications_class() {
    // A standalone's error window, titled apart from its application (the CA-72's has one).
    let conn = RecordingConnection::default();
    let class = top_level_class("Instrument — error", Some("Instrument"));
    set_identity_and_map(&conn, 42, "Instrument — error", Some(class)).unwrap();
    let requests = conn.requests.borrow();
    assert_property(&requests[0], AtomEnum::WM_CLASS, b"Instrument\0Instrument\0");
    assert_property(&requests[1], AtomEnum::WM_NAME, "Instrument — error".as_bytes());
    assert_map(&requests[2]);
}

#[test]
fn the_first_application_class_given_stays() {
    crate::set_application_class("Instrument");
    crate::set_application_class("Other instrument");
    assert_eq!(crate::application_class(), Some("Instrument"));
}

#[test]
fn failed_identity_requests_prevent_mapping() {
    for fail_after in 0..2 {
        let conn = RecordingConnection { fail_after: Some(fail_after), ..Default::default() };
        assert!(set_identity_and_map(&conn, 42, "Instrument", Some("Instrument")).is_err());
        assert!(conn.requests.borrow().iter().all(|request| request[0] != MAP_WINDOW_REQUEST));
    }
}
