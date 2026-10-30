use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use sofie_core::{TestClock, Timestamp, unpoisoned};

use super::blocks::{Grade, Key};
use super::detector::{Detector, Message, Safety, Signal};
use super::rule::{Rule, Thresholds};
use super::*;

const ATTACKER: &str = "203.0.113.9";

/// A decision logged while another test starts capturing can leave callsite interest stale.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    unpoisoned::lock(&SERIAL)
}

fn test_clock() -> TestClock {
    TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap())
}

fn now(clock: &TestClock) -> i64 {
    clock.clock().now().timestamp().as_microsecond()
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn caller(address: &str) -> Key {
    Key::Caller(Requester::of(None, ip(address)))
}

fn detector(dry_run: bool) -> Detector {
    Detector::new(Thresholds::default(), Safety::default(), dry_run, None)
}

fn send(detector: &mut Detector, clock: &TestClock, rule: Rule, key: Key, count: u32) {
    for _ in 0..count {
        detector.handle(Message::Signal(Signal {
            rule,
            key,
            address: ip(ATTACKER),
            area: None,
            at: now(clock),
        }));
    }
}

fn grade(detector: &Detector, clock: &TestClock, key: Key) -> Option<Grade> {
    detector
        .blocks
        .find(key, now(clock))
        .map(|block| block.grade)
}

#[test]
fn every_caller_rule_slows_down_at_its_threshold_and_blocks_at_twice_it() {
    let _serial = serial();
    for rule in Rule::ALL
        .into_iter()
        .filter(|rule| !matches!(rule, Rule::Account | Rule::AccountTotal))
    {
        let clock = test_clock();
        let mut detector = detector(false);
        let key = caller(ATTACKER);
        let threshold = Thresholds::default().limit(None, rule).count;

        send(&mut detector, &clock, rule, key, threshold - 1);
        assert_eq!(grade(&detector, &clock, key), None, "{rule:?} reports only");
        send(&mut detector, &clock, rule, key, 1);
        assert_eq!(grade(&detector, &clock, key), Some(Grade::Slow), "{rule:?}");
        send(&mut detector, &clock, rule, key, threshold);
        assert_eq!(
            grade(&detector, &clock, key),
            Some(Grade::Block),
            "{rule:?}"
        );
    }
}

#[test]
fn failed_sign_ins_to_one_account_lock_it_until_the_lock_ends() {
    let _serial = serial();
    let clock = test_clock();
    let mut detector = detector(false);
    let account = Key::AccountFrom(Uuid::now_v7().unwrap(), Requester::of(None, ip(ATTACKER)));

    send(&mut detector, &clock, Rule::Account, account, 9);
    assert_eq!(grade(&detector, &clock, account), None);
    send(&mut detector, &clock, Rule::Account, account, 1);
    assert_eq!(grade(&detector, &clock, account), Some(Grade::Lock));

    clock.advance(Duration::from_mins(15)).unwrap();
    assert_eq!(grade(&detector, &clock, account), None, "every lock ends");
}

#[test]
fn an_attacker_locks_an_account_for_its_own_network_until_the_account_wide_ceiling() {
    let _serial = serial();
    let (sender, receiver) = mpsc::sync_channel(QUEUE);
    let mut detector = detector(false);
    let sentinel = Arc::new(Sentinel {
        blocks: Arc::clone(&detector.blocks),
        sender,
        dropped: Arc::default(),
        clock: test_clock().clock(),
        slowed: Limiter::new(SLOW_RATE),
        shared: None,
    });
    let from = |address: &str| SignIns {
        request: Some(RequestShield {
            sentinel: Arc::clone(&sentinel),
            address: ip(address),
            area: "web",
        }),
    };
    let account = Uuid::now_v7().unwrap();
    let (attacker, owner) = (from(ATTACKER), from("198.51.100.7"));
    let mut fail = |sign_ins: &SignIns, times: u32| {
        for _ in 0..times {
            sign_ins.failed(Some(account));
        }
        while let Ok(message) = receiver.try_recv() {
            detector.handle(message);
        }
    };

    fail(&attacker, 10);
    assert!(attacker.locked(account));
    assert!(!owner.locked(account), "the owner signs in from elsewhere");

    for network in 0..9 {
        fail(&from(&format!("192.0.2.{network}")), 10);
    }
    assert!(
        owner.locked(account),
        "past the ceiling the whole account locks"
    );
}

#[test]
fn a_block_ends_after_its_period_and_returns_while_the_attack_goes_on() {
    let _serial = serial();
    let clock = test_clock();
    let mut detector = detector(false);
    let key = caller(ATTACKER);
    send(&mut detector, &clock, Rule::Scan, key, 60);
    assert_eq!(grade(&detector, &clock, key), Some(Grade::Block));

    clock.advance(Duration::from_mins(15)).unwrap();
    assert_eq!(grade(&detector, &clock, key), None);
    send(&mut detector, &clock, Rule::Scan, key, 60);

    assert_eq!(grade(&detector, &clock, key), Some(Grade::Block));
}

#[test]
fn signals_spread_thinner_than_the_window_never_add_up() {
    let _serial = serial();
    let clock = test_clock();
    let mut detector = detector(false);
    let key = caller(ATTACKER);

    for _ in 0..100 {
        send(&mut detector, &clock, Rule::Probe, key, 5);
        clock.advance(Duration::from_mins(1)).unwrap();
    }

    assert_eq!(grade(&detector, &clock, key), None);
}

#[test]
fn a_dry_run_reports_each_decision_and_blocks_nobody() {
    let _serial = serial();
    let telemetry = sofie_telemetry::capture();
    let clock = test_clock();
    let mut detector = detector(true);
    let key = caller(ATTACKER);

    send(&mut detector, &clock, Rule::Scan, key, 60);

    assert_eq!(grade(&detector, &clock, key), None);
    let decisions: Vec<_> = telemetry
        .events()
        .into_iter()
        .filter(|event| {
            event
                .fields
                .get("shield.dry_run")
                .is_some_and(|dry| dry == "true")
        })
        .map(|event| event.fields["shield.grade"].clone())
        .collect();
    assert_eq!(decisions, ["observe", "slow", "block"]);
}

#[test]
fn a_decision_names_its_rule_and_reason_and_the_address_by_its_network_only() {
    let _serial = serial();
    let telemetry = sofie_telemetry::capture();
    let clock = test_clock();
    let mut detector = detector(false);

    send(&mut detector, &clock, Rule::Refused, caller(ATTACKER), 10);

    let event = telemetry.events().pop().unwrap();
    assert_eq!(event.fields["shield.rule"], "refused");
    assert_eq!(event.fields["shield.grade"], "slow");
    assert_eq!(event.fields["shield.caller"], "address 203.0.113.0/24");
    assert!(!format!("{event:?}").contains(ATTACKER), "{event:?}");
    assert!(event.fields["shield.reason"].contains("cross-site"));
}

#[test]
fn an_unknown_or_loopback_address_is_never_blocked_nor_a_principal_behind_it() {
    let _serial = serial();
    let clock = test_clock();
    let mut detector = detector(false);
    let principal = Key::Caller(Requester::Principal(Uuid::now_v7().unwrap()));

    for address in ["0.0.0.0", "::", "127.0.0.1", "::1", "::ffff:127.0.0.1"] {
        let key = caller(address);
        for _ in 0..100 {
            detector.handle(Message::Signal(Signal {
                rule: Rule::Scan,
                key,
                address: ip(address),
                area: None,
                at: now(&clock),
            }));
        }
        assert_eq!(grade(&detector, &clock, key), None, "{address}");
    }
    send(&mut detector, &clock, Rule::Probe, principal, 40);
    assert_eq!(grade(&detector, &clock, principal), Some(Grade::Block));
    assert_eq!(
        grade(&detector, &clock, caller(ATTACKER)),
        None,
        "the principal is blocked, not its address"
    );
}

#[cfg(feature = "trusted-proxies")]
#[test]
fn a_trusted_proxy_is_never_blocked() {
    let _serial = serial();
    let clock = test_clock();
    let safety = Safety {
        proxies: crate::TrustedProxies::parse("10.0.0.0/8").unwrap(),
    };
    let mut detector = Detector::new(Thresholds::default(), safety, false, None);
    let proxy = caller("10.1.2.3");

    for _ in 0..100 {
        detector.handle(Message::Signal(Signal {
            rule: Rule::Protocol,
            key: proxy,
            address: ip("10.1.2.3"),
            area: None,
            at: now(&clock),
        }));
    }

    assert_eq!(grade(&detector, &clock, proxy), None);
}

#[test]
fn an_area_threshold_holds_for_its_area_only() {
    let _serial = serial();
    let clock = test_clock();
    let thresholds = Thresholds::parse("console.probe=2").unwrap();
    let mut detector = Detector::new(thresholds, Safety::default(), false, None);
    let (console, web) = (caller(ATTACKER), caller("198.51.100.1"));
    let signal = |key, area| Signal {
        rule: Rule::Probe,
        key,
        address: ip(ATTACKER),
        area: Some(area),
        at: now(&clock),
    };

    for _ in 0..2 {
        detector.handle(Message::Signal(signal(console, "console")));
        detector.handle(Message::Signal(signal(web, "web")));
    }

    assert_eq!(grade(&detector, &clock, console), Some(Grade::Slow));
    assert_eq!(grade(&detector, &clock, web), None);
}

#[test]
fn a_lifted_caller_counts_from_nothing() {
    let _serial = serial();
    let clock = test_clock();
    let mut detector = detector(false);
    let key = caller(ATTACKER);
    send(&mut detector, &clock, Rule::Malformed, key, 40);
    let id = detector.blocks.active(now(&clock))[0].0.id();

    detector.blocks.lift(&id);
    detector.handle(Message::Lifted(key));
    send(&mut detector, &clock, Rule::Malformed, key, 19);

    assert_eq!(grade(&detector, &clock, key), None);
}

fn sentinel(sender: SyncSender<Message>) -> Sentinel {
    Sentinel {
        blocks: Arc::default(),
        sender,
        dropped: Arc::default(),
        clock: test_clock().clock(),
        slowed: Limiter::new(SLOW_RATE),
        shared: None,
    }
}

#[test]
fn a_full_queue_drops_signals_and_counts_them_without_waiting() {
    let (sender, _receiver) = mpsc::sync_channel(1);
    let sentinel = sentinel(sender);

    for _ in 0..3 {
        sentinel.send(Rule::Scan, caller(ATTACKER), ip(ATTACKER), None);
    }

    assert_eq!(sentinel.dropped.load(Ordering::Relaxed), 2);
}

#[cfg(feature = "remote-contracts")]
#[test]
fn an_answer_to_a_verified_call_from_another_app_is_no_signal() {
    let (sender, receiver) = mpsc::sync_channel(4);
    let guard = Guard(Some(Arc::new(sentinel(sender))));
    let (request, ()) = http::Request::post("/_sofie/contracts/orders/missing")
        .body(())
        .unwrap()
        .into_parts();
    let deadline = crate::deadline::Deadline::default();
    let asked = Asked {
        request: &request,
        caller: crate::caller::Caller::Json,
        client: ip(ATTACKER),
        deadline: &deadline,
    };
    let missing = || crate::answer::failure(Error::NotFound, crate::caller::Caller::Json);

    guard.observe(asked, None, &unobserved(missing()));
    let verified = receiver.try_recv();
    guard.observe(asked, None, &missing());

    assert!(verified.is_err());
    assert!(receiver.try_recv().is_ok());
}

#[test]
fn statuses_are_signals_only_where_they_tell_of_an_attack() {
    assert_eq!(rule_of(StatusCode::NOT_FOUND, true), Some(Rule::Probe));
    assert_eq!(rule_of(StatusCode::NOT_FOUND, false), Some(Rule::Scan));
    assert_eq!(
        rule_of(StatusCode::METHOD_NOT_ALLOWED, true),
        Some(Rule::Scan)
    );
    assert_eq!(
        rule_of(StatusCode::PAYLOAD_TOO_LARGE, true),
        Some(Rule::Malformed)
    );
    for quiet in [
        StatusCode::OK,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNPROCESSABLE_ENTITY,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        assert_eq!(rule_of(quiet, true), None, "{quiet}");
    }
}

#[test]
fn a_development_build_only_reports_and_a_mode_is_read_by_name() {
    assert_eq!(Shield::default().mode, ShieldMode::DryRun);
    assert_eq!(ShieldMode::parse(" on ").unwrap(), ShieldMode::Enforce);
    assert_eq!(ShieldMode::parse("off").unwrap(), ShieldMode::Off);
    assert!(ShieldMode::parse("loud").is_err());
}

#[test]
fn sign_ins_and_sign_ups_are_held_to_the_address_whichever_session_cookie_they_carry() {
    let _serial = serial();
    let (sender, receiver) = mpsc::sync_channel(QUEUE);
    let mut detector = detector(false);
    let sentinel = Arc::new(Sentinel {
        blocks: Arc::clone(&detector.blocks),
        sender,
        dropped: Arc::default(),
        clock: test_clock().clock(),
        slowed: Limiter::new(SLOW_RATE),
        shared: None,
    });
    let signed_in = || {
        let principal = Uuid::now_v7().unwrap();
        let request = RequestShield {
            sentinel: Arc::clone(&sentinel),
            address: ip(ATTACKER),
            area: "web",
        };
        (
            principal,
            SignIns {
                request: Some(request),
            },
        )
    };
    for _ in 0..40 {
        signed_in().1.failed(None);
        signed_in().1.signed_up();
    }
    while let Ok(message) = receiver.try_recv() {
        detector.handle(message);
    }
    let (principal, _) = signed_in();
    let (parts, ()) = http::Request::post("/sign-in")
        .body(())
        .unwrap()
        .into_parts();
    let deadline = crate::deadline::Deadline::default();
    let asked = Asked {
        request: &parts,
        caller: crate::caller::Caller::Json,
        client: ip(ATTACKER),
        deadline: &deadline,
    };

    let refusal = Guard(Some(Arc::clone(&sentinel))).refusal(asked, Some(principal));

    assert_eq!(
        refusal.map(|response| response.status()),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
}
