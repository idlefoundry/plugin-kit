//! The source drawer shares its still parts (CA-72 R44). These tests also cover simultaneous
//! opening, key changes while warming, failure, and the lifetime of the warming thread.

use plugin_kit_editor_state::cache::{BuildError, StillCache, StillKey};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;

#[test]
fn concurrent_editors_share_one_build_even_while_it_is_pending() {
    let cache = StillCache::new();
    let starts = Arc::new(AtomicUsize::new(0));
    let open = Barrier::new(17);
    let release = Arc::new(Barrier::new(2));
    let caller = thread::current().id();
    let leases = thread::scope(|s| {
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let (cache, starts, open, release) =
                    (&cache, starts.clone(), &open, release.clone());
                s.spawn(move || {
                    open.wait();
                    cache
                        .warm(StillKey::new("part", 0, 0.5).expect("scale"), move || {
                            starts.fetch_add(1, Ordering::SeqCst);
                            release.wait();
                            (thread::current().id(), vec![1, 2, 3])
                        })
                        .expect("warm")
                })
            })
            .collect();
        open.wait();
        threads
            .into_iter()
            .map(|t| t.join().expect("editor"))
            .collect::<Vec<_>>()
    });
    assert!(leases.iter().all(|s| s.get().is_none()));
    release.wait();
    let first = leases[0].wait().expect("built");
    assert_ne!(first.0, caller, "built off the opening thread");
    for lease in &leases {
        let value = lease.wait().expect("built");
        assert!(Arc::ptr_eq(&first, &value));
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[test]
fn caller_appearance_and_exact_scale_each_make_a_distinct_key() {
    let cache = StillCache::new();
    let keys = [
        StillKey::new("part-a", 0, 0.5).expect("scale"),
        StillKey::new("part-b", 0, 0.5).expect("scale"),
        StillKey::new("part-a", 1, 0.5).expect("scale"),
        StillKey::new("part-a", 0, f64::from_bits(0.5_f64.to_bits() + 1)).expect("scale"),
    ];
    assert_eq!(keys[0].scale(), 0.5);
    let leases: Vec<_> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| cache.warm(k.clone(), move || i).expect("warm"))
        .collect();
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(*leases[i].wait().expect("built"), i);
        let reused = cache
            .warm(key.clone(), || panic!("must reuse"))
            .expect("reuse");
        assert!(Arc::ptr_eq(
            &leases[i].wait().expect("built"),
            &reused.wait().expect("reused")
        ));
    }
    for scale in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(StillKey::new("part", 0, scale).is_err());
    }
}

#[test]
fn an_old_build_finishing_does_not_replace_a_changed_key() {
    let cache = StillCache::new();
    let (release, wait) = mpsc::channel();
    let old = cache
        .warm(StillKey::new("part", 4, 1.0).expect("key"), move || {
            wait.recv().expect("released");
            "old"
        })
        .expect("warm");
    let key = StillKey::new("part", 5, 1.0).expect("key");
    let new = cache.warm(key.clone(), || "new").expect("warm");
    assert_eq!(*new.wait().expect("built"), "new");
    release.send(()).expect("release");
    assert_eq!(*old.wait().expect("built"), "old");
    let reused = cache.warm(key, || "wrong").expect("warm");
    assert!(Arc::ptr_eq(
        &new.wait().expect("built"),
        &reused.wait().expect("built")
    ));
}

#[test]
fn stills_are_freed_after_the_last_editor_releases_them() {
    let cache = StillCache::new();
    let first = cache.warm(0, || vec![1]).expect("warm");
    let second = first.clone();
    let value = first.wait().expect("built");
    let weak = Arc::downgrade(&value);
    drop(value);
    drop(first);
    assert!(weak.upgrade().is_some());
    drop(second);
    assert!(weak.upgrade().is_none());
    let next = cache.warm(0, || vec![2]).expect("warm");
    assert_eq!(next.wait().expect("built").as_slice(), &[2]);
}

#[test]
fn a_panicking_builder_finishes_with_an_error_and_can_be_retried_after_release() {
    let cache = StillCache::new();
    let first = cache
        .warm(0, || -> u8 { panic!("a failed picture") })
        .expect("warm");
    assert_eq!(first.wait(), Err(BuildError));
    assert_eq!(first.get(), Some(Err(BuildError)));
    assert_eq!(cache.warm(0, || 9).expect("reuse").wait(), Err(BuildError));
    drop(first);
    assert_eq!(
        *cache.warm(0, || 7).expect("retry").wait().expect("built"),
        7
    );
}

#[test]
fn dropping_the_last_lease_joins_a_pending_builder_even_after_the_cache_is_dropped() {
    let cache = StillCache::new();
    let (release, wait) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    let lease = cache
        .warm(0, move || {
            wait.recv().expect("released");
            flag.store(true, Ordering::SeqCst);
        })
        .expect("warm");
    drop(cache);
    let (dropping, began) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let closer = thread::spawn(move || {
        dropping.send(()).expect("closing");
        drop(lease);
        done.send(()).expect("closed");
    });
    began.recv().expect("closing");
    assert_eq!(completed.try_recv(), Err(mpsc::TryRecvError::Empty));
    release.send(()).expect("release");
    completed.recv().expect("closed");
    assert!(finished.load(Ordering::SeqCst));
    closer.join().expect("closer");
}

#[test]
fn two_waiters_both_get_the_completed_value() {
    let cache = StillCache::new();
    let (release, wait) = mpsc::channel();
    let first = cache
        .warm(0, move || {
            wait.recv().expect("released");
            42
        })
        .expect("warm");
    let second = first.clone();
    thread::scope(|scope| {
        let a = scope.spawn(|| first.wait());
        let b = scope.spawn(|| second.wait());
        release.send(()).expect("release");
        let a = a.join().expect("waiter").expect("built");
        let b = b.join().expect("waiter").expect("built");
        assert!(Arc::ptr_eq(&a, &b));
    });
}
