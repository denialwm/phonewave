use super::*;

fn request(op: u32, sid: u64, handle: &[u8], current: &[u8], desired: &[u8]) -> Request {
    Request {
        op,
        challenge: if op == VERIFY { 77 } else { 0 },
        sid,
        handle: handle.to_vec(),
        current: current.to_vec(),
        desired: desired.to_vec(),
    }
}

fn enroll(store: &Store, pin: &[u8]) -> (u64, Vec<u8>) {
    match store
        .handle(&request(ENROLL, 0, &[], &[], pin), 0, 0)
        .unwrap()
    {
        Outcome::Enrolled { sid, handle } => (sid, handle),
        other => panic!("{other:?}"),
    }
}

#[test]
fn one_pin_per_phone_and_changes_keep_the_secure_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path());
    let (sid, first) = enroll(&store, b"123456");
    assert_eq!(
        store
            .handle(&request(ENROLL, 0, &[], &[], b"999999"), 0, 0)
            .unwrap(),
        Outcome::Refused(libc::EEXIST)
    );
    let Outcome::Verified {
        sid: verified,
        token,
    } = store
        .handle(&request(VERIFY, sid, &first, b"123456", &[]), 0, 5)
        .unwrap()
    else {
        panic!("the PIN should verify");
    };
    assert_eq!(verified, sid);
    assert_eq!(token.len(), TOKEN_SIZE);
    assert_eq!(u64_at(&token, 1), 77);
    assert_eq!(u64_at(&token, 9), sid);
    assert_eq!(token[25..29], [0, 0, 0, 1]);
    let Outcome::Enrolled {
        sid: changed,
        handle: second,
    } = store
        .handle(&request(CHANGE, sid, &first, b"123456", b"654321"), 0, 0)
        .unwrap()
    else {
        panic!("the change should be accepted");
    };
    assert_eq!(changed, sid);
    assert_eq!(
        store
            .handle(&request(VERIFY, sid, &first, b"654321", &[]), 0, 0)
            .unwrap(),
        Outcome::Refused(libc::ESTALE),
        "the old handle no longer works"
    );
    assert!(matches!(
        store
            .handle(&request(VERIFY, sid, &second, b"654321", &[]), 0, 0)
            .unwrap(),
        Outcome::Verified { .. }
    ));
}

#[test]
fn forged_handles_and_secure_ids_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path());
    let (sid, mut handle) = enroll(&store, b"123456");
    assert_eq!(
        store
            .handle(&request(VERIFY, sid ^ 1, &handle, b"123456", &[]), 0, 0)
            .unwrap(),
        Outcome::Refused(libc::ESTALE)
    );
    handle[30] ^= 1;
    assert_eq!(
        store
            .handle(&request(VERIFY, sid, &handle, b"123456", &[]), 0, 0)
            .unwrap(),
        Outcome::Refused(libc::ESTALE)
    );
}

#[test]
fn every_fifth_wrong_pin_waits_and_the_wait_survives_restarts() {
    let directory = tempfile::tempdir().unwrap();
    let (sid, handle) = enroll(&Store::at(directory.path()), b"123456");
    for attempt in 1..=5 {
        assert_eq!(
            Store::at(directory.path())
                .handle(&request(VERIFY, sid, &handle, b"000000", &[]), 1000, 0)
                .unwrap(),
            Outcome::Rejected,
            "attempt {attempt}"
        );
    }
    let store = Store::at(directory.path());
    assert_eq!(
        store
            .handle(&request(VERIFY, sid, &handle, b"123456", &[]), 11_000, 0)
            .unwrap(),
        Outcome::Throttled(20_000)
    );
    assert_eq!(
        store
            .handle(&request(VERIFY, sid, &handle, b"123456", &[]), 500, 0)
            .unwrap(),
        Outcome::Throttled(30_000),
        "a clock set back waits the whole time"
    );
    assert!(matches!(
        store
            .handle(&request(VERIFY, sid, &handle, b"123456", &[]), 31_000, 0)
            .unwrap(),
        Outcome::Verified { .. }
    ));
    assert_eq!(
        store
            .handle(&request(VERIFY, sid, &handle, b"000000", &[]), 31_000, 0)
            .unwrap(),
        Outcome::Rejected,
        "a match cleared the count"
    );
}

#[test]
fn schedule_matches_android() {
    assert_eq!(timeout_ms(4), 0);
    assert_eq!(timeout_ms(5), 30_000);
    assert_eq!(timeout_ms(6), 0);
    assert_eq!(timeout_ms(10), 30_000);
    assert_eq!(timeout_ms(11), 30_000);
    assert_eq!(timeout_ms(40), 60_000);
    assert_eq!(timeout_ms(140), DAY_MS);
}

#[test]
fn headers_follow_the_hardware_providers() {
    let mut header = [0u8; HEADER_SIZE];
    header[..8].copy_from_slice(b"NXGK1\0\0\0");
    header[8..12].copy_from_slice(&VERIFY.to_le_bytes());
    header[24..32].copy_from_slice(&5u64.to_le_bytes());
    header[32..36].copy_from_slice(&56u32.to_le_bytes());
    header[36..40].copy_from_slice(&6u32.to_le_bytes());
    assert_eq!(parse_header(&header), Some((VERIFY, 0, 5, [56, 6, 0])));
    header[40..44].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(parse_header(&header), None, "a verify carries no new PIN");
    let reply = reply(&Outcome::Throttled(7));
    assert_eq!(reply.len(), REPLY_SIZE);
    assert_eq!(&reply[..8], b"NXGR1\0\0\0");
    assert_eq!(u32_at(&reply, 8) as i32, -libc::EAGAIN);
    assert_eq!(u32_at(&reply, 12) as i32, -2);
    assert_eq!(u32_at(&reply, 16), 7);
    assert_eq!(u32_at(&reply, 20), UID);
}
