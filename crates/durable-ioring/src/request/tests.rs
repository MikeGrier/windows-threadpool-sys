// Copyright (c) 2026 Mike Grier
//! A request in the driver's buffer, and the request lent from it (DI-D-115): what a fill puts in,
//! what the provider is lent, which completions it takes, and what is abandoned when a view or the
//! iterator of requests is dropped; and that a request that would not fit leaves the buffer as it
//! was.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{Overflow, RequestBuf};
use crate::capacity::Capacity;
use crate::domains::DomainId;
use crate::driver::{Answer, DomainCall};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::provider::DomainOutcome;
use crate::types::{FlushDomain, IoTargetKey};

type V = DioringIds<u64, u64>;

/// Where completions answer: a stand-in for the instance.
#[derive(Default)]
struct Home {
    issued: AtomicUsize,
    answers: Mutex<Vec<(u64, FlushDomain, &'static str)>>,
}

impl Answer for Home {
    fn issued(&self) {
        self.issued.fetch_add(1, Ordering::Relaxed);
    }

    fn answer(&self, call: &DomainCall, outcome: DomainOutcome) {
        let how = match outcome {
            DomainOutcome::Succeeded => "succeeded",
            DomainOutcome::Failed(_) => "failed",
            DomainOutcome::Abandoned => "abandoned",
        };
        self.answers
            .lock()
            .expect("not poisoned")
            .push((call.seal, call.name.clone(), how));
    }
}

impl Home {
    fn weak(self: &Arc<Self>) -> Weak<dyn Answer> {
        Arc::downgrade(self) as Weak<dyn Answer>
    }

    /// How each domain was answered, in the order answered.
    fn answered(&self) -> Vec<(u64, String, &'static str)> {
        self.answers
            .lock()
            .expect("not poisoned")
            .iter()
            .map(|(seal, domain, how)| (*seal, name(domain), *how))
            .collect()
    }

    fn issued(&self) -> usize {
        self.issued.load(Ordering::Relaxed)
    }
}

fn domain(name: &str) -> FlushDomain {
    FlushDomain::new(name.as_bytes())
}

fn name(domain: &FlushDomain) -> String {
    String::from_utf8(domain.bytes().to_vec()).expect("a test name")
}

fn capacity(operations: usize) -> Capacity {
    Capacity {
        operations,
        lineages: 1,
        imports: 0,
        targets: 8,
        domains: 8,
    }
}

/// A write as the core names it: its identity, its file, and the domains the file declares.
type Write = (OpId, IoTargetKey, Vec<DomainId>);

fn write(instance: InstanceId, seq: u64, file: u64, domains: &[u32]) -> Write {
    (
        OpId { instance, seq },
        IoTargetKey(file),
        domains.iter().map(|&d| DomainId(d)).collect(),
    )
}

/// Fill `buf` with the request for `seal`: `asked` domains, ascending, of `writes`.
fn fill(
    buf: &mut RequestBuf<V>,
    instance: InstanceId,
    seal: u64,
    asked: &[u32],
    writes: &[Write],
) -> Result<(), Overflow> {
    let lineage = Lineage { instance, seq: 0 };
    let asked = asked
        .iter()
        .map(|&d| (DomainId(d), domain(&format!("d{d}"))));
    buf.fill(lineage, seal, asked, || {
        writes
            .iter()
            .map(|(op, file, domains)| (*op, *file, domains.as_slice()))
    })
}

fn buf(operations: usize, served: usize) -> RequestBuf<V> {
    RequestBuf::new(&capacity(operations), served).expect("a buffer")
}

/// Each domain of each request, with each file's writes' sequence numbers.
type Shape = Vec<(String, Vec<(u64, Vec<u64>)>)>;

fn shape(buf: &RequestBuf<V>) -> Vec<Shape> {
    buf.owned()
        .into_iter()
        .map(|request| {
            request
                .domains
                .into_iter()
                .map(|part| {
                    let files = part
                        .targets
                        .into_iter()
                        .map(|t| (t.key.0, t.writes.iter().map(|op| op.seq).collect()))
                        .collect();
                    (name(&part.domain), files)
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_fill_names_each_domain_with_its_files_in_the_order_first_written_and_their_writes_together() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 3);
    let writes = [
        write(instance, 0, 10, &[2, 1]),
        write(instance, 1, 20, &[1]),
        write(instance, 2, 10, &[2, 1]),
        write(instance, 3, 30, &[3, 9]),
    ];
    fill(&mut buf, instance, 5, &[1, 2, 3], &writes).expect("fits");
    assert_eq!(
        shape(&buf),
        [vec![
            ("d1".to_owned(), vec![(10, vec![0, 2]), (20, vec![1])]),
            ("d2".to_owned(), vec![(10, vec![0, 2])]),
            ("d3".to_owned(), vec![(30, vec![3])]),
        ]]
    );
    assert_eq!((buf.len(), buf.is_empty()), (1, false));
}

#[test]
fn a_write_not_declared_with_a_domain_is_not_named_for_it() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 2);
    let writes = [write(instance, 0, 1, &[1]), write(instance, 1, 2, &[2])];
    fill(&mut buf, instance, 0, &[1, 2], &writes).expect("fits");
    assert_eq!(
        shape(&buf),
        [vec![
            ("d1".to_owned(), vec![(1, vec![0])]),
            ("d2".to_owned(), vec![(2, vec![1])]),
        ]]
    );
}

#[test]
fn a_domain_asked_about_with_no_write_left_is_a_part_with_no_target() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 1);
    fill(&mut buf, instance, 0, &[1], &[]).expect("fits");
    assert_eq!(shape(&buf), [vec![("d1".to_owned(), vec![])]]);
}

