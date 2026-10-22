// Copyright (c) 2026 Mike Grier
//! Pairs the begin and end events of a trace of the provider and says how long each pair took (DE-D-6): the way a reader of
//! the trace computes a duration, since nothing in durable-ioring measures one.
//!
//! The input is `tracerpt`'s CSV dump of a trace, `tracerpt trace.etl -o trace.csv -of CSV`. A `Start` opens a pair in its
//! activity and a `Stop` in the same activity ends it, and each must agree with the other on its task and on the pair id it
//! carries in its `Pair` field. Its duration is the gap between their `Clock-Time` columns, which `tracerpt` gives as system
//! time in 100-nanosecond units unless asked for raw timestamps. Events of other providers are skipped.
//!
//! `cargo run -p durable-ioring-events --example pair_trace -- trace.csv` prints each pair and exits with a failure if any
//! event breaks the pairing.

#![cfg(windows)]

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use durable_ioring_events::{PROVIDER_NAME, TASKS, schema_of_id};

#[cfg(test)]
mod tests;

/// The opcodes a pair is written with: `win:Start` and `win:Stop`.
mod opcode {
    pub const START: u8 = 1;
    pub const STOP: u8 = 2;
}

/// The activity id the platform says is no activity.
const NO_ACTIVITY: &str = "{00000000-0000-0000-0000-000000000000}";

/// One event of the provider, as the dump gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    id: u16,
    opcode: u8,
    task: u16,
    thread: String,
    activity: String,
    clock: u64,
    data: Vec<String>,
}

impl Row {
    /// The pair id the event carries in its `Pair` field, found by the provider's own table.
    fn pair(&self) -> Option<&str> {
        let schema = schema_of_id(self.id)?;
        let at = schema.columns.iter().position(|c| c.name == "Pair")?;
        self.data.get(at).map(String::as_str)
    }
}

/// A begin and its end.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pair {
    task: u16,
    pair: String,
    activity: String,
    took: Duration,
    began_on: String,
    ended_on: String,
}

impl fmt::Display for Pair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let task = TASKS
            .iter()
            .find(|t| t.value == self.task)
            .map_or("?", |t| t.name);
        write!(
            f,
            "{task} pair {} in activity {}: {:.3} ms, begun on thread {} and ended on {}",
            self.pair,
            self.activity,
            self.took.as_secs_f64() * 1000.0,
            self.began_on,
            self.ended_on
        )
    }
}

/// An event that breaks the pairing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Broken {
    /// A start or a stop in no activity.
    NoActivity { id: u16 },
    /// A start in an activity a start already opened.
    StartedTwice { activity: String },
    /// A stop in an activity no start opened.
    StopWithoutStart { activity: String },
    /// A stop of another task than its start's.
    TaskDiffers {
        activity: String,
        began: u16,
        ended: u16,
    },
    /// A stop that names another pair than its start.
    PairDiffers {
        activity: String,
        began: Option<String>,
        ended: Option<String>,
    },
    /// A stop that came before its start.
    EndedBeforeBegun { activity: String },
    /// A start never stopped.
    LeftOpen { activity: String },
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoActivity { id } => write!(f, "event {id} is a pair's but has no activity"),
            Self::StartedTwice { activity } => write!(f, "activity {activity} was started twice"),
            Self::StopWithoutStart { activity } => {
                write!(f, "activity {activity} was stopped and never started")
            }
            Self::TaskDiffers {
                activity,
                began,
                ended,
            } => {
                write!(
                    f,
                    "activity {activity} began as task {began} and ended as task {ended}"
                )
            }
            Self::PairDiffers {
                activity,
                began,
                ended,
            } => {
                write!(
                    f,
                    "activity {activity} began as pair {began:?} and ended as pair {ended:?}"
                )
            }
            Self::EndedBeforeBegun { activity } => {
                write!(f, "activity {activity} ended before it began")
            }
            Self::LeftOpen { activity } => {
                write!(f, "activity {activity} was started and never stopped")
            }
        }
    }
}

