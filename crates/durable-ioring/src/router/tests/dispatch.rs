// Copyright (c) 2026 Mike Grier
//! The router's dispatch (DI-12.2.2): one call per provider with a part, in registration order,
//! each completion handed on untouched, a panic propagating, and an unrouted domain panicked on.

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;

use super::{V, domain, domains};
use crate::driver::{Answer, DomainCall};
use crate::ids::{InstanceId, Lineage, OpId};
use crate::provider::{
    DomainCompletion, DomainOutcome, DomainRequest, DurabilityProvider, FlushRequest,
    IoTargetWrites,
};
use crate::router::RoutingProvider;
use crate::types::{FlushDomain, IoTargetKey};

/// How a domain was answered, as the instance would record it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seen {
    Succeeded,
    Failed(io::ErrorKind, String),
    Abandoned,
}

/// Where completions answer: a stand-in for the instance.
#[derive(Default)]
struct Home {
    issued: AtomicUsize,
    answers: Mutex<Vec<(FlushDomain, Seen)>>,
}

impl Answer for Home {
    fn issued(&self) {
        self.issued.fetch_add(1, Ordering::Relaxed);
    }

    fn answer(&self, call: &DomainCall, outcome: DomainOutcome) {
        let seen = match outcome {
            DomainOutcome::Succeeded => Seen::Succeeded,
            DomainOutcome::Failed(error) => Seen::Failed(
                error
                    .downcast_ref::<io::Error>()
                    .expect("the tests fail with an io::Error")
                    .kind(),
                error.to_string(),
            ),
            DomainOutcome::Abandoned => Seen::Abandoned,
        };
        self.answers
            .lock()
            .expect("not poisoned")
            .push((call.name.clone(), seen));
    }
}

impl Home {
    fn answers(&self) -> Vec<(FlushDomain, Seen)> {
        self.answers.lock().expect("not poisoned").clone()
    }

    fn answer_for(&self, name: &str) -> Option<Seen> {
        let wanted = domain(name);
        let found: Vec<_> = self
            .answers()
            .into_iter()
            .filter(|(domain, _)| *domain == wanted)
            .map(|(_, seen)| seen)
            .collect();
        assert!(found.len() <= 1, "{name} answered {} times", found.len());
        found.into_iter().next()
    }

    /// Every completion made has been answered, once.
    fn all_answered(&self) -> bool {
        self.answers().len() == self.issued.load(Ordering::Relaxed)
    }
}

/// A request with one part per name, in order: each part's one file is keyed by its place in the
/// request, with one write numbered the same.
fn request(home: &Arc<Home>, names: &[&str]) -> FlushRequest<V> {
    let instance = InstanceId::next();
    let lineage = Lineage { instance, seq: 0 };
    let weak: Weak<dyn Answer> = Arc::downgrade(home) as Weak<dyn Answer>;
    FlushRequest {
        domains: names
            .iter()
            .enumerate()
            .map(|(place, name)| {
                let seq = u64::try_from(place).expect("small");
                DomainRequest {
                    domain: domain(name),
                    files: vec![IoTargetWrites {
                        key: IoTargetKey(seq),
                        writes: vec![OpId { instance, seq }],
                    }],
                    completion: DomainCompletion::new(
                        weak.clone(),
                        DomainCall {
                            lineage,
                            seal: 7,
                            name: domain(name),
                        },
                    ),
                }
            })
            .collect(),
    }
}

/// What a provider does with the parts it is given.
#[derive(Debug)]
enum Act {
    Succeed,
    Fail,
    Drop,
    Keep,
    Hand(mpsc::Sender<DomainCompletion>),
    Panic,
}