#[test]
fn requests_are_kept_in_the_order_they_were_filled() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 1);
    for seal in [4, 2, 9] {
        let writes = [write(instance, seal, 1, &[1])];
        fill(&mut buf, instance, seal, &[1], &writes).expect("fits");
    }
    let seals: Vec<u64> = buf.owned().iter().map(|request| request.seal).collect();
    assert_eq!(seals, [4, 2, 9]);
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let lent: Vec<u64> = buf
        .requests(&weak)
        .map(|request| request.request().seal)
        .collect();
    assert_eq!(lent, [4, 2, 9]);
}

#[test]
fn a_request_that_would_not_fit_is_refused_and_leaves_the_buffer_as_it_was() {
    let instance = InstanceId::next();
    // Room for two writes in all, over one domain.
    let mut buf = buf(2, 1);
    let first = [write(instance, 0, 1, &[1])];
    fill(&mut buf, instance, 0, &[1], &first).expect("fits");
    let before = shape(&buf);
    let too_many = [
        write(instance, 1, 1, &[1]),
        write(instance, 2, 1, &[1]),
        write(instance, 3, 1, &[1]),
    ];
    assert!(fill(&mut buf, instance, 1, &[1], &too_many).is_err());
    assert_eq!(
        shape(&buf),
        before,
        "nothing of the refused request is left"
    );
    let one = [write(instance, 4, 1, &[1])];
    fill(&mut buf, instance, 2, &[1], &one).expect("what was left still fits");
    assert_eq!(buf.len(), 2);
}

#[test]
fn each_of_the_buffers_lists_refuses_past_its_room() {
    let instance = InstanceId::next();
    // One operation and one served domain: room for one part.
    let mut parts = buf(1, 1);
    fill(
        &mut parts,
        instance,
        0,
        &[1],
        &[write(instance, 0, 1, &[1])],
    )
    .expect("fits");
    assert!(fill(&mut parts, instance, 1, &[1], &[]).is_err(), "a part");
    // Two operations and one served domain: room for two targets, and for two writes.
    let mut targets = buf(2, 1);
    let three_files = [
        write(instance, 0, 1, &[1]),
        write(instance, 1, 2, &[1]),
        write(instance, 2, 3, &[1]),
    ];
    assert!(
        fill(&mut targets, instance, 0, &[1], &three_files).is_err(),
        "a target"
    );
    assert!(targets.is_empty());
    // Room for one request, and for no part.
    let mut requests = buf(1, 0);
    fill(&mut requests, instance, 0, &[], &[]).expect("a request with nothing asked");
    assert!(
        fill(&mut requests, instance, 1, &[], &[]).is_err(),
        "a request"
    );
}