/// The provider's events in a dump, in the order it gives them.
///
/// # Errors
///
/// A dump without the columns this reads, or a row of the provider whose fields cannot be read.
fn read(dump: &str) -> Result<Vec<Row>, String> {
    let mut lines = dump.lines();
    let header: Vec<&str> = lines
        .next()
        .ok_or("an empty dump")?
        .split(',')
        .map(str::trim)
        .collect();
    let at = |name: &str| {
        header
            .iter()
            .position(|h| *h == name)
            .ok_or(format!("no `{name}` column"))
    };
    let (provider, id, opcode, task, thread, activity, clock) = (
        at("Event Name")?,
        at("Event ID")?,
        at("Opcode")?,
        at("Task")?,
        at("TID")?,
        at("Activity ID")?,
        at("Clock-Time")?,
    );
    let data = at("User Data")?;
    if data + 1 != header.len() {
        return Err("`User Data` is not the last column".into());
    }
    let mut rows = Vec::new();
    for (n, line) in lines.enumerate() {
        // Only the last column can hold a comma, inside a quoted string, so the fields before it split plainly.
        let fields: Vec<&str> = line.splitn(header.len(), ',').map(str::trim).collect();
        if fields.get(provider) != Some(&PROVIDER_NAME) {
            continue;
        }
        let field = |i: usize| {
            fields
                .get(i)
                .copied()
                .ok_or(format!("row {} is short", n + 2))
        };
        let number = |i: usize| -> Result<u64, String> {
            field(i)?
                .parse()
                .map_err(|_| format!("row {}: `{}` is not a number", n + 2, fields[i]))
        };
        rows.push(Row {
            id: u16::try_from(number(id)?).map_err(|e| e.to_string())?,
            opcode: u8::try_from(number(opcode)?).map_err(|e| e.to_string())?,
            task: u16::try_from(number(task)?).map_err(|e| e.to_string())?,
            thread: field(thread)?.to_owned(),
            activity: field(activity)?.to_lowercase(),
            clock: number(clock)?,
            data: values(fields.get(data).copied().unwrap_or(""))?,
        });
    }
    Ok(rows)
}

/// The values of a `User Data` column: separated by commas, a string quoted, with a doubled quote for a quote in it.
fn values(data: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = data.trim().chars().peekable();
    while chars.peek().is_some() {
        while chars.next_if(|c| *c == ' ').is_some() {}
        let mut value = String::new();
        if chars.next_if_eq(&'"').is_some() {
            loop {
                match chars.next() {
                    Some('"') if chars.next_if_eq(&'"').is_some() => value.push('"'),
                    Some('"') => break,
                    Some(c) => value.push(c),
                    None => return Err(format!("an unterminated string in `{data}`")),
                }
            }
            while chars.next_if(|c| *c == ' ').is_some() {}
        } else {
            while let Some(c) = chars.next_if(|c| *c != ',') {
                value.push(c);
            }
            value = value.trim_end().to_owned();
        }
        out.push(value);
        match chars.next() {
            None | Some(',') => {}
            Some(c) => return Err(format!("`{c}` after a value in `{data}`")),
        }
    }
    Ok(out)
}

/// Each begin with its end, in the order the ends came, and what broke the pairing.
fn pair(rows: &[Row]) -> (Vec<Pair>, Vec<Broken>) {
    let mut open: BTreeMap<&str, &Row> = BTreeMap::new();
    let (mut pairs, mut broken) = (Vec::new(), Vec::new());
    for row in rows {
        if row.opcode != opcode::START && row.opcode != opcode::STOP {
            continue;
        }
        if row.activity == NO_ACTIVITY {
            broken.push(Broken::NoActivity { id: row.id });
            continue;
        }
        let activity = row.activity.as_str();
        if row.opcode == opcode::START {
            if open.insert(activity, row).is_some() {
                broken.push(Broken::StartedTwice {
                    activity: activity.to_owned(),
                });
            }
            continue;
        }
        let Some(began) = open.remove(activity) else {
            broken.push(Broken::StopWithoutStart {
                activity: activity.to_owned(),
            });
            continue;
        };
        if began.task != row.task {
            broken.push(Broken::TaskDiffers {
                activity: activity.to_owned(),
                began: began.task,
                ended: row.task,
            });
            continue;
        }
        if began.pair() != row.pair() || row.pair().is_none() {
            broken.push(Broken::PairDiffers {
                activity: activity.to_owned(),
                began: began.pair().map(str::to_owned),
                ended: row.pair().map(str::to_owned),
            });
            continue;
        }
        let Some(ticks) = row.clock.checked_sub(began.clock) else {
            broken.push(Broken::EndedBeforeBegun {
                activity: activity.to_owned(),
            });
            continue;
        };
        pairs.push(Pair {
            task: row.task,
            pair: row.pair().unwrap_or_default().to_owned(),
            activity: activity.to_owned(),
            took: Duration::from_nanos(ticks.saturating_mul(100)),
            began_on: began.thread.clone(),
            ended_on: row.thread.clone(),
        });
    }
    broken.extend(open.into_keys().map(|activity| Broken::LeftOpen {
        activity: activity.to_owned(),
    }));
    (pairs, broken)
}

fn main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: pair_trace <tracerpt CSV dump>")?;
    let dump = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let (pairs, broken) = pair(&read(&dump)?);
    for pair in &pairs {
        println!("{pair}");
    }
    for fault in &broken {
        println!("broken: {fault}");
    }
    if broken.is_empty() {
        Ok(())
    } else {
        Err(format!("{} events broke the pairing", broken.len()))
    }
}