/// One call a provider received: its name, and each part's domain and file keys, in order.
type Call = (&'static str, Vec<(FlushDomain, Vec<u64>)>);

#[derive(Debug)]
struct Fake {
    name: &'static str,
    domains: Vec<FlushDomain>,
    act: Act,
    calls: Arc<Mutex<Vec<Call>>>,
    kept: Vec<DomainCompletion>,
}

impl DurabilityProvider<V> for Fake {
    fn domains(&self) -> Vec<FlushDomain> {
        self.domains.clone()
    }

    fn make_durable(&mut self, request: FlushRequest<V>) {
        let call = request
            .domains
            .iter()
            .map(|part| {
                let keys = part.files.iter().map(|file| file.key.0).collect();
                (part.domain.clone(), keys)
            })
            .collect();
        self.calls
            .lock()
            .expect("not poisoned")
            .push((self.name, call));
        for part in request.domains {
            match &self.act {
                Act::Succeed => part.completion.succeed(),
                Act::Fail => part.completion.fail(crate::report_io_error(io::Error::new(
                    io::ErrorKind::TimedOut,
                    self.name,
                ))),
                Act::Drop => drop(part),
                Act::Keep => self.kept.push(part.completion),
                Act::Hand(sender) => sender.send(part.completion).expect("the test listens"),
                Act::Panic => panic!("{} panicked", self.name),
            }
        }
    }
}

/// A rig: the calls every provider received, and a router built over the providers given.
struct Rig {
    calls: Arc<Mutex<Vec<Call>>>,
    router: RoutingProvider<V>,
}

impl Rig {
    fn new(providers: Vec<(&'static str, &[&str], Act)>) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let router = providers
            .into_iter()
            .fold(RoutingProvider::builder(), |builder, (name, names, act)| {
                builder.register(Fake {
                    name,
                    domains: domains(names),
                    act,
                    calls: Arc::clone(&calls),
                    kept: Vec::new(),
                })
            })
            .build()
            .expect("no domain twice");
        Self { calls, router }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("not poisoned").clone()
    }

    fn called(&self) -> Vec<&'static str> {
        self.calls().into_iter().map(|(name, _)| name).collect()
    }
}

fn part(name: &str, key: u64) -> (FlushDomain, Vec<u64>) {
    (domain(name), vec![key])
}

#[test]
fn one_provider_is_given_the_whole_request_as_it_came() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![("only", &["log", "data"], Act::Succeed)]);
    rig.router.make_durable(request(&home, &["data", "log"]));
    assert_eq!(
        rig.calls(),
        vec![("only", vec![part("data", 0), part("log", 1)])]
    );
    assert_eq!(home.answer_for("data"), Some(Seen::Succeeded));
    assert_eq!(home.answer_for("log"), Some(Seen::Succeeded));
    assert!(home.all_answered());
}

#[test]
fn each_provider_is_given_only_its_own_domains() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("logs", &["log"], Act::Succeed),
        ("disks", &["disk-a", "disk-b"], Act::Succeed),
    ]);
    rig.router
        .make_durable(request(&home, &["disk-b", "log", "disk-a"]));
    assert_eq!(
        rig.calls(),
        vec![
            ("logs", vec![part("log", 1)]),
            ("disks", vec![part("disk-b", 0), part("disk-a", 2)]),
        ]
    );
    assert!(home.all_answered());
}

#[test]
fn providers_are_called_in_registration_order_whatever_the_request_order() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("first", &["a"], Act::Succeed),
        ("second", &["b"], Act::Succeed),
        ("third", &["c"], Act::Succeed),
    ]);
    rig.router.make_durable(request(&home, &["c", "b", "a"]));
    assert_eq!(rig.called(), vec!["first", "second", "third"]);
}

#[test]
fn a_provider_with_no_part_is_not_called() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("idle", &["a"], Act::Panic),
        ("busy", &["b"], Act::Succeed),
        ("empty", &[], Act::Panic),
    ]);
    rig.router.make_durable(request(&home, &["b"]));
    assert_eq!(rig.called(), vec!["busy"]);
    assert!(home.all_answered());
}