#[test]
fn clearing_empties_the_buffer_and_keeps_its_room() {
    let instance = InstanceId::next();
    let mut buf = buf(2, 1);
    let writes = [write(instance, 0, 1, &[1]), write(instance, 1, 1, &[1])];
    for _ in 0..3 {
        fill(&mut buf, instance, 0, &[1], &writes).expect("fits every time");
        assert_eq!(buf.len(), 1);
        buf.clear();
        assert!(buf.is_empty());
        assert!(buf.owned().is_empty());
    }
}

#[test]
fn a_lent_request_gives_each_domain_its_files_and_writes_as_slices_of_the_buffer() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 2);
    let writes = [
        write(instance, 0, 10, &[1, 2]),
        write(instance, 1, 20, &[1]),
        write(instance, 2, 10, &[1]),
    ];
    fill(&mut buf, instance, 7, &[1, 2], &writes).expect("fits");
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let mut requests = buf.requests(&weak);
    assert_eq!(requests.len(), 1);
    let request = requests.next().expect("one request");
    assert_eq!(request.domain_count(), 2);
    let named: Vec<String> = request.domain_names().map(name).collect();
    assert_eq!(named, ["d1", "d2"]);
    let mut seen = Vec::new();
    for part in request.domains() {
        let files: Vec<(u64, Vec<u64>)> = part
            .files
            .clone()
            .map(|file| (file.key.0, file.writes.iter().map(|op| op.seq).collect()))
            .collect();
        assert_eq!(part.files.len(), files.len());
        seen.push((name(part.domain), files));
        part.completion.succeed();
    }
    assert_eq!(
        seen,
        [
            ("d1".to_owned(), vec![(10, vec![0, 2]), (20, vec![1])]),
            ("d2".to_owned(), vec![(10, vec![0])]),
        ]
    );
    assert_eq!(
        home.answered(),
        [
            (7, "d1".to_owned(), "succeeded"),
            (7, "d2".to_owned(), "succeeded")
        ]
    );
    assert_eq!(home.issued(), 2);
}

#[test]
fn taking_a_domain_is_what_removes_it_from_the_view() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 2);
    let writes = [write(instance, 0, 1, &[1, 2])];
    fill(&mut buf, instance, 0, &[1, 2], &writes).expect("fits");
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let request = buf.requests(&weak).next().expect("one request");
    // Looking creates nothing.
    assert_eq!(request.domain_count(), 2);
    assert_eq!(home.issued(), 0);
    let first = request.domains().next().expect("d1");
    assert_eq!(request.domain_count(), 1, "d1 is taken");
    assert_eq!(request.domains().count(), 1, "only d2 is left to take");
    assert_eq!(request.domain_count(), 0);
    first.completion.succeed();
    assert_eq!(home.issued(), 2);
}

#[test]
fn a_domain_not_taken_is_abandoned_when_the_request_is_dropped() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 2);
    let writes = [write(instance, 0, 1, &[1, 2])];
    fill(&mut buf, instance, 3, &[1, 2], &writes).expect("fits");
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let request = buf.requests(&weak).next().expect("one request");
    let first = request.domains().next().expect("d1");
    first.completion.succeed();
    drop(request);
    assert_eq!(
        home.answered(),
        [
            (3, "d1".to_owned(), "succeeded"),
            (3, "d2".to_owned(), "abandoned")
        ]
    );
    assert_eq!(home.issued(), 2, "every completion made was answered");
}

#[test]
fn a_domain_taken_and_dropped_is_abandoned_once() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 1);
    let writes = [write(instance, 0, 1, &[1])];
    fill(&mut buf, instance, 0, &[1], &writes).expect("fits");
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let request = buf.requests(&weak).next().expect("one request");
    drop(request.domains().next());
    drop(request);
    assert_eq!(home.answered(), [(0, "d1".to_owned(), "abandoned")]);
}

#[test]
fn a_narrowed_view_takes_only_its_domains_and_abandons_only_its_own() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 3);
    let writes = [write(instance, 0, 1, &[1, 2, 3])];
    fill(&mut buf, instance, 0, &[1, 2, 3], &writes).expect("fits");
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let request = buf.requests(&weak).next().expect("one request");
    let wants_one_and_two = |d: &FlushDomain| matches!(name(d).as_str(), "d1" | "d2");
    {
        let view = request.only(&wants_one_and_two);
        assert_eq!(view.domain_count(), 2);
        let names: Vec<String> = view.domain_names().map(name).collect();
        assert_eq!(names, ["d1", "d2"]);
        assert_eq!(request.domain_count(), 3, "looking takes nothing");
        view.domains().next().expect("d1").completion.succeed();
        assert_eq!(request.domain_count(), 2, "what the view took is gone");
    }
    assert_eq!(
        home.answered(),
        [
            (0, "d1".to_owned(), "succeeded"),
            (0, "d2".to_owned(), "abandoned")
        ],
        "the view abandoned d2, which it covered and did not take, and not d3"
    );
    assert_eq!(request.domain_count(), 1);
    assert_eq!(request.domain_names().map(name).collect::<Vec<_>>(), ["d3"]);
}

