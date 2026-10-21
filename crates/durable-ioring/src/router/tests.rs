// Copyright (c) 2026 Mike Grier
//! The router's build (DI-12.2.1, DI-D-81): the union of its providers' domains, and a domain
//! claimed twice refused.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{RoutedTwice, RouterBuilder, RoutingProvider};
use crate::ids::DioringIds;
use crate::provider::{DurabilityProvider, FlushRequest};
use crate::types::FlushDomain;

type V = DioringIds<u64, u64>;

fn domain(name: &str) -> FlushDomain {
    FlushDomain::new(name.as_bytes())
}

fn domains(names: &[&str]) -> Vec<FlushDomain> {
    names.iter().map(|name| domain(name)).collect()
}

/// A provider that declares `domains` and counts how often it is asked for them.
#[derive(Debug)]
struct Serves {
    domains: Vec<FlushDomain>,
    asked: Arc<AtomicUsize>,
}

impl DurabilityProvider<V> for Serves {
    fn domains(&self) -> Vec<FlushDomain> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        self.domains.clone()
    }

    fn make_durable(&mut self, _: FlushRequest<V>) {
        panic!("a router's build makes no request");
    }
}

fn serves(names: &[&str]) -> Serves {
    counted(names, &Arc::new(AtomicUsize::new(0)))
}

fn counted(names: &[&str], asked: &Arc<AtomicUsize>) -> Serves {
    Serves {
        domains: domains(names),
        asked: Arc::clone(asked),
    }
}

fn built(providers: Vec<Serves>) -> Result<RoutingProvider<V>, RoutedTwice> {
    providers
        .into_iter()
        .fold(RoutingProvider::builder(), RouterBuilder::register)
        .build()
}

#[test]
fn one_provider_serves_its_own_domains() {
    let router = built(vec![serves(&["log", "data"])]).expect("no domain twice");
    assert_eq!(router.domains(), domains(&["log", "data"]));
}

#[test]
fn two_providers_serve_the_union_in_registration_order() {
    let router =
        built(vec![serves(&["log"]), serves(&["disk-a", "disk-b"])]).expect("no domain twice");
    assert_eq!(router.domains(), domains(&["log", "disk-a", "disk-b"]));
}

#[test]
fn the_union_keeps_each_providers_own_order() {
    let router =
        built(vec![serves(&["z", "a", "m"]), serves(&["c", "b"])]).expect("no domain twice");
    assert_eq!(router.domains(), domains(&["z", "a", "m", "c", "b"]));
}

#[test]
fn many_providers_serve_every_domain_once() {
    let names: Vec<String> = (0..64).map(|n| format!("d{n}")).collect();
    let providers = names
        .chunks(4)
        .map(|chunk| serves(&chunk.iter().map(String::as_str).collect::<Vec<_>>()))
        .collect();
    let router = built(providers).expect("no domain twice");
    let expected: Vec<_> = names.iter().map(|name| domain(name)).collect();
    assert_eq!(router.domains(), expected);
}

#[test]
fn a_provider_that_serves_no_domain_adds_none() {
    let router =
        built(vec![serves(&["log"]), serves(&[]), serves(&["data"])]).expect("no domain twice");
    assert_eq!(router.domains(), domains(&["log", "data"]));
}

#[test]
fn a_router_with_no_provider_serves_nothing() {
    let router = RoutingProvider::<V>::builder()
        .build()
        .expect("nothing to repeat");
    assert!(router.domains().is_empty());
    let defaulted = RouterBuilder::<V>::default()
        .build()
        .expect("nothing to repeat");
    assert!(defaulted.domains().is_empty());
}

#[test]
fn providers_that_serve_nothing_make_a_router_that_serves_nothing() {
    let router = built(vec![serves(&[]), serves(&[])]).expect("nothing to repeat");
    assert!(router.domains().is_empty());
}

#[test]
fn a_domain_claimed_by_adjacent_registrations_is_refused_naming_both() {
    let refused = built(vec![serves(&["log"]), serves(&["data", "log"])]).expect_err("log twice");
    assert_eq!(
        refused,
        RoutedTwice {
            domain: domain("log"),
            first: 0,
            second: 1,
        }
    );
    let shown = refused.to_string();
    assert!(shown.contains("registrations 0 and 1"), "{shown}");
}