#[test]
fn an_empty_request_calls_no_provider() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![("a", &["a"], Act::Panic), ("b", &["b"], Act::Panic)]);
    rig.router.make_durable(request(&home, &[]));
    assert!(rig.calls().is_empty());
    assert!(home.answers().is_empty());
}

#[test]
fn several_domains_of_one_provider_are_one_call() {
    let home = Arc::new(Home::default());
    let names = ["d0", "d1", "d2", "d3", "d4", "d5"];
    let mut rig = Rig::new(vec![("many", &names, Act::Succeed)]);
    rig.router.make_durable(request(&home, &names));
    let calls = rig.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.len(), names.len());
    assert!(home.all_answered());
}

#[test]
fn an_answer_inside_the_call_reaches_the_instance_during_it() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("ok", &["a"], Act::Succeed),
        ("bad", &["b"], Act::Fail),
    ]);
    rig.router.make_durable(request(&home, &["a", "b"]));
    assert_eq!(home.answer_for("a"), Some(Seen::Succeeded));
    assert_eq!(
        home.answer_for("b"),
        Some(Seen::Failed(io::ErrorKind::TimedOut, "bad".to_owned()))
    );
}

#[test]
fn a_late_answer_from_another_thread_reaches_the_instance_as_it_arrives() {
    let home = Arc::new(Home::default());
    let (sender, handed) = mpsc::channel();
    let mut rig = Rig::new(vec![("later", &["a", "b"], Act::Hand(sender))]);
    rig.router.make_durable(request(&home, &["a", "b"]));
    assert!(home.answers().is_empty(), "nothing answered yet");
    let first = handed.recv().expect("a");
    let second = handed.recv().expect("b");
    thread::spawn(move || second.succeed())
        .join()
        .expect("answered");
    assert_eq!(home.answers(), vec![(domain("b"), Seen::Succeeded)]);
    thread::spawn(move || first.fail(crate::report_io_error(io::Error::other("late"))))
        .join()
        .expect("answered");
    assert_eq!(
        home.answer_for("a"),
        Some(Seen::Failed(io::ErrorKind::Other, "late".to_owned()))
    );
    assert!(home.all_answered());
}

#[test]
fn completions_a_provider_drops_are_abandoned_and_only_its_own() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("drops", &["a"], Act::Drop),
        ("answers", &["b"], Act::Succeed),
    ]);
    rig.router.make_durable(request(&home, &["a", "b"]));
    assert_eq!(home.answer_for("a"), Some(Seen::Abandoned));
    assert_eq!(home.answer_for("b"), Some(Seen::Succeeded));
}

#[test]
fn completions_a_provider_keeps_stay_unanswered_until_it_is_dropped() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![("keeps", &["a"], Act::Keep)]);
    rig.router.make_durable(request(&home, &["a"]));
    assert_eq!(home.answer_for("a"), None);
    assert!(!home.all_answered());
    drop(rig);
    assert_eq!(home.answer_for("a"), Some(Seen::Abandoned));
}

#[test]
fn a_panic_propagates_and_abandons_the_parts_not_yet_handed_on() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("before", &["a"], Act::Succeed),
        ("panics", &["b", "c"], Act::Panic),
        ("after", &["d"], Act::Succeed),
    ]);
    let request = request(&home, &["d", "c", "b", "a"]);
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| rig.router.make_durable(request)));
    let payload = outcome.expect_err("the panic propagates");
    let message = payload.downcast_ref::<String>().expect("a formatted panic");
    assert_eq!(message, "panics panicked");
    assert_eq!(rig.called(), vec!["before", "panics"]);
    assert_eq!(home.answer_for("a"), Some(Seen::Succeeded));
    assert_eq!(home.answer_for("b"), Some(Seen::Abandoned));
    assert_eq!(home.answer_for("c"), Some(Seen::Abandoned));
    assert_eq!(home.answer_for("d"), Some(Seen::Abandoned));
    assert!(home.all_answered());
}