#[test]
fn a_view_narrowed_again_keeps_the_narrowing_it_came_from() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 3);
    let writes = [write(instance, 0, 1, &[1, 2, 3])];
    fill(&mut buf, instance, 0, &[1, 2, 3], &writes).expect("fits");
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let request = buf.requests(&weak).next().expect("one request");
    let not_three = |d: &FlushDomain| name(d) != "d3";
    let not_one = |d: &FlushDomain| name(d) != "d1";
    let outer = request.only(&not_three);
    let inner = outer.only(&not_one);
    assert_eq!(
        inner.domain_names().map(name).collect::<Vec<_>>(),
        ["d2"],
        "both filters hold"
    );
    for part in inner.domains() {
        part.completion.succeed();
    }
    assert_eq!(outer.domain_count(), 1, "d1 is the outer view's still");
    assert_eq!(request.domain_count(), 2, "d1 and d3");
    drop(inner);
    drop(outer);
    assert_eq!(
        home.answered(),
        [
            (0, "d2".to_owned(), "succeeded"),
            (0, "d1".to_owned(), "abandoned")
        ]
    );
    drop(request);
    assert_eq!(home.answered().len(), 3);
    assert_eq!(home.answered()[2].1, "d3");
}

#[test]
fn requests_not_taken_from_the_iterator_are_abandoned_when_it_is_dropped() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 1);
    for seal in [1, 2, 3] {
        let writes = [write(instance, seal, 1, &[1])];
        fill(&mut buf, instance, seal, &[1], &writes).expect("fits");
    }
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let mut requests = buf.requests(&weak);
    let first = requests.next().expect("the first");
    first
        .domains()
        .next()
        .expect("its domain")
        .completion
        .succeed();
    drop(first);
    drop(requests);
    assert_eq!(
        home.answered(),
        [
            (1, "d1".to_owned(), "succeeded"),
            (2, "d1".to_owned(), "abandoned"),
            (3, "d1".to_owned(), "abandoned"),
        ]
    );
}

#[test]
fn a_panic_while_a_request_is_lent_abandons_what_it_left_and_the_requests_after_it() {
    let instance = InstanceId::next();
    let mut buf = buf(8, 2);
    for seal in [1, 2] {
        let writes = [write(instance, seal, 1, &[1, 2])];
        fill(&mut buf, instance, seal, &[1, 2], &writes).expect("fits");
    }
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut requests = buf.requests(&weak);
        let request = requests.next().expect("the first");
        // The first domain is taken and answered, then the provider panics.
        request
            .domains()
            .next()
            .expect("a domain")
            .completion
            .succeed();
        panic!("the provider panicked");
    }));
    assert!(outcome.is_err());
    assert_eq!(
        home.answered(),
        [
            (1, "d1".to_owned(), "succeeded"),
            (1, "d2".to_owned(), "abandoned"),
            (2, "d1".to_owned(), "abandoned"),
            (2, "d2".to_owned(), "abandoned"),
        ]
    );
    assert_eq!(home.issued(), 4, "every completion made was answered");
}

#[test]
fn a_completion_taken_outlives_the_buffer_it_was_taken_from() {
    let instance = InstanceId::next();
    let home = Arc::new(Home::default());
    let weak = home.weak();
    let completion = {
        let mut buf = buf(8, 1);
        let writes = [write(instance, 0, 1, &[1])];
        fill(&mut buf, instance, 6, &[1], &writes).expect("fits");
        let request = buf.requests(&weak).next().expect("one request");
        request.domains().next().expect("d1").completion
    };
    assert!(home.answered().is_empty(), "kept, not answered");
    completion.fail(crate::report_io_error(std::io::Error::other("late")));
    assert_eq!(home.answered(), [(6, "d1".to_owned(), "failed")]);
}