#[test]
fn a_domain_claimed_by_distant_registrations_is_refused_naming_both() {
    let refused = built(vec![
        serves(&["a"]),
        serves(&["b"]),
        serves(&[]),
        serves(&["c"]),
        serves(&["d"]),
        serves(&["e", "a"]),
    ])
    .expect_err("a twice");
    assert_eq!(
        refused,
        RoutedTwice {
            domain: domain("a"),
            first: 0,
            second: 5,
        }
    );
}

#[test]
fn a_provider_declaring_a_domain_twice_is_refused_naming_it_once() {
    let refused =
        built(vec![serves(&["x"]), serves(&["log", "data", "log"])]).expect_err("log twice");
    assert_eq!(
        refused,
        RoutedTwice {
            domain: domain("log"),
            first: 1,
            second: 1,
        }
    );
    let shown = refused.to_string();
    assert!(shown.contains("registration 1 declared"), "{shown}");
    assert!(shown.contains("twice"), "{shown}");
}

#[test]
fn the_first_repeat_met_in_reading_order_is_the_one_refused() {
    // `b` is claimed again by registration 2 and `a` by registration 3; registration 2 is read
    // first, so `b` is the one named, though `a` was registered first.
    let refused = built(vec![
        serves(&["a", "b"]),
        serves(&["c"]),
        serves(&["b"]),
        serves(&["a"]),
    ])
    .expect_err("a repeat");
    assert_eq!(
        refused,
        RoutedTwice {
            domain: domain("b"),
            first: 0,
            second: 2,
        }
    );
}

#[test]
fn domains_are_one_when_their_bytes_are_equal() {
    let shared: Arc<[u8]> = Arc::from(&b"volume"[..]);
    let apart: Arc<[u8]> = Arc::from(&b"volume"[..]);
    let first = Serves {
        domains: vec![FlushDomain::new(shared)],
        asked: Arc::new(AtomicUsize::new(0)),
    };
    let second = Serves {
        domains: vec![FlushDomain::new(apart)],
        asked: Arc::new(AtomicUsize::new(0)),
    };
    let refused = built(vec![first, second]).expect_err("the same bytes twice");
    assert_eq!(refused.domain, domain("volume"));
}

#[test]
fn domains_that_differ_in_any_byte_are_distinct() {
    let router = built(vec![
        serves(&["log"]),
        serves(&["log2"]),
        serves(&["Log"]),
        serves(&[""]),
    ])
    .expect("all distinct");
    assert_eq!(router.domains(), domains(&["log", "log2", "Log", ""]));
}

#[test]
fn each_providers_domains_are_read_once_at_build() {
    let asked = Arc::new(AtomicUsize::new(0));
    let builder = RoutingProvider::builder()
        .register(counted(&["a"], &asked))
        .register(counted(&["b"], &asked));
    assert_eq!(
        asked.load(Ordering::Relaxed),
        0,
        "registering reads nothing"
    );
    let router = builder.build().expect("no domain twice");
    assert_eq!(asked.load(Ordering::Relaxed), 2, "one read per provider");
    let _ = router.domains();
    let _ = router.domains();
    assert_eq!(
        asked.load(Ordering::Relaxed),
        2,
        "the router answers from what it read"
    );
}

#[test]
fn a_refused_build_does_not_ask_the_providers_after_the_repeat() {
    let before = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicUsize::new(0));
    let refused = RoutingProvider::builder()
        .register(counted(&["a"], &before))
        .register(counted(&["a"], &before))
        .register(counted(&["b"], &after))
        .build();
    assert!(refused.is_err());
    assert_eq!(before.load(Ordering::Relaxed), 2);
    assert_eq!(after.load(Ordering::Relaxed), 0);
}

#[test]
fn the_refusal_is_an_error() {
    let refused: Box<dyn Error> = Box::new(RoutedTwice {
        domain: domain("log"),
        first: 0,
        second: 3,
    });
    let shown = refused.to_string();
    assert!(shown.contains("registrations 0 and 3"), "{shown}");
    assert!(refused.source().is_none());
}

#[test]
fn a_router_shows_its_routes() {
    let router = built(vec![serves(&["log"])]).expect("no domain twice");
    let shown = format!("{router:?}");
    assert!(shown.starts_with("RoutingProvider"), "{shown}");
    assert!(shown.contains("Serves"), "{shown}");
    let builder = RoutingProvider::<V>::builder().register(serves(&["log"]));
    assert!(format!("{builder:?}").contains("Serves"));
}