/// The message of the panic `request` raises in `router`, which must panic.
fn panic_of(router: &mut RoutingProvider<V>, request: FlushRequest<V>) -> String {
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| router.make_durable(request)));
    let payload = outcome.expect_err("an unrouted domain panics");
    payload
        .downcast_ref::<String>()
        .expect("a formatted panic")
        .clone()
}

#[test]
fn a_domain_no_provider_serves_is_panicked_on_naming_it() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![("known", &["a"], Act::Succeed)]);
    let message = panic_of(&mut rig.router, request(&home, &["a", "stray"]));
    assert!(
        message.contains(&format!("{:?}", domain("stray"))),
        "{message}"
    );
    assert!(message.contains("no provider registered"), "{message}");
}

#[test]
fn an_unrouted_domain_is_panicked_on_before_any_provider_is_called() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("first", &["a"], Act::Succeed),
        ("second", &["b"], Act::Succeed),
    ]);
    let _ = panic_of(&mut rig.router, request(&home, &["a", "b", "stray"]));
    assert!(rig.calls().is_empty(), "no provider called");
}

#[test]
fn an_unrouted_domain_abandons_every_domain_of_the_request() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![("known", &["a", "b"], Act::Succeed)]);
    let _ = panic_of(&mut rig.router, request(&home, &["a", "stray", "b"]));
    assert_eq!(home.answer_for("a"), Some(Seen::Abandoned));
    assert_eq!(home.answer_for("stray"), Some(Seen::Abandoned));
    assert_eq!(home.answer_for("b"), Some(Seen::Abandoned));
    assert!(home.all_answered());
}

#[test]
fn a_router_with_no_provider_panics_on_any_domain() {
    let home = Arc::new(Home::default());
    let mut router = RoutingProvider::<V>::builder().build().expect("empty");
    let message = panic_of(&mut router, request(&home, &["a"]));
    assert!(message.contains(&format!("{:?}", domain("a"))), "{message}");
    assert_eq!(home.answer_for("a"), Some(Seen::Abandoned));
}

#[test]
fn the_router_is_usable_after_an_unrouted_request_panicked() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![("known", &["a"], Act::Succeed)]);
    let _ = panic_of(&mut rig.router, request(&home, &["stray"]));
    rig.router.make_durable(request(&home, &["a"]));
    assert_eq!(rig.called(), vec!["known"]);
    assert_eq!(home.answer_for("a"), Some(Seen::Succeeded));
}

#[test]
fn each_seal_is_dispatched_on_its_own() {
    let home = Arc::new(Home::default());
    let mut rig = Rig::new(vec![
        ("x", &["a"], Act::Succeed),
        ("y", &["b"], Act::Succeed),
    ]);
    rig.router.make_durable(request(&home, &["a"]));
    rig.router.make_durable(request(&home, &["b"]));
    rig.router.make_durable(request(&home, &["a", "b"]));
    assert_eq!(rig.called(), vec!["x", "y", "x", "y"]);
    assert_eq!(home.answers().len(), 4);
    assert!(home.all_answered());
}

#[test]
fn a_router_can_be_registered_with_a_router() {
    let home = Arc::new(Home::default());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fake = |name, names: &[&str]| Fake {
        name,
        domains: domains(names),
        act: Act::Succeed,
        calls: Arc::clone(&calls),
        kept: Vec::new(),
    };
    let inner = RoutingProvider::builder()
        .register(fake("inner-a", &["a"]))
        .register(fake("inner-b", &["b"]))
        .build()
        .expect("no domain twice");
    let mut outer = RoutingProvider::builder()
        .register(inner)
        .register(fake("outer-c", &["c"]))
        .build()
        .expect("no domain twice");
    assert_eq!(outer.domains(), domains(&["a", "b", "c"]));
    outer.make_durable(request(&home, &["c", "b", "a"]));
    let called: Vec<_> = calls
        .lock()
        .expect("not poisoned")
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(called, vec!["inner-a", "inner-b", "outer-c"]);
    assert!(home.all_answered());
}
