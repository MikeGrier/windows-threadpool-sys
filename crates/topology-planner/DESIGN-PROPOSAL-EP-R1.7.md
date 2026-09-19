# EP-R1.7 proposal: describing flows and trying execution patterns

**Runtime-campaign scope superseded by [EP-D-10](DESIGN-NOTES.md#ep-d-10); detailed reconciliation is queued as [CHECKLIST.md](CHECKLIST.md) -> `EP-R1.7.1`.**

**Working basis for iteration, not a frozen contract.** Prepared 2026-09-18 19:03:36 -07:00 for discussion.
This is a design proposal, not an implemented API or a completed checklist item.
Current decisions remain in [DESIGN-NOTES.md](DESIGN-NOTES.md). The work is owned by
[CHECKLIST.md](CHECKLIST.md) `EP-R1.7`; final public names belong to `EP-1+.5`.

The engineer accepted this as a basis, then clarified that the archetypes and runtime
boundary must emerge through trying alternatives. Start with the smaller workloads in
[DESIGN-RESEARCH-WORKLOAD-PATTERNS.md](DESIGN-RESEARCH-WORKLOAD-PATTERNS.md), not the
complex multi-endpoint example. Define the constraints needed for each bounded
experiment without requiring the entire framework contract to be final first.

## The recommendation

Start with the developer's account of where data comes from, what happens to it,
and where it goes. Make the required semantics explicit without asking the developer
to choose a threading or queue architecture. Match that description to a small
catalog of execution patterns and generate concrete candidates for the current
allocation. Select from discovered facts and application constraints, using only
optional probes permitted by [EP-D-10](DESIGN-NOTES.md#ep-d-10). Do not run representative
workloads or size-mix sweeps as a prerequisite to selection or realization.

The outcome is a selected plan with its evidence, alternatives, and limitations,
not a declaration of global optimality. A trial of generated payloads establishes
behavior for those payloads and processing kernels, not arbitrary application code.

The engineer's suggestion of roughly four to seven recognizable archetypes is a
hypothesis to test against workloads. The families below are a proposed starting
catalog, not a fixed count or a new requirement for callers to learn.

## Boundaries already settled

This proposal applies, rather than replaces:

- [DESIGN-NOTES.md](DESIGN-NOTES.md) `EP-D-2`: scoped physical locality and distinct
  developer partitions.
- [DESIGN-NOTES.md](DESIGN-NOTES.md) `EP-D-6`: planner-owned, caller-authorized
  measurement; scenario evidence stays separate from machine facts.
- [DESIGN-NOTES.md](DESIGN-NOTES.md) `EP-D-7`: neutral vocabulary, planner, inward
  fact adapter, measurement foundation, and outward realizer.
- [DESIGN-NOTES.md](DESIGN-NOTES.md) `EP-D-9`: common I/O endpoint vocabulary with
  typed storage and network capabilities.
- [DESIGN-NOTES.md](DESIGN-NOTES.md) `EP-D-10`: the controlling startup-cost boundary,
  superseding this proposal's original default-campaign assumption.

No application is pinned, probed, opened, transmitted to, or written to merely
because its specification was parsed. Description, binding, permission, and
execution are separate steps.

## 1. Describe the workload before its execution pattern

### What a developer supplies

The authoring surface should allow a short source -> processing -> sink description,
with detail added where the application has a requirement. A builder and serialized
input normalize to the same data contract; neither invents missing semantics.
An unspecified fact remains unknown. In particular, an unspecified stage is not
assumed stateless or parallelizable.

| Element | Required meaning |
|---|---|
| Endpoint role | A stable logical name, source/sink or duplex use, storage/network kind, required operations, and binding constraints |
| Stage | Logical identity, named implementation contract, input/output ports, framing, permitted concurrency, state ownership, ordering, and side effects |
| Flow | Source and destination ports, payload contract, delivery obligations, ordering scope, fan-out/routing meaning, and pressure policy |
| Workload profile | Known or unknown payload sizes, arrival schedule, burst shape, key/flow skew, expansion ratio, service profile, and workload phases |
| Constraints | Resource ceilings, allowed universe, named partitions, isolation, required placement, permitted copies, and adaptation limits |
| Objectives | An ordered list of metrics to optimize, plus separately identified acceptance thresholds |
| Measurement grant | Which mechanisms may run, against which resources, with what traffic/data, budgets, and cleanup obligations |

IDs name logical resources, not processor numbers or OS handles. A role such as
`input-store` can bind to a caller-provided open endpoint for this run without
putting a machine-specific device path or credentials in the reusable specification.
Concrete identity is retained in the run's binding record under its disclosure policy.

Ports name payload/framing contracts. A byte stream does not become independently
processable records because the planner split a read into buffers. The application
provides framing and partitioning hooks where those transformations are legal.

State ownership is one of: no mutable retained state, one owner for the whole stage,
one owner per declared key, or an application-provided concurrent-state contract.
Unknown ownership excludes transformations that require a particular answer and
produces a clarification if no candidate remains.

Ordering distinguishes input admission, processing/commit, output emission, and
external effect completion, each scoped to stream, key, or whole stage. Restoring
output sequence after parallel processing does not restore the order of side effects.
Aggregation and collation require their operation, window/end condition, and permitted
reassociation to be declared; associativity is not inferred from the word "collate."

### Start with the engineer's example

```text
input-store -> frame -> parse -> collate -> format -> output-store
```

For a worked instance, suppose framing emits independent records; parsing is pure;
collation has one mutable owner; formatting is pure; and output must follow collation
order. These are example assumptions, not defaults for all parsing workloads.

The planner can try the whole path on one domain, replicated parsing feeding a
single collation owner, or an input-local region and an output-local region joined
by an explicit transfer. Formatting can be replicated only with a bounded resequencer
if the required output order would otherwise be lost. A second variant with
key-partitionable collation admits per-key owners instead of one global owner.

The workload specification need not change when its endpoints occupy different memory
domains on another machine. Replacing a storage role with a network role changes that
role's declared capabilities and binding, but need not change the processing stages.
The candidate placements and available evidence follow the actual bound resources.

### Hard requirements and preferences are different fields

A correctness or placement requirement is never relaxed because another candidate
benchmarked faster. Preferences rank admissible candidates. A performance threshold
also names its evidence protocol: load, duration, phase, metric, and allowed evidence
class. Meeting a measured threshold is not a guarantee for all future traffic.

Cross-domain intent is explicit: prohibited, permitted with no declared purpose,
or intentional with a role/reason such as an endpoint handoff or state-owner boundary.
An intentional crossing still has a cost and must still satisfy hard requirements.
The diagnostic reports the crossing, evidence, and affected requirement; it does not
prescribe that locality must always win.

## 2. Match patterns; do not classify the application once and stop

An archetype is a versioned matcher and candidate generator. It is not a switch
that labels the whole application and hides alternatives. Regions can match several
families; a complete candidate records which matches compose and which logical nodes
and flows each covers.

| Proposed family | Match requires | Candidate variation | Must not invent |
|---|---|---|---|
| Serial pipeline | Stages whose required semantics are implementable by one owner in sequence | One domain or staged domains; batch sizes; polling/waiting | Parallelism or stage fusion forbidden by the application |
| Partitioned pipeline | Explicitly independent partitions and permitted replicated stages | Replicas near endpoints, per-domain lanes, bounded redistribution | Independence from raw byte offsets |
| Scatter/process/gather | Splittable work plus a specified gather/ordering operation | Worker count, dispatcher, collector, bounded resequencing | Associativity, loss of source order, or a multi-consumer queue |
| Key-owned service | Declared key extraction, partition mapping, and serialized state ownership | Owner count and placement, per-owner input queues | Concurrent updates to one key or live state migration |
| Broadcast/branch/join | Explicit recipient set, branch semantics, and join condition if any | Independent queues, shared immutable payload or copies, join location | Dropping a slow branch or equating broadcast with work distribution |
| Request/response | Correlation, bounded in-flight requests, reply path, and terminal outcomes | Shared or partitioned service lanes, response routing | Exactly-once external effects or unbounded retry cycles |

These are composable families rather than six mutually exclusive application types.
Arbitrary feedback must have explicit credit/termination semantics and a matching
protocol; an unexplained cycle is not silently converted to a pipeline.

Each match returns covered nodes/edges, semantic prerequisites, missing answers,
capability requirements, candidate dimensions, and rejection reasons. Unmatched
regions remain visible. A graph with no matching composition is unsupported by the
catalog version, not necessarily impossible to execute.

For offline research or separately requested tuning, trial order is deterministic:
validate semantics; prefer more specific
matches; generate an admissible low-resource baseline where one exists; then explore
placement, replication, batching, queue shape, and transfer alternatives. The first
match does not win by default. Structural priority decides what to try first, while
the declared objectives and collected evidence decide which tested candidate to select.

Enumeration has an explicit budget and records omitted dimensions. Pruning may remove
a candidate for a proved constraint violation or duplicate normalized plan. Heuristic
pruning is separately reported. Exhausting the search budget produces "no candidate
found within this search," never a claim that the specification is unsatisfiable.

### Queue shape is constrained by cardinality before locality

SPSC is legal only for one actual producer and one actual consumer. Proximity cannot
make a multi-producer edge SPSC. A parallel worker group can use a dispatcher feeding
one SPSC per worker, followed by a collector or a supported MPSC; it cannot be mapped
to an imaginary MPMC version of an existing single-consumer queue.

Locality then helps choose among legal arrangements. Queue implementation, producer
ownership, consumer ownership, reservation support, capacity, and layout limits are
part of the candidate, including any operational lifetime limit that the primitive
requires. A reserved control slot and an ordinary data slot are separately accounted.

## 3. Separate logical flows, candidate plans, and execution bindings

The neutral plan vocabulary contains:

| Record | Contents |
|---|---|
| Plan identity | Format version, specification digest, machine/allocation snapshot, catalog version, capability snapshot, evidence references |
| Execution domain | Dedicated or shared execution mode, processor set, binding strength, memory policy, resource allowance |
| Stage instance | Logical stage and replica/key ownership, implementation binding ID, input/output ports, batching and scheduling policy |
| Channel | Producer/consumer instances, implementation/capabilities, capacity in items and bytes, backing-memory domain, pressure and close protocol |
| Pool | Allocation domain, size/alignment classes, byte ceiling, registration/endpoint compatibility, and return owner |
| Endpoint binding | Logical role, run-local endpoint identity, backend, required operations, discovered attachment and provenance |
| Route | Ordered hops for a logical flow, intermediate owners/queues, transfer action, final recipient, return/acknowledgment path |
| Control protocol | Correlation, credits, cancellation, drain, termination, join/resequence, and admission rules |
| Resolution report | Satisfied constraints, explicit exceptions to preferences, rejected candidates, uncertainty, untested alternatives |

A cross-domain route is not merely an edge with an integer hop count. Each hop says
whether it moves an ownership descriptor, copies into a destination pool, or shares
an immutable lease. Moving a descriptor does not move the underlying pages. No-copy
may therefore mean deliberate remote access, and the plan describes that rather than
relabelling the memory local.

No raw pointer, borrowed reference, function address, or OS handle belongs in a
serialized plan. Execution binds names to code and resources and checks that the
binding satisfies the same contract used during planning.

### Plan validation is owned by the neutral model

One validator checks the following for every generated, deserialized, or manually
constructed plan. Planner generation, trial preparation, and the realizer all call it.
Tests consume its result rather than carrying another copy of its predicates.

- Every ID/reference resolves, every logical flow has a complete route, and every
  stage has exactly the declared ownership and instance coverage.
- Required ordering, framing, replication, join, and delivery semantics are preserved.
- Queue cardinality and capabilities match its users; all buffers, reservations,
  resequencing windows, pending requests, and intermediate hops have bounded budgets.
- Hard processor placement stays inside the authorized active universe. Shared
  execution is not represented as a processor-pinning guarantee.
- Pool placement, payload access mode, endpoint registration, and operation lifetime
  constraints are compatible. Unknown required placement never becomes node zero.
- Every state owner, buffer lease, accepted item, and operation has a termination
  or recovery owner; no cancellation edge authorizes freeing outstanding I/O memory.
- Dataflow cycles require explicit bounded feedback. Control/credit paths cannot
  depend solely on room in the saturated data path they are meant to unblock.
- All resource arithmetic is checked, required capabilities are present, and all
  realization actions are within the caller's grants.

Validation does not prove arbitrary client code terminates or obeys declared semantics.
Those are application contracts. Unproved progress obligations remain explicit;
catalog recipes carry the progress protocol they implement rather than asserting
that a finite queue graph alone proves freedom from deadlock.

## 4. What the project supplies above the primitives

Recommendation: provide the execution plumbing that makes a generated candidate
real, but not the application's parser, formatter, collation algorithm, or business
state. Otherwise trials would benchmark one orchestration while callers implement
another.

Application bindings supply stage factories, framing/key hooks, state initialization,
and effect-aware processing functions. Each contract names its version and whether
it can execute with fixture data in a trial. The runtime supplies bounded channels,
dispatch and gather, buffer leases, scheduling, credits, correlation, completion
routing, cancellation, and rundown.

Stage execution is resumable and budgeted: consume an admitted item/batch, produce
bounded outputs, yield for input/output readiness, complete, or fail. A stage that
can block declares that fact and needs an execution domain that permits blocking.
When an output is full, the stage retains its input/continuation; it does not spin
indefinitely on a domain needed to drain that output. A single-thread candidate
must still interleave stage progress rather than recursively filling its own queues.

A framing stage declares its maximum retained prefix or an authorized spill policy.
Output expansion, gather state, and reorder storage count toward memory limits.
An unbounded whole-input collation cannot be made bounded by choosing small queues:
it needs a declared finite dataset, an external/spill algorithm, or a refusal.

Admission creates a run-local identity; fan-out adds child identities and a parent
join obligation. Backend operation tokens are mapped to those identities without
replacing their own ring/port identities. Completion means the declared terminal
event, not necessarily durable storage or receipt by a remote application.

Buffer leases keep the actual allocation and its return path alive until kernel I/O,
stages, and branches release their interests. Sharing a lease for reading does not
authorize concurrent mutation. Abort returns a report of completed, failed, cancelled,
and externally indeterminate effects; it does not promise rollback of writes or sends.
Graceful close stops admission and drains; forced stop requests cancellation and
retains resources that still have external owners until rundown actually completes.

### Reuse and genuine missing work

| Existing evidence | Use here | What it does not supply |
|---|---|---|
| Queue [README.md](../windows-waitable-queues/README.md) and [traits.rs](../windows-waitable-queues/src/traits.rs) | Bounded SPSC/MPSC capabilities, readiness, reservation where supported | Generic worker-pool distribution, pipeline ordering, or producer-space waiting as a universal capability |
| I/O ring [README.md](../windows-ioring-sys/README.md) | File data plane; separate shared-callback and owned-domain delivery architectures | Socket operations or ordering across different completion backends |
| Overlapped I/O [README.md](../windows-overlapped-io-sys/README.md) and [socket.rs](../windows-overlapped-io-sys/src/socket.rs) | File/socket operations, owned buffers and operation completion | Workload stage scheduling or RSS control |
| Thread pool [pool.rs](../windows-threadpool-sys/src/pool.rs) and [callback_env.rs](../windows-threadpool-sys/src/callback_env.rs) | Pool selection, callback priorities, waits and asynchronous dispatch | An owned shard with a specified processor and state owner |
| Ambient-state [README.md](../windows-thread-ambient-sys/README.md) | Capturing/applying thread context under its restore contract | A completed domain runtime or a proof of stack NUMA placement |
| [CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) M32/M33+ | Existing owner for domain ordering, correlation, pressure, thread construction and execution | Those open contracts cannot be assumed already implemented |

Keep the shared-callback and dedicated-domain execution paths separate. A shared
pool can implement a candidate that permits placement variability; it cannot satisfy
a hard owned-thread contract merely by limiting the pool size. Callback priority is
not an application-level sequencing guarantee; Microsoft's
[callback-priority contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadpoolcallbackpriority)
distinguishes dispatch precedence from finishing order.

The measurement foundation and realizer should use the same domain/channel/dispatch
execution mechanisms owned below them in the runtime layer. A trial adds fixture
sources, clocks, and observation around those mechanisms; it must not introduce a
second production scheduler inside a probe. This preserves the five component roles
and the isolation of existing working delivery paths.

M32/M33+ are a real implementation dependency, not a reason to stop this design.
Resolve their contracts with the owning plan before dependent code lands. The
acceptance handoff in section 10 records this work for EP-R1.8; this proposal does
not silently decide the root plan's open items.

## 5. Generate experiments without inventing evidence

This section specifies offline research and separately requested tuning, not a
mandatory deployment sequence. Runtime probes are restricted by
[EP-D-10](DESIGN-NOTES.md#ep-d-10); its reconciliation item will identify which
mechanisms, if any, fit that budget. Sharing mechanisms does not import the
warm-up, repetitions, workload duration or parameter matrix into startup.

Every result records three independent dimensions:

| Dimension | Examples |
|---|---|
| Execution | Pure model/fake platform; live runtime and queues |
| Stage behavior | Synthetic service kernel; actual application binding |
| Endpoint path | Simulated completion; fixture file/device; loopback; authorized external peer |

Live queues with synthetic parsing are hardware evidence about that transport and
synthetic service, not measured parser performance. Loopback results do not establish
the physical NIC path. A trial with fixture files does not establish another volume's
performance. Mocks never enter the hardware-evidence class.

The fixture description carries payload/framing sizes, key and connection distribution,
arrival schedule, burst/recovery phases, output expansion, service behavior, and
deterministic generation identity. Application-provided fixture generators may supply
valid inputs for actual parser/formatter bindings. CPU delay and memory-touch kernels
are alternative service models, not substitutes silently labelled as those bindings.

A trial measures offered, admitted, and completed work separately; failures and
backpressure are not removed from latency accounting. Record queue residence,
processing and end-to-end response distributions, bytes moved, memory high-water,
throughput, unfinished work, and drain/recovery time. Timestamp offered work at its
scheduled arrival, not only when a blocked producer finally succeeds in admitting it.
Otherwise a saturated candidate can hide its waiting time by slowing the generator.

Experiment phases are setup, placement verification, warm-up, steady traffic,
declared bursts, recovery, and rundown. Shared-resource effects require full composed
candidate trials: isolated edges cannot predict contention between two flows using
one controller, link, core, or memory domain.

Use an identical fixture specification for comparable candidates and balanced,
deterministically scheduled candidate order across repetitions. Retain variation and
achieved placement, not just a mean. Cache state, endpoint configuration, and concurrent
load are part of the record; if they cannot be controlled, report the limitation.
No percentile acceptance is reported from a sample too small for its declared protocol.

The EP-X1.1 comparison-descriptor requirements are now recorded in the experiment's
[DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md) -> `RC-D8`.
Apply that distinction between resource ceilings, role/concurrency assignments and
observations when materializing the evidence vocabulary; the production types and
selection policy remain open. The
[capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-1/README.md)
supplies the experiment evidence, not acceptance of this whole proposal.

The independent read/payload credits and explicit scheduling-batch semantics added
by EP-X1.2 are defined in the experiment's
[DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md) -> `RC-D9`.
Its [capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-2/README.md)
is awaiting result discussion; it does not settle a production batch or queue default.

EP-X2.2's request-service evidence requirements and path disposition are recorded in
[DESIGN-NOTES.md](experiments/request-reply/DESIGN-NOTES.md) -> `RR-D4`, supported by
its [demonstration](experiments/request-reply/captures/2026-09-19/README.md).
Carry its distinction between logical lanes, worker eligibility, admitted-work credits,
queue capacities and terminal outcomes into future plan/runtime contracts. The in-memory
experiment supplies no physical placement or kernel-completion scheduling claim.

EP-X2.3's stateful acceptance and disposition are in the same experiment's
[DESIGN-NOTES.md](experiments/request-reply/DESIGN-NOTES.md) -> `RR-D6`, with its
[stateful record](experiments/request-reply/captures/2026-09-19-stateful/README.md).
Carry key identity, partition/owner mapping, per-key commit order and cancellation
effect boundaries into the future plan/runtime contract. Intermediate lookup results
need verification as well as final state; neither key skew nor worker ownership is
a discovered machine-locality fact. No stateful candidate is chosen as a default.

### Permissions and budgets

The grant names allowed endpoints and operations, scratch storage, read ranges,
permitted writes, traffic peers, configuration changes, CPU/time/memory/I/O ceilings,
and teardown reserve. Absent authority denies that action; the planner may request
clarification, not silently broaden it. Read-only access is still resource-consuming.

Do not use application output files as benchmark scratch space. Running application
code requires an explicit fixture/effect contract; sending test network traffic
requires an authorized peer/path. Discovery of RSS settings does not authorize changing
adapter-wide settings. Reconfiguration, where allowed, captures state, checks for
concurrent change, and reports restoration failure without overwriting a newer owner's
configuration.

Budgets include setup, warm-up, repetitions, outstanding I/O, and teardown. Wall-clock
timeouts stop new admission and begin cancellation; they cannot promise that arbitrary
in-process callbacks or drivers have stopped. Trial bindings must cooperate with
cancellation or use an explicitly authorized isolated worker process. Even then,
external effects and outstanding device work need an explicit final status.

If teardown cannot complete, return outstanding-resource information and a retained
rundown owner, and prohibit conflicting follow-on trials. Do not free kernel-visible
buffers to meet a time budget or present a timed-out trial as a completed observation.

### Candidate ranking

First enforce structural constraints and required correctness facts. Topology and
application constraints may select an arrangement without active measurement under
[EP-D-10](DESIGN-NOTES.md#ep-d-10). Do not fabricate performance evidence or silently
claim a measured threshold is satisfied. Missing required performance evidence is
an explicit unresolved requirement, not authority to run startup workload sweeps.
The precise selection and missing-evidence policy is queued as `EP-R1.7.1`.

Among eligible candidates, the caller's ordered objectives still govern selection.
Use declared units and comparison tolerances for available metrics; do not invent
a weighted sum between latency and throughput. If an objective order is missing
and changes the answer, request it rather than selecting a hidden business policy.

Where available measurements do not distinguish candidates, report that uncertainty;
absence of measurement is not a measured tie. A deterministic
resource-footprint and canonical-plan-ID tie-break selects one without claiming it is
faster. The result records the tested set and search limits, and does not claim the
selected candidate outranks untested candidates. A better later measurement can alter
selection without changing the meaning of the specification.

## 6. I/O endpoints: common vocabulary, distinct control

Keep physical attachment, configured preference, and observed execution separate.
A NUMA preference is not evidence that an application's buffer pages or callback
actually landed there. Unknown or composite attachment can be a set of observations
with provenance; it must not be reduced to the first node to simplify placement.

Storage bindings distinguish controller, namespace, volume, file and open endpoint.
Several logical roles can share a device/controller; account for that shared capacity.
Required read/write/alignment/registration/durability capabilities are verified on
the bound backend. Cross-backend completion and flush dependencies are explicit.

Network bindings distinguish interface, connection/flow, software completion queue,
and reported hardware queues. RSS is a receive-side placement input, not a promise
that a user-mode parser or transmit completion runs on that processor. The
[RSS contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/network/receive-scaling)
describes target processors and DPC processing; application scheduling is another
step. The plan records both instead of identifying them as one queue or one thread.

Use a capability-checked socket backend for network candidates. The current
[I/O ring operation vocabulary](https://learn.microsoft.com/en-us/windows/win32/api/ntioring_x/ne-ntioring_x-ioring_op_code)
does not provide a socket operation; the local overlapped socket adapter is a separate
source of mechanisms. Features such as registered networking or remote-memory access
must be explicit backend capabilities, not inferred from link speed.

Interconnect resource records distinguish known shared links/bandwidth constraints
from unknown paths. Do not create independent bandwidth budgets for devices sharing
a reported upstream link. Workload-specific cost remains measurement evidence, and
forward/reverse flows retain separate observations.

## 7. Planning outcomes, callbacks, and realization

Prefer a resumable planning session over arbitrary nested callbacks. Its next step
returns a request or terminal outcome, and a host services the request through separate
clarification, measurement, and binding/execution interfaces.

```text
validate -> bind/discover -> match -> instantiate -> validate candidates
         -> evaluate -> prepare selection
```

There is no mandatory trial phase. An optional probe request must obey
[EP-D-10](DESIGN-NOTES.md#ep-d-10), including the total planning/realization budget.

Any phase can produce a typed diagnostic or a request for missing information.
Each request has an ID, session generation, expected response contract, cancellation
state, and budget. The host may answer, explicitly decline, or cancel. Stale, duplicate,
or wrong-kind responses are errors. No callback runs while planner-internal locks are
held, and no callback may expand the grant by returning a convenient default.

Clarifications ask for missing semantics, ambiguous endpoint binding, incompatible
constraints, objective priorities, or requested additional authority. They do not ask
clients to choose queue algorithms the planner is responsible for choosing. A headless
host gets a structured needs-input result; it does not hang waiting for a user.

Permission to measure is separate from permission to install or start the final plan.
Before activation, the realizer revalidates specification, machine/allocation,
bindings, capabilities, required evidence and resource budgets. It prepares resources
with ingress stopped, verifies achieved placement, and activates only after preparation
succeeds. Partial setup failures unwind resources in dependency order and report
cleanup failures separately from the initiating error.

Topology or configuration drift before activation invalidates affected claims and
returns control for rediscovery/replanning; do not silently adapt a serialized plan.
Running-system migration is not settled here: the existing runtime-replanning item in
[CHECKLIST.md](CHECKLIST.md) remains open. Detection and reporting of invalidated
assumptions are required even before automatic live migration exists.

| Outcome/error | Meaning and required information |
|---|---|
| Invalid specification | Field/ID and semantic violation; no trial has run |
| Needs input | Typed clarification and dependent candidate/constraint |
| Unsupported shape/capability | Unmatched region or required capability; not claimed impossible in principle |
| Conflicting requirements | A demonstrated conflicting set; do not claim a minimal unsatisfiable core unless computed |
| Search exhausted | Bounds reached, candidates considered and remaining dimensions |
| Evidence insufficient | Missing fact/metric, attempted measurement, refusal/failure/uncertainty and allowed next action |
| Candidate rejected | Constraint/threshold failed with supporting evidence; other candidates may remain |
| Proposed plan | Valid under stated facts and assumptions but lacks required evidence/binding for activation |
| Ready selection | Valid candidate plus satisfied activation prerequisites as of its snapshot; zero active probes is supported and realization still rechecks |
| Cancelled/failed campaign | Partial evidence, external effects, outstanding resources and cleanup status |
| Stale/failed realization | Changed assumption or setup failure, with acquired-resource disposition |

An assumption can concern an optimization only when the specification explicitly
allows it. It cannot substitute for evidence needed for memory safety, ownership,
authorization, or a required semantic guarantee. A required but unanswered NUMA
placement refuses that candidate; a policy permitting an explicitly non-local pool
can instead generate and report that different candidate. Unknown cache relation
does not mean "remote": exclude rules requiring it or seek permitted evidence.

## 8. Reproducibility and JSON

Specification, candidate, and evidence formats have independent major/minor versions
and explicit required-feature identifiers. Unknown major versions or required
capabilities are refused. Core semantic fields and discriminants are strict; additive
optional data belongs in a namespaced extension map with a required/optional marker.
A consumer cannot ignore an extension that changes admissibility or execution.

Reject duplicate object keys and duplicate IDs. Units are field-defined; integer
quantities that exceed interoperable JSON exact-number range use canonical decimal
strings. Exclude non-finite numeric values. Canonical serialization specifies stable
ID ordering, integer spelling and map ordering for digests; pretty output is not the
digest input.

The same normalized input, catalog/capability versions, search budget, and recorded
answer/evidence stream yield the same candidates, diagnostics, and selection.
Machine timing itself is not deterministic. Replaying a planning transcript proves
policy reproducibility, not that fresh measurements will select the same candidate.

The concrete plan references evidence by content identity, including its workload,
backend and runtime configuration. A reader cannot silently reuse a capture for a
different allocation, endpoint, service binding, or catalog assumption. Deserialization
creates unvalidated data; validation and live binding are required before execution.
Serialized artifacts contain no credentials or executable callbacks; the host resolves
code/resource references through an explicit registry.

## 9. Acceptance matrix

These are proposed acceptance obligations, not tests claimed to exist or have run.
`U` means deterministic in-memory model/fake-backend tests. `I` means live OS/runtime
integration. `H` means hardware/path characterization with the named topology; an
unavailable host yields an explicit unrun result, never a passing mock substitute.

[EP-D-11](DESIGN-NOTES.md#ep-d-11) governs NUMA acceptance across this matrix. Inject
one consistent scenario through gathering and fake realization to validate behavior;
do not make multi-node hardware a separate closure gate for each row. Physical NUMA
checks belong to the single `EP-HW.1` follow-up in [CHECKLIST.md](CHECKLIST.md), not
to each software milestone. `H` observations remain distinct from behavioral tests
and do not establish a required performance baseline. Other OS/endpoint requirements
are not silently waived by this NUMA-specific policy.

### Representative normal workloads

| ID | Workload | Required acceptance |
|---|---|---|
| N1 | One source, serial parse/format, one sink, one CPU | U: legal single-domain candidate makes progress under full queues and bounded memory; I: implementation returns all accepted items |
| N2 | Two NVMe roles, parse then serial collate then format | U: inject distinct endpoint attachment domains consistently through gathering and fake realization; preserve collation owner and output order; verify source-local, sink-local and split placement requests. Physical NUMA fidelity follows EP-D-11. |
| N3 | Independent file partitions with pure transforms | U: enumerate replica counts within limits and preserve record boundaries; I: fixture outputs equal the serial application oracle |
| N4 | Parallel parsing with ordered output | U: deliberately inverted completion order is resequenced within its bound; pressure propagates when the missing item stalls |
| N5 | Per-key stateful aggregation | U: adversarial key skew never creates two simultaneous owners for one key; ordering remains per declared key |
| N6 | NIC receive, frame/parse, NVMe output | U: bind distinct network/storage capabilities; I: real socket/file completion ownership; H: physical NIC-path trial distinguished from loopback |
| N7 | NVMe read, format, NIC transmit | U: independently parameterize reverse flow and partial sends; I: output stream equals fixture stream without duplication |
| N8 | Broadcast to storage and network sinks | U: each required branch receives its obligation; slow branch applies specified pressure; lease returned only after all interests finish |
| N9 | Scatter with deterministic gather | U: preserve declared gather operation and correlation across interleaved completions; bounded gather storage |
| N10 | Bounded request/response service | U: credits bound outstanding work; reordered replies correlate; cancellation and late completion do not collide with reused IDs |
| N11 | Mixed shared-pool control and dedicated data domains | U: isolate execution contracts; I: callback path is not used to claim dedicated placement or ordering |
| N12 | Same flow across synthetic machine families | U: one faux environment drives gathering, selection and fake realization for single/multiple nodes, no L3, hybrid cores, processor groups, incomparable relations and restricted universes; preserve unknowns and prevent synthetic identities reaching live placement calls |
| N13 | Multiple logical endpoints on one physical controller/link | U: joint capacity accounting; H: composed trial reports interference rather than summing isolated throughputs |
| N14 | Bursty flow with low steady rate | U: replay offered-arrival schedule and pressure accounting; I: observe recovery and drain, not just steady throughput |

### Edge conditions and failure paths

| ID | Condition | Required acceptance |
|---|---|---|
| E1 | Empty universe, empty query, unknown processor, duplicate query IDs | U: reject first three as applicable; normalize duplicate set members without changing the question |
| E2 | Missing core/cache/memory or contested provenance | U: retain each absence; no invented efficiency class or node; allow only candidates whose prerequisites remain established |
| E3 | Multiple locality minima or composite endpoint attachment | U: retain ambiguity and evaluate applicable alternatives; never first-select as fact |
| E4 | Offline/out-of-allocation CPU, uninformative allocation flags | U: real constraints govern eligibility; false flag alone neither excludes nor grants a processor |
| E5 | Unknown framing, state or collation semantics | U: ask or exclude dependent transformations; do not turn unknown into stateless parallel work |
| E6 | Incompatible payload ports, invalid IDs, disconnected/unmatched region | U: exact source diagnostic; valid disconnected independent flows remain legal |
| E7 | Unsatisfied placement versus permitted intentional crossing | U: reject hard violation and accept deliberate legal crossing with diagnostic, not an automatic warning verdict |
| E8 | Wrong queue cardinality, missing reservations, unsupported layout/lifetime | U: reject invalid recipe; legal SPSC and MPSC counterparts accepted |
| E9 | Zero/overflowing capacities, oversized payload, expanding output | U: checked arithmetic, explicit refusal/pressure, and no hidden unbounded allocation |
| E10 | Cyclic flow, credit starvation, full data path during shutdown | U: refuse undeclared cycle; declared feedback and control paths terminate without needing a data slot |
| E11 | Slow/failed broadcast branch or missing gather result | U: retain ownership and pressure; apply declared timeout/failure protocol without silent loss |
| E12 | Immediate, pending, partial, failed and out-of-order I/O completion | U: every operation follows its backend contract; I: exercise file/socket paths and retain buffers through cancellation |
| E13 | Trial permission absent or read-only, unauthorized endpoint/peer | U: no prohibited backend call; denied trial returns insufficient evidence, not zero cost |
| E14 | Budget expires in setup, warm-up, run or teardown | U: separate partial data from accepted evidence; expose pending rundown; I: verify no unsafe early resource release |
| E15 | Stage fails, blocks without cooperation or panics | U: declared failure policy; I: cooperative stop or process isolation contract; no promise of in-process preemption |
| E16 | Permission/RSS/configuration restore fails or concurrent owner changes it | U: report original and cleanup failures; never overwrite the newer state; I: granted fixture resource only |
| E17 | Synthetic processing, loopback or fake backend passed as deployment evidence | U: evidence-class mismatch refused; corresponding permitted transport-only claim accepted |
| E18 | Tied/noisy measurements, too few samples, workload changes between trials | U: disclose inconclusive comparison, enforce evidence requirements, deterministic tie-break only where allowed |
| E19 | Search/callback budget exhausted, no feasible candidate found | U: no proof-of-impossibility claim; record searched/unsearched work and decline state |
| E20 | Duplicate/stale/wrong-kind callback response and cancellation race | U: typed error or cancellation outcome; no double execution of accepted response |
| E21 | Reordered input records/maps, repeated transcript | U: same canonical plan/diagnostics; permitted ordering-sensitive input remains significant |
| E22 | JSON unknown version, required extension, duplicate key, malformed quantity | U: reject; known optional extension survives round-trip without influencing core semantics |
| E23 | Stale machine, removed device or achieved placement differs at activation | U/I: ingress remains stopped, acquired resources accounted, stale-plan result rather than silent fallback |
| E24 | Side effect completed but acknowledgment lost, multi-domain flush | U: report indeterminate outcome where necessary; never invent exactly-once or cross-ring durability |
| E25 | RSS locality differs from user worker or buffer locality | U: independent records; H: observe each claim separately on a permitted physical path |
| E26 | Unbounded collation/frame retention or absent spill permission | U: require bounded algorithm/input or refuse; valid bounded counterpart accepted |
| E27 | Admission pressure hides offered-load waiting | U: synthetic clock proves latency starts at scheduled arrival and incomplete/rejected work stays counted |
| E28 | Queue counter lifetime exhausted | U: backend-specific bound enforced and stop/drain action precedes reuse; no claim that a long measured horizon prevents wrap |

Unit tests use deterministic fixtures and bounded state exploration, not random
runtime sampling. Integration tests run the shared execution mechanisms across the
actual OS boundary. Hardware results carry host/path/provenance and stay separate
from portable acceptance. Ordinary CPU/memory timing in a faux environment is not
a simulation of NUMA distances, and no timing penalty is added to claim that fidelity.

Bindings to the shared plan validator and evidence classifier need persistent sabotage:
alter the owning predicate, verify generator/trial/realizer behavior changes, and include
valid-but-surprising controls that must survive. Store those cases in the owning
component's sabotage manifest when implementation lands; do not claim a source-text
grep verifies an executed plan.

## 10. Consequences for implementation planning

This proposal does not close EP-R1.7. Acceptance requires discussion of section 11,
promotion of accepted contracts to Tier 1, and reconciliation with the owning runtime
decisions. Existing `EP-1.4`, `EP-1+.1`, and `EP-1+.2` can then be closed together for
the contracts actually settled, with their own completion provenance.

If accepted, EP-R1.8 must distribute these obligations without treating them as already
implemented:

| Owner | Required work and handoff |
|---|---|
| Neutral model | Specification normalization, endpoint/plan/evidence vocabulary, shared validator, format/version rules, acceptance fixtures |
| Planner | Match catalog, bounded candidate generation, resumable session, evidence policy, ranking and diagnostics |
| Inward Windows component | Endpoint identity reconciliation, capability and allocation snapshots, provenance, physical/configured/observed distinctions |
| Measurement foundation | Permission enforcement, reproducible load generation, actual-stage fixture binding, live/fake separation, campaign accounting and rundown |
| Domain/runtime owners | Resolve M32 contracts; shared stage/queue/lease/dispatch mechanisms; buffer-placement and wakeup capabilities; backend completion adapters |
| Outward component | Registry/binding validation, transactional preparation, achieved-placement checks, activation and teardown |

The dependency order is neutral contracts and runtime contract reconciliation, then
mechanisms/backends, then candidate execution/trials, then selection and realization.
Model-only candidate generation and fake-backend tests can proceed before hardware
characterization, but do not discharge live execution obligations.

Any queue placement, producer readiness, thread binding, or network-discovery gap
belongs in its owning lower layer. Existing baselines and experimental peer paths
remain separate until a deliberate merge-or-delete decision; benchmark convenience
does not authorize collapsing them.

Promotion must also reconcile the current statements this proposal would change:
the quadratic all-pairs-ring framing in EP-D-1; proximity alone choosing SPSC/MPSC
in EP-D-2 and M3+.2; the older claim that the planner cannot measure to resolve a
proximity gap; and the unspecified scenario/callback/runtime boundaries in
[DESIGN-NOTES.md](DESIGN-NOTES.md), [COMPONENT.md](COMPONENT.md), and
[CHECKLIST.md](CHECKLIST.md). Do not leave the proposal as a contradictory second
authority. Keep the historical discussion in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

The open root M32/M33+ dependency is recorded here for reconciliation, not deferred
for lack of a caller. Reciprocal handoffs and component names are finalized by
`EP-1+.5` and `EP-R1.8`; no new provisional crate is created by this proposal.

## 11. Choices to discuss, with recommendations

Most mechanics above follow from preserving workload semantics and the settled
layering. These choices change the product boundary and need the engineer's judgment:

| Choice | Recommendation | Consequence |
|---|---|---|
| How much execution machinery the project owns | Supply stage plumbing, bounded dispatch/gather, leases and rundown; clients supply processing algorithms | A plan can be tried and realized through the same machinery rather than becoming a suggestion clients reimplement |
| How representative offline trials become | Retain distinct transport/service-model and actual-stage evidence classes outside startup, under [EP-D-10](DESIGN-NOTES.md#ep-d-10) | Offline evidence does not oblige deployments to replay workload validation |
| Catalog coverage versus a universal optimizer | Start with the composable families derived above and version the catalog; unmatched regions are visible | The system explains its current coverage instead of forcing arbitrary workloads into a template |
| Insufficient performance evidence | Reconcile under `EP-R1.7.1`: support topology-first zero-probe selection without asserting unknown performance or relaxing correctness/authority | Missing measurements do not automatically force a fallback or a startup benchmark campaign |

No answers are presumed by writing this table. The proposal supplies a concrete design
to argue against; it does not turn the engineer's archetype intuition into an accepted
API without discussion.
